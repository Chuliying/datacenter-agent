---
output: docs/work/agentgateway-openai-endpoint/spec.md
stage: spec
slug: agentgateway-openai-endpoint
prd: docs/work/agentgateway-openai-endpoint/prd.md
---

# Spec — OpenAI 相容 `POST /v1/chat/completions` 端點

## Capability Snapshot（manifest）

| flag | 值 | 影響 |
|---|---|---|
| has_api | true | 新增對外 HTTP API;提供 request/response 範例 |
| typed_contracts | true | Rust serde 型別;實作後以 `cargo check` 驗證 |
| has_ui | false | 無 UI/mockup/token（Transformer N/A） |
| has_e2e | false | 無 E2E |
| Story 類型 | **新功能** | 使用 spec-template |

## 目標

方案 A:新增 `POST /v1/chat/completions`(OpenAI 相容,含 SSE)掛 agentgateway Path C,共用現有 runtime prelude + sub-agent pipeline;`/agent/stream` 與豐富事件原樣保留。

## 關鍵技術決策（Decisions）

> D1/D2 修正 PRD 的技術假設(基於 code 研究,佐證見各項)。

### D1 — 串流採「偽串流」（真串流不可行）
- **事實**:sub-agent pipeline 的最終答案由終端純邏輯 stage 事後組裝——insight 的 `Finalizer` 把 analyst 報告 `trim_end` 後**附加 charts fenced block**([pipeline.rs:304](src/agent/pipeline.rs:304));report 的 `Renderer` 產出注入 `report.data` 的 **HTML**([pipeline.rs:434](src/agent/pipeline.rs:434))。`AgentEvent::ContentDelta` 只由 streaming LLM adapter 發([llm.rs:485](src/agent/llm.rs:485)),是各 stage 的**中間 preview**,終端純邏輯 stage 不發。故**無任何 token 流等於最終答案**;現有契約在 `Finished` 時發 `[Clear, Token(完整答案), Done]`([handler.rs:821](src/server/handler.rs:821))撤回 preview。OpenAI `delta` 無撤回語意。
- **決策**:跑 pipeline 到 `AgentEvent::Finished`(或 buffered `run()` 拿 `FinalResult.assistant`),取完整答案字串,**切塊逐塊送** `chat.completion.chunk`(`choices[0].delta.content`);首塊帶 `role:"assistant"`,末塊 `delta:{}` + `finish_reason:"stop"`,再 `data: [DONE]`。**不轉發中間 preview**。
- **影響**:首 token 延遲 ≈ 完整計算時間(等同現有 clear+重送語意,只是改為分塊)。這是 pipeline 架構的本質限制,不是實作選擇。

### D2 — 非串流走 buffered pipeline（Option A），不字面接 `run_agent_turn`
- **事實**:`run_agent_turn` / `LlmAgentPort` 驅動的是**舊 monolith** loop([turn.rs:156](src/runtime/turn.rs:156)),production 未使用;`AgentTurnOutcome` 也無 usage 欄位([turn.rs:35](src/runtime/turn.rs:35))。
- **決策**:非串流仿 `/agent/stream` 但 buffered——`plan_stream_turn` prelude → `wants_report_pipeline(&normalized)` 選 pipeline → `build_*_pipeline(..., sink=None)` → `Orchestrator::run()` → `final_answer(outcome)`([handler.rs:748](src/server/handler.rs:748))。達成 **REST/stream 同後端(pipeline)**,契合使用者「REST/stream 同組件」判準。

### D3 — usage 自行累加
- pipeline 每個 stage 各發一次 `AgentEvent::Usage`([llm.rs:634](src/agent/llm.rs:634)),**目前無任何累計**。新端點累加所有 `Usage`(sum prompt/completion/total,reasoning 有則 sum)填 OpenAI `usage`。⚠️ buffered `OpenAiLlm` **不發 Usage**([llm.rs:401](src/agent/llm.rs:401)):非串流若要 usage,需改用 streaming client + 收集 sink(見 Steps 5)。

> **實作狀態(2026-07-23,已補齊,符合 OpenAI contract)**：
> - **非串流 usage 已取值**：`openai_buffered_response` 不再走 buffered `run()`,改走 **streaming client + drain**(`build_openai_pipeline(Some(sink))` → `run_emitting` → 收集 `AgentEvent::Usage`),`accumulate_usage` 填實值(不動 `llm.rs`/`payload.rs`)。
> - **串流 usage 已上 wire**:`ChatCompletionRequest.stream_options.include_usage` 為 `true` 時,於內容 chunk 之後、`data: [DONE]` 之前多送一個 **usage-only chunk**(`choices: []` + `usage`),符合 OpenAI `include_usage` 語意;未設時 wire 完全不變。**(第二輪 #5 補正)** 啟用 include_usage 時,每個 content chunk 另帶 `usage: null`(見下方「第二輪 §#5」);未啟用則完全不帶 usage 欄位。
> - **post-stream audit 已補**:buffered/stream 兩路徑均寫 `AuditEvent::ResponseCompleted`(完成)/`ResponseFailed`(失敗),對齊 `/agent/stream`。
> - **memory 仍 inert**:OpenAI body 無 `session_id`(恆 `None`),無 turn 可複製,故 `append_memory_turn_if_enabled` 省略(設計如此,非落差)。

### D4 — prompt 長度上限 4000（runtime）
走 `plan_stream_turn` prelude → `input_guard::validate_prompt(prompt, 4000)`([input_guard.rs:6](src/runtime/guardrails/input_guard.rs:6)、[config.rs:38](src/runtime/config.rs:38))。非 legacy 2000。

### D5 — system message 忽略
pipeline 無 system 槽(各 stage 自帶 designed instruction,[payload.rs:179](src/agent/payload.rs:179))。傳入 `role:"system"` 忽略(spec 明確;不 prepend,避免污染既有 stage prompt 設計)。

### D6 — auth bearer（/v1 回 401,其他 7 端點維持 418）
> **2026-07-23 修正(finding #6)**:原設計繼承共用 `require_bearer`(失敗 418),對 OpenAI client / gateway 不友善。
新路由改掛**專屬** `require_bearer_openai`([auth.rs](src/server/auth.rs)):**相同 constant-time 比對 `GLOBAL_TOKEN`**,但失敗回 **401 + OpenAI error envelope**(`invalid_request_error`),符合 OpenAI 慣例。**共用 `require_bearer`(其他 7 端點的 418 契約)完全不動**——route 拆成 standard / openai 兩個 sub-router,各自掛自己的 auth layer 再 `.merge()`。

### D7 — runtime-off 行為
端點需 runtime(走 prelude);`RUNTIME_ENABLED=false` 時回 **503** + OpenAI 風格 error body(與 `/agent/stream` 一致,[handler.rs:455](src/server/handler.rs:455))。

### D8 — 非串流 timeout 覆寫 600s（finding #1）
> 全域 `TimeoutLayer` 為 120s([route.rs](src/server/route.rs));非串流 `/v1/chat/completions` 需 await **整條** sub-agent pipeline,常超過 120s → 被砍成空 body 504。
route 拆 standard(120s)/ openai(**600s**)兩 sub-router,各自套 `TimeoutLayer` 再 `.merge()`,故只有 `/v1/chat/completions` 得到較長 timeout,其他 7 端點維持 120s。SSE 串流的 response handle 立即回傳,body timeout 本就不影響(D1 偽串流)。
> **2026-07-23 修正(第二輪 #4)**:openai sub-router 逾時原回空 body → 改回 **OpenAI error envelope**(504),見下方「第二輪 code review 決策 §#4」。

### D9 — disclaimer prefix 併入答案（finding #7）
`StreamPlan::Proceed` 的 answer-policy `prefix`(如免責聲明)原被丟棄。OpenAI `delta`/`message` 無撤回語意(不同於 `/agent/stream` 靠終端 `clear` 蓋掉),故改為 **prepend 進最終答案**:`answer = if prefix 非空 { format!("{prefix}\n\n{answer}") } else { answer }`,串流/非串流皆套。

## 第二輪 code review 決策（2026-07-23）

> 本節 D10 與 #2/#4/#5 註記為**第二輪** review 修正（與上方第一輪 finding 編號不同源）。

### D10 — history 織入 prompt（第二輪 #1）
- **事實**:engine `ConfiguredAgent::run`([engine.rs:234](src/agent/engine.rs:234))`AgentPayload::Initial(p) => (p.prompt, …)` **只取 `p.prompt`**,`InitialPrompt.history` 完全不進 LLM;`map_request` 映射出的 history 因此白費,多輪對話塌成只剩最後一則 user。
- **決策(handler 層,不動 engine / `/agent/stream` / falcon)**:抽純函式 `fold_history_into_prompt(&[Exchange], &str) -> String`,history 非空時把各輪 render 成 `User: …\nAssistant: …` transcript **prepend** 到目前問題前;空 history 原樣回傳(單輪不變)。`chat_completions` 的 `Proceed` 分支用它組 `Initial.prompt`,`Initial.history` 留空(engine 本就忽略,避免重複)。**僅改 `chat_completions` 一處**;`/agent/stream` 建 `InitialPrompt` 之處未動。
- **影響**:多輪 OpenAI 請求現能讓第一 stage 的 LLM 看到完整對話;prompt 長度上限(D4,prelude 對「目前問題」驗 4000)在 fold 之前完成,history 不計入該 cap。

### 第二輪 #4 — timeout 回 OpenAI envelope（修正 D8）
D8 的 openai sub-router 逾時原回**空 body**(tower_http `with_status_code`)。改用 `ServiceBuilder`:`axum::error_handling::HandleErrorLayer`(外)包 `tower::timeout::TimeoutLayer`(內);逾時的 `Elapsed` 由 `handle_openai_middleware_error` 映成 **504 + OpenAI error envelope**(`server_error`)。standard sub-router 的 120s(tower_http,空 body)未動;auth 仍最外層。

### 第二輪 #5 — include_usage 時 content chunk 帶 `usage:null`（修正 D3）
OpenAI `include_usage` 契約:啟用時**每個 content chunk 帶 `usage: null`**,只有終端 usage-only chunk 帶實值;未啟用完全不帶。`ChatCompletionChunk.usage` 改 `Option<Option<Usage>>`(`None`=省略、`Some(None)`=顯式 `null`、`Some(Some(u))`=實值);`build_chunks` 收 `include_usage` flag 決定一般 chunk 為 `Some(None)` 或 `None`。

### 第二輪 #2 — 斷線 abort：保留現狀（無 code 變更）
`state.mcp` 為跨請求共用、多工的 rmcp `Peer`(`McpHandle` 包單一 peer)。mid-request abort 對共用 peer 的取消清理語意**離線無法確證**,corruption 風險 > 省下的計算成本,且與 sibling `agent_stream`(同不 abort)一致。supervised cancellation 列為後續(需先外驗 rmcp 取消語意)。

## Files

| 檔案 | 動作 | 內容 |
|---|---|---|
| `src/server/openai.rs` | **NEW** | OpenAI DTO(`ChatCompletionRequest/Response/Chunk`、`ChatMessage`、`Usage`)+ `messages↔AgentRequest` 映射 + usage 累加 helper |
| `src/server/handler.rs` | **MODIFY** | 新增 `chat_completions` handler;複用 `sse_event`/`INSIGHT_STREAM_BUFFER`/`final_answer`/`wants_report_pipeline`/`insight_error_to_app_error` |
| `src/server/route.rs` | **MODIFY** | `Router::new()`([route.rs:61](src/server/route.rs:61))auth layer **之前**加 `.route("/v1/chat/completions", post(handler::chat_completions))` |
| `src/server/mod.rs` | **MODIFY** | `mod openai;` |
| `src/agent/llm.rs` / `payload.rs` | **MODIFY(僅非串流要 usage)** | buffered `OpenAiLlm::chat` 補發 `Usage`,或 `FinalResult` 加 usage 欄位([payload.rs:229](src/agent/payload.rs:229) 已預留 EXTEND) |

> 不動 `run_agent_turn`(D2);Option C(`PipelineAgentPort`)不在本次範圍。

## Contracts（資料契約）

### 新增 OpenAI DTO（`src/server/openai.rs`）
```rust
#[derive(Deserialize)]
pub struct ChatCompletionRequest {
    pub model: String,
    pub messages: Vec<ChatMessage>,
    #[serde(default)] pub stream: bool,
}
#[derive(Deserialize)]
pub struct ChatMessage { pub role: String, pub content: String }

#[derive(Serialize)]
pub struct ChatCompletionResponse {   // stream=false
    pub id: String, pub object: &'static str /* "chat.completion" */,
    pub created: i64, pub model: String,
    pub choices: Vec<Choice>, pub usage: Usage,
}
#[derive(Serialize)]
pub struct Choice { pub index: u32, pub message: ChatMessage, pub finish_reason: String }

#[derive(Serialize)]
pub struct ChatCompletionChunk {      // stream=true (每塊)
    pub id: String, pub object: &'static str /* "chat.completion.chunk" */,
    pub created: i64, pub model: String, pub choices: Vec<ChunkChoice>,
}
#[derive(Serialize)]
pub struct ChunkChoice { pub index: u32, pub delta: Delta, pub finish_reason: Option<String> }
#[derive(Serialize, Default)]
pub struct Delta { #[serde(skip_serializing_if="Option::is_none")] pub role: Option<String>,
                   #[serde(skip_serializing_if="Option::is_none")] pub content: Option<String> }

#[derive(Serialize, Default)]
pub struct Usage { pub prompt_tokens: u32, pub completion_tokens: u32, pub total_tokens: u32,
                   #[serde(skip_serializing_if="Option::is_none")] pub completion_tokens_details: Option<ReasoningDetails> }
```
> `created` 由呼叫端傳入(避免在純函式取時間);`id` 用固定前綴 + 計數/隨機來源由 handler 提供。

### 既有內部型別（引用,不改）
`AgentRequest`([dto.rs:27](src/server/dto.rs:27))[EXISTING]、`StreamPlan`/`AgentTurnDeps`([turn.rs:70](src/runtime/turn.rs:70))[EXISTING]、`AgentEvent`([agent](src/agent))[EXISTING]、`UsageData`([dto.rs:155](src/server/dto.rs:155))[EXISTING,可複用累加]。

### 映射規則 `messages → AgentRequest`（2026-07-23 放寬,finding #2）
> 原規則對真實 OpenAI client 的合法形狀過嚴(一律 400),放寬如下:
- **`content` 接受 string 或 content-parts 陣列** `[{"type":"text","text":..}]`:陣列取所有 `text` part **串接**,非 text part(如 `image_url`)忽略(自訂 untagged `deserialize_with`)。回應側 `content` 恆序列化為 string。
- **`role:"system"` 與 `role:"developer"`**(OpenAI 新 system 同義)→ 皆忽略(D5)。
- **不再對非交替結構一律 400**:先把**相鄰同 role** 合併為一則(content 以 `\n` 連接),使序列嚴格交替;再取**最後一則 user** 當 `prompt`、其前摺成 `history: Vec<History>{user_prompt, model_response}`;**開場 assistant** 以空 `user_prompt` 配對。
- 仍要求有 user 當 prompt:空 `messages`(或全 system/developer 移除後為空)→ 400 `NoUserMessage`;合併後結尾非 user → 400 `BadShape`。皆 `ERR2`(`invalid_request_error`)。
- **`tools`/`tool_choice`**:本端點驅動固定內部 pipeline,不支援 client 端 tool-calling → **接受但忽略**(不 deserialize,serde 預設丟棄未知欄位)。
- `session_id`/`option_id` → OpenAI body 無 → `None`(memory 停用,等同 `sessions=None`)。

## API

### `POST /v1/chat/completions`（bearer required,D6）

Request(OpenAI 標準):
```json
{"model":"<rd-model>","messages":[{"role":"user","content":"上個月營收多少?"}],"stream":false}
```
Response(stream=false):
```json
{"id":"chatcmpl-…","object":"chat.completion","created":1750000000,"model":"<rd-model>",
 "choices":[{"index":0,"message":{"role":"assistant","content":"<report+charts 或 HTML>"},"finish_reason":"stop"}],
 "usage":{"prompt_tokens":…,"completion_tokens":…,"total_tokens":…}}
```
Response(stream=true,SSE 逐塊,偽串流 D1):
```
data: {"id":"chatcmpl-…","object":"chat.completion.chunk","choices":[{"index":0,"delta":{"role":"assistant"},"finish_reason":null}]}
data: {"…","choices":[{"index":0,"delta":{"content":"<答案分塊 1>"},"finish_reason":null}]}
data: {"…","choices":[{"index":0,"delta":{"content":"<答案分塊 2>"},"finish_reason":null}]}
data: {"…","choices":[{"index":0,"delta":{},"finish_reason":"stop"}]}
data: [DONE]
```

## Data Flow

```
messages ──(映射)──▶ AgentRequest ──▶ plan_stream_turn(dummy AgentPort + no-op emit)  [prelude 共用]
                                          │
                                StreamPlan┤
   Error   ─▶ OpenAI error(依 status)
   Refused ─▶ 200 + copy 當單一 assistant 內容(stream 則分塊)
   Proceed ─▶ wants_report_pipeline(&normalized) 選 report / insight pipeline
                 ├─ stream=false: build_*_pipeline(sink=None) → Orchestrator::run() → final_answer → ChatCompletionResponse   [D2]
                 └─ stream=true : build_*_pipeline(sink) → spawn run_emitting → 收 AgentEvent
                        累加 Usage(D3);忽略中間 ContentDelta;
                        於 Finished 取完整 assistant → 切塊送 chunk → finish_reason:"stop" → [DONE]   [D1]
```
prelude 呼叫樣板見 [handler.rs:496](src/server/handler.rs:496)(dummy `UnusedAgentPort` + no-op emit,`plan_stream_turn` 不碰 agent/emit)。

## Errors

| 代號 | 觸發 | HTTP | body |
|---|---|---|---|
| ERR1 | RUNTIME_ENABLED=false | 503 | OpenAI error(D7) |
| ERR2 | messages 空/無 user/格式錯/非法 JSON | 400 | OpenAI error(`invalid_request_error`) |
| ERR2a | **body 過大**(超過 64 KiB body limit) | **413** | OpenAI error(finding #6;`json_rejection_status`) |
| ERR2b | **content-type 錯/缺** | **415** | OpenAI error(finding #6) |
| ERR3 | prompt > 4000(D4) | 400 | OpenAI error |
| ERR4 | bearer 失敗 | **401**(D6,finding #6) | **OpenAI error envelope**(`require_bearer_openai`) |
| ERR5 | LLM/MCP capability 失敗 / pipeline 無最終結果 | 502 | OpenAI error(`upstream_error`;answer/failure 取自 `run.await` 權威值,finding #3) |

## Steps（分步實作）

1. [ ] **OpenAI DTO + 映射 + usage helper**（45min）
   - 檔案:`src/server/openai.rs` [NEW]、`src/server/mod.rs` [MODIFY]
   - `messages→AgentRequest`、`accumulate_usage(&[UsageData])`、chunk/response 建構 helper
2. [ ] **映射單元測試**（30min）— 依賴 1;多輪/空/system/相鄰同 role/長度
3. [ ] **`chat_completions` handler 非串流路徑**（60min）[MODIFY handler.rs]— 依賴 1;prelude→分流→Option A buffered→`ChatCompletionResponse`
4. [ ] **串流路徑(偽串流)**（45min）— 依賴 3;收到 Finished 取完整答案切塊送 chunk + `[DONE]`
5. [ ] **usage 累加**（30min）— 依賴 3/4;串流端累加 `AgentEvent::Usage`;非串流若要 usage 需 buffered adapter 補發(否則標註從缺)
6. [ ] **route 註冊**（10min）[MODIFY route.rs]— auth layer 之前
7. [ ] **整合測試 + 規格書 C-3 curl**（45min）— 對照 AC1–5;`cargo test` + 手動 curl(串流/非串流)

**總估時 ≈ 4h15m**（不含 D3 buffered-usage 的擴充,若需另 +30–45min）

## Test Strategy

- **Unit**(`tests/` 或模組內):
  - `messages→AgentRequest`:單輪 / 多輪配對 / 只有 system(→400)/ 空(→400)/ 相鄰同 role(→400)。
  - `accumulate_usage`:多筆 Usage 加總,reasoning 有無。
  - chunk 序列化格式(delta.content、finish_reason、`[DONE]` sentinel)。
- **Integration**(對照 AC):
  - AC1 非串流:200 + `choices[0].message.content` 非空 + `usage`。
  - AC2 串流:逐塊 `delta.content`,末塊 `finish_reason:"stop"`,收到 `data: [DONE]`。
  - AC3:`/agent/stream` 既有測試不受影響(回歸)。
  - AC5:injection 輸入 → 走 prelude 被 refuse(對照 `/agent/stream` refuse 行為)。
  - ERR1 runtime-off → 503;ERR3 超長 → 400;ERR4 無 token → 418。

## Gate 2 自檢

| 類別 | 結果 | 證據 |
|---|---|---|
| Environment | PASS | auth 沿用 require_bearer(D6);無 rate-limit 現況一致;manifest 無 `environment_rules` → 依既有慣例 |
| Tech Research | PASS | code 研究報告(pipeline 串流語意、run_agent_turn、plan_stream_turn),repo evidence 充分 |
| 檔案清單 | PASS | 每個 FR 有對應檔案(見 Files) |
| Typed contract | PENDING→實作驗 | 契約已定義;實作步驟 1 後以 `cargo check` 驗證(typed_contracts=true) |
| API 範例 | PASS | request/response/chunk 範例符合上述契約 |
| Transformer | N/A | has_ui=false |
| 無未定義引用 | PASS | 複用符號均標 [EXISTING] 並附 file:line |
| 資料流 | PASS | Data Flow 圖含 stream/非-stream 兩路 |
| 測試案例 | PASS | Unit + Integration 規劃對照 AC |
| UI Token/Mockup | N/A | has_ui=false |
| 版控 | PASS | 見版本歷史 |

> 一個 Gate 2 殘留待實作確認:**Typed contract** 需步驟 1 完成後 `cargo check` 實跑驗證(spec 階段無 code 無法編譯)。

## 版本歷史

| 時間 | 內容 | 對應 PRD 版本 | 作者 |
|---|---|---|---|
| 2026-07-22 | 初版;含 D1 偽串流、D2 buffered pipeline 兩項對 PRD 技術假設的修正 | prd v1(approved) | Chuliying + Claude |
| 2026-07-23 | code review findings 修正:映射放寬(#2)、非串流 timeout 600s(D8/#1)、auth /v1→401(D6/#6)、body 錯誤 413/415/400(#6)、disclaimer prepend(D9/#7)、run.await 權威(#3)、logprobs/system_fingerprint 欄位(#5)、refusal include_usage(#4) | prd v1(approved) | Chuliying + Claude |
| 2026-07-23 | 第二輪 code review 修正:history 織入 prompt(D10/#1)、timeout 回 OpenAI envelope(D8 修正/#4)、include_usage content chunk `usage:null`(D3 修正/#5)、斷線 abort 保留(#2)、pipeline-build 失敗補 audit(#6);manifest value backtick(#3,非 spec 範圍) | prd v1(approved) | Chuliying + Claude |
