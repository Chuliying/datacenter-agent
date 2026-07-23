---
output: docs/work/agentgateway-openai-endpoint/implement-report.md
stage: implement
slug: agentgateway-openai-endpoint
status: in_progress
---

# Implement Report — OpenAI 相容 `/v1/chat/completions` 端點

**Execution Mode**: team-feature（PRD approved / Spec approved / qa-plan ready 皆讀取）
**cargo toolchain**: `~/.rustup/toolchains/stable-aarch64-apple-darwin`（PATH 未含 cargo,以完整 toolchain 路徑執行）

## Step 1 — 環境與邊界 [PASS]
- Mode team-feature;inputs 三者皆備。
- manifest:`test_cmd=cargo test` / `typecheck_cmd=cargo check` / `lint_cmd=cargo clippy -- -D warnings`;tests 慣例為模組內 `#[cfg(test)]`。
- 範圍限 spec Files:`openai.rs`[NEW]、`mod.rs`[MODIFY]、待做 `handler.rs`/`route.rs`。

## Step 2 — RED-GREEN-REFACTOR

### 迴圈 1 — `messages → AgentRequest` 映射（TC-U01..05）
- **RED**:`map_request` = `todo!()`,`cargo test --lib server::openai` → 6 FAIL（`not yet implemented`,observed failing,非編譯錯）。
  - 修正 1:`assert_eq!` 比較整個 `Result` 需 `AgentRequest: PartialEq` → 改用 `.unwrap_err()` 只比 `MapError`（不動現有型別）。
- **GREEN**:實作 `map_request`（drop system、最後 user→prompt、前面配對→history、交替驗證）→ 6 PASS。

### 迴圈 2 — usage 累加（TC-U06）
- **RED**:`accumulate_usage` = `todo!()` → FAIL。
  - 修正:`use serde::{Deserialize, Serialize}`（新增 response DTO 需 Serialize）。
- **GREEN**:sum prompt/completion/total,reasoning 有才 `Some` → PASS。

### 迴圈 3 — chunk 切塊（TC-U07 / TC-B05）
- **RED**:`build_chunks` = `todo!()` → FAIL。
- **GREEN**:leading role chunk → char-safe content chunks（CHUNK_CHARS=24）→ terminal `finish_reason:"stop"` → PASS。空答案亦正確終止。

**目前測試**:`cargo test --lib server::openai` → **10 passed / 0 failed**（166 既有 filter）。

## 已產出/修改檔案
- `src/server/openai.rs` [NEW] — DTO（ChatMessage/ChatCompletionRequest/Usage/ReasoningDetails/ChatCompletionChunk/ChunkChoice/Delta）+ `map_request`/`accumulate_usage`/`build_chunks` + 10 unit tests。
- `src/server/mod.rs` [MODIFY] — `pub mod openai;`。

## 剩餘（未完成）
- `ChatCompletionResponse`/`Choice` DTO（非串流 response）。
- `handler::chat_completions`（最大塊):`plan_stream_turn` prelude（dummy port + no-op emit）→ 分流 StreamPlan → `wants_report_pipeline` 選 pipeline;非串流 buffered `run()`→`ChatCompletionResponse`（D2）、串流 `run_emitting`→收 Finished→`build_chunks`→SSE（D1);usage 累加。
- `route.rs`:auth layer 之前註冊 `/v1/chat/completions`。
- L4 整合測試（受控 LLM/MCP 替身）:TC-I01..04、TC-ERR01..05。
- 完整驗證:`cargo test`（全）+ `cargo check` + `cargo clippy -- -D warnings`。

## Gate（迴圈 1–3 當時，進行中）
- type-check:PENDING（完整 `cargo check` 待 handler 完成後）
- lint:PENDING（`cargo clippy`）
- test:PARTIAL — openai 純函式層 10/10;整合測試 + 全 suite 待做
- UI mockup:N/A（has_ui=false）

---

# 續作 — handler 整合批（迴圈 4–6 + handler/route）

**toolchain**：`~/.rustup/toolchains/stable-aarch64-apple-darwin/bin`（PATH prefix 執行）。
基線：進場時 `cargo test --lib server::openai` → 10 passed；全 crate 可編譯。

## Step 2（續）— RED-GREEN

### 迴圈 4 — 非串流 response DTO（AC-1 / spec D2）
- 新增 `ChatCompletionResponse{id,object:"chat.completion",created,model,choices,usage}` + `Choice{index,message:ChatMessage,finish_reason}` + `build_response(answer,id,model,created,usage)`。`ChatMessage` derive 補 `Serialize/PartialEq/Eq`（回應的 assistant message 重用同型別，spec 契約 `message: ChatMessage`）。
- **RED**：`build_response` = `todo!()`,測 `build_response_wraps_answer_as_a_single_stop_choice` + `response_serializes_to_the_openai_wire_shape` → 2 panic（`not yet implemented: build_response`,observed）。
- **GREEN**：single choice、index 0、`finish_reason:"stop"`、`role:"assistant"`、usage 透傳 → 2 PASS。

### 迴圈 5 — OpenAI 錯誤碼對映（spec Errors）
- 新增 `MapError::to_openai()->(u16,&str,String)` + `error_type_for_status(u16)->&str` + 常數 `ERR_INVALID_REQUEST/ERR_UPSTREAM/ERR_SERVER`。
- **RED**：兩函式 `todo!()`,測 `map_error_maps_to_400_invalid_request` + `error_type_tracks_the_http_status` → 2 panic（observed）。
- **GREEN**：`MapError`→(400, invalid_request_error);status 4xx→invalid_request_error、502→upstream_error、其餘 5xx→server_error → 2 PASS。

### 迴圈 6 — OpenAI 錯誤 envelope（純資料，序列化 pin）
- 新增 `OpenAiErrorBody{error:OpenAiError{message,type}}` + `OpenAiErrorBody::new`。測 `openai_error_body_serializes_to_the_nested_envelope` 固定 `{"error":{"message","type"}}` wire shape（純資料,首跑即綠,無需 RED todo）。

**openai 模組**：`cargo test --lib server::openai` → **15 passed / 0 failed**（166 filtered）。

## handler / route 整合（無新單元測；理由見下）
- `src/server/handler.rs`[MODIFY]：新增 `chat_completions`（`#[instrument(skip_all)]`)+ 私有 helper `openai_error` / `agent_error_to_openai` / `openai_chunk_event` / `build_openai_pipeline` / `openai_refusal` / `openai_buffered_response` / `openai_stream_response`。
  - 前置：body 解析失敗→400；`runtime` None/disabled→503（D7）；`map_request` Err→其 `to_openai()`（400,D5 忽略 system）；`created`（`SystemTime`→unix secs）+ `id`（`chatcmpl-{uuid}`）於 handler 生成。
  - prelude：完全比照 `agent_stream`——`UnusedAgentPort` + no-op emit + `AgentTurnDeps` → `plan_stream_turn`（route 標 `/v1/chat/completions`）。
  - 分流：`StreamPlan::Error{code,status}`→`error_type_for_status` 對映 HTTP + OpenAI error；`Refused{copy}`→200,copy 當單一 assistant（串流走 `build_chunks`,非串流走 `build_response`);`Proceed`→選 pipeline 跑。**`prefix`（disclaimer）刻意丟棄**——`/agent/stream` 終端 `clear` 本就蓋掉它,交付內容不含,對齊之。
  - 非串流（D2）：`build_openai_pipeline(None)` → `Orchestrator::run` → `Final.assistant` → `build_response`;capability 失敗→`agent_error_to_openai`（Capability→502、其餘→503）。
  - 串流（D1 偽串流）：`build_openai_pipeline(Some(sink))` → `tokio::spawn(run_emitting)` → drain：忽略 `ContentDelta`、累加 `AgentEvent::Usage`→`UsageData`、`Finished` 取完整答案、`Error` 記 failure;收完 → `build_chunks` 逐塊 `Event::json_data`（純 `data:` 行）+ `data: [DONE]`;無答案時 in-band OpenAI error object + `[DONE]`（headers 已 200,無法改 status）。
- `src/server/route.rs`[MODIFY]：auth layer **之前** `.route("/v1/chat/completions", post(handler::chat_completions))`,繼承 `require_bearer`（D6,418）。
- `src/server/mod.rs`：`pub mod openai;`（迴圈 1 已加）。

## 驗收 gate（實跑 evidence）
| gate | 指令 | 結果 |
|---|---|---|
| type-check | `cargo check` | **PASS**（Finished dev profile,無 error/warning） |
| lint | `cargo clippy -- -D warnings` | **PASS**（無 warning）;額外 `cargo clippy --all-targets -- -D warnings` 亦 **PASS**（含新測試碼） |
| test | `cargo test` | **PASS** — lib **181/0**（前基線 176 + 新 5)、eval bin 4/0、`deployment_contract` 1/0、`eval_cli` 1/0、`runtime_contract` 4/0;live 整合測試（agent_pipeline/fetcher/streaming/llm_connector/repro）皆 `#[ignore]`（需 live MCP+OpenRouter）。**全 suite 0 failed**。 |

AC-3（回歸）：既有 lib 181 + 整合測試全綠,證 `/agent/stream`、`/insight*`、`/report*`、runtime 行為未被破壞。

## TC 覆蓋
| TC | 層級 | 狀態 | 覆蓋方式 |
|---|---|---|---|
| TC-U01..05（map_request 映射/錯誤） | L2 | ✅ | openai 單元測（迴圈 1，10 測內） |
| TC-U06（usage 累加） | L2 | ✅ | openai 單元測 |
| TC-U07 / TC-B05（chunk 切塊/空答案） | L2 | ✅ | openai 單元測 |
| ERR2 對映（MapError→400 invalid_request） | L2 | ✅ | 迴圈 5 `map_error_maps_to_400_invalid_request` |
| ERR1/ERR3/ERR5 狀態→type 對映 | L2 | ✅ | 迴圈 5 `error_type_tracks_the_http_status` + envelope 序列化 |
| AC-1 非串流組裝（`build_response`） | L2 | ✅ | 迴圈 4 兩測 |
| TC-I01..04（端到端非串流/串流/回歸/injection） | L4 | ⚠️ 部分 | 回歸(TC-I03)✅;非串流/串流/injection **端到端待手動 curl（規格書 C-3）**——見下方限制 |
| TC-ERR01（runtime off→503） | L4 | ⚠️ | handler 邏輯已實作;端到端待 curl |
| TC-ERR02（bad JSON→400） | L4 | ⚠️ | 已實作;端到端待 curl |
| TC-ERR03/TC-B01/B02（4000 邊界） | L4 | ⚠️ | 走 prelude 4000 cap（D4);端到端待 curl |
| TC-ERR04（auth→418） | L4 | ⚠️ | route 繼承 `require_bearer`;端到端待 curl |
| TC-ERR05（capability→502） | L4 | ⚠️ | `agent_error_to_openai` 已實作;端到端待 curl |

## 限制與待辦（未造假,未連真實服務）
1. **L4 真整合無法離線自動化**：`McpHandle` 包住 live `rmcp::Peer<RoleClient>`,無 fake/mock 建構子;`build_*_pipeline` 內部直接建 `OpenAiLlm`/`StreamingOpenAiLlm` + `McpTool`,handler 層無注入 fake LLM/MCP 的接縫（scripted fake 僅在 `ConfiguredAgent` 單元層可用,不經 `build_*_pipeline`）。repo 既有 pipeline 整合測試（`tests/*_datacenter.rs`、`agent_pipeline.rs`、`repro_report_data.rs`）**全部 `#[ignore]` 且需 `DATACENTER_MCP_URL` + `OPENROUTER_API_KEY`**。故 TC-I01..04 / TC-ERR01..05 端到端維持**手動 curl 驗證（規格書 C-3）**,未加造假替身,未連真實 OpenRouter/MCP。
2. **非串流 usage 從缺（D3）**：buffered `OpenAiLlm::chat` 只 log 不發 `Usage`（`llm.rs:401`),且 `Orchestrator::run`（非 emitting）不收集;故非串流 `usage` 填 `Usage::default()`（全 0)。改善需動 `llm.rs`/`payload.rs`（本批範圍外,spec 已標註）。
3. **串流 usage 僅記 log**：串流累加 `AgentEvent::Usage` 後以 `tracing::info!` 輸出,**不放上 wire**——`ChatCompletionChunk` DTO 無 usage 欄位,spec SSE 範例亦無 usage 行,故不擅自加欄位。
4. **未複製 `agent_stream` 的 post-stream 副作用**：OpenAI 端點不寫 `ResponseCompleted`/`ResponseFailed` audit、不做 memory append（OpenAI body 無 `session_id`,memory 本就 inert;prelude 已寫 RequestReceived/InputNormalized/Refused）。

## 本批修改檔案
- `src/server/openai.rs`[MODIFY,untracked 新檔續補] — `ChatCompletionResponse`/`Choice`/`build_response`、`OpenAiErrorBody`/`OpenAiError`、`MapError::to_openai`、`error_type_for_status`、`ERR_*` 常數 + 5 新單元測（openai 模組 15/15）。
- `src/server/handler.rs`[MODIFY] — `chat_completions` + 7 helper。
- `src/server/route.rs`[MODIFY] — 註冊 `/v1/chat/completions`（auth 前）。

## 最終 Gate
- type-check：**PASS**（`cargo check`）
- lint：**PASS**（`cargo clippy -- -D warnings`;`--all-targets` 亦 PASS）
- test：**PASS**（`cargo test` 全 suite 0 failed;lib 181/0）
- UI mockup：N/A（has_ui=false）
- L4 端到端：**已驗證（本機 mock stub，2026-07-22）** — 見下方端到端段落

## 端到端驗證（全本機 mock stub 繞過錯 host）

拓樸：`agent(127.0.0.1:18080) → datacenter-mcp(127.0.0.1:8088) → mock stub(127.0.0.1:9099)`，
LLM 為真實 `anthropic/claude-opus-4.7`。stub 對 6 個 `starcharger/api/v2/*` endpoint 回符合
`dto.rs` 契約的假 JSON array（繞過 `DATACENTER_API_BASE=falcon.andywu.uk` 錯 host）。

| 測試 | 期待 | 實測 |
|---|---|---|
| T1 無 token | 418 | **418** ✓（D6 繼承 require_bearer） |
| T2 bad-json + token | 400 | **400** + `{"error":{"message":..,"type":"invalid_request_error"}}` ✓ |
| T3 空 messages + token | 400 | **400** `messages must contain at least one user message` ✓ |
| T4 非串流成功查詢 | 200 OpenAI `chat.completion` | **200** ✓ `choices[0].message.content`（含 falcon-chart）、`finish_reason:"stop"`、`usage` 全 0（D3） |
| T5 串流成功查詢 | SSE chunks + `[DONE]` | **27 chunks** ✓ 首塊 `delta.role="assistant"` → content 塊（累加還原=完整答案）→ 末塊 `delta:{},finish_reason:"stop"` → `data: [DONE]`；`object:"chat.completion.chunk"` |

結論：wiring / auth(D6) / 映射(D5) / prelude / pipeline 驅動 / 非串流(D2) / 串流偽串流(D1) / 錯誤 envelope / SSE `[DONE]` 全部端到端 work。D3 usage 全 0 經實測確認（已知限制）。真實上游成功查詢仍待正確 `DATACENTER_API_BASE` host（規格書 C-3）。

---

# 續作 — usage + audit 補強（2026-07-23）

補齊兩個已知落差：**usage 符合 OpenAI contract**（原限制 #2、#3）與 **post-stream audit**（原限制 #4）。**僅動新端點 helper，不碰 `/insight`、`/report` 的 `run()`，不動 `llm.rs`/`payload.rs`。**
基線：進場 `cargo test` → lib **181/0**（全 suite 191 passed，其餘 ignored）。

## Step 2（續）— RED-GREEN

### 迴圈 7 — 串流 usage DTO/純函式（`src/server/openai.rs`）
純資料/序列化 pin，unit RED→GREEN：
- **RED**：新增 4 測（`stream_options_deserializes_include_usage`、`request_stream_options_default_none_and_parse`、`content_chunks_omit_the_usage_field`、`usage_chunk_serializes_with_empty_choices_and_usage`）引用尚未存在的 `StreamOptions` / `ChatCompletionRequest.stream_options` / `ChatCompletionChunk.usage` / `usage_chunk()` → `cargo test --lib server::openai` **5 compile error**（`cannot find type StreamOptions`×2、`no field stream_options`×2、`cannot find function usage_chunk`）observed。
- **GREEN**：
  - `StreamOptions { #[serde(default)] include_usage: bool }`（Deserialize、Default）。
  - `ChatCompletionRequest` 加 `#[serde(default)] stream_options: Option<StreamOptions>`。
  - `ChatCompletionChunk` 加 `#[serde(skip_serializing_if = "Option::is_none")] usage: Option<Usage>`；`build_chunks` 的 `mk` closure 填 `usage: None`（平常 chunk wire 不變，`content_chunks_omit_the_usage_field` 驗序列化無 `usage` key）。
  - `usage_chunk(usage, id, model, created)`：`choices: []` + `usage: Some(..)`，序列化 pin `{"choices":[],"usage":{...},...}`。
  - → `cargo test --lib server::openai` **19 passed / 0 failed**（15 + 4）。

### 迴圈 8 — 非串流 usage 取值（`handler::openai_buffered_response`，改動一）
buffered `OpenAiLlm` 不發 `Usage`（`llm.rs:401`），原 `run()` 路徑 usage 恆 0。改成**複用串流 drain 模式**：`build_openai_pipeline(state, report, Some(sink))` → `tokio::spawn(run_emitting)` → drain channel 收集 `answer`(from `Finished`)、`usages`(from `Usage`→`dto::UsageData`)、`failure`(from `Error`)，`run.await` 收 panic → `build_response(&answer, id, model, created, accumulate_usage(&usages))`。整合行為（需 pipeline sink），unit 難覆蓋 → 以 code 對齊既有串流 drain 為主，端到端由 curl 驗（見下）。
- 副作用：失敗回應改走 `ERR_UPSTREAM`(502) in-band（與串流路徑一致），原 `agent_error_to_openai`(唯一使用點在此)移除以免 dead_code（clippy `-D warnings`）。

### 迴圈 9 — 串流 usage（OpenAI `include_usage`，改動二）
`openai_stream_response`：當 `include_usage == true`，於 `build_chunks` 末塊之後、`data: [DONE]` 之前多送一個 `usage_chunk`（`choices:[]` + `usage: Some(accumulate_usage(&usages))`）。**未設 `include_usage` 時 wire 完全不變**。`chat_completions` 從 `req.stream_options` 取 `include_usage` 傳入。

### 迴圈 10 — post-stream audit（改動三，對齊 `agent_stream` 646–681）
`chat_completions` 原 `StreamPlan::Proceed { agent_input, normalized, .. }` 忽略 `started` → 取出 `started`，連同 `audit`(AuditWriter)、`audit_ctx` 傳入 buffered/stream 兩路徑：
- 有 `Finished` → `AuditEvent::ResponseCompleted { response_hash: hash_identifier(&answer), response_chars, duration_ms: started.elapsed(), status: "completed" }`；寫失敗只 `warn!` 不影響回應。
- 有 `failure` → `AuditEvent::ResponseFailed { error_code, duration_ms }`。
- 串流 audit 寫在 `async_stream` 尾端（同 `agent_stream`）；非串流寫在回 response 前。
- **memory**：OpenAI 端點 `session_id` 恆 `None`（memory inert），故**省略** `append_memory_turn_if_enabled`（無 turn 可複製）。

## 驗收 gate（實跑 evidence，2026-07-23）
| gate | 指令 | 結果 |
|---|---|---|
| type-check | `cargo check` | **PASS**（Finished dev profile，無 error/warning） |
| lint | `cargo clippy --all-targets -- -D warnings` | **PASS**（無 warning） |
| test | `cargo test` | **PASS** — lib **185/0**（181 基線 + 4 新 openai 測）、eval bin 4/0、`deployment_contract` 1/0、`eval_cli` 1/0、`runtime_contract` 4/0；全 suite **195 passed / 0 failed**；live 整合測試維持 `#[ignore]`。 |

AC-3（回歸）：lib 181 基線 → 185（純增），`/agent/stream`、`/insight*`、`/report*`、runtime 全綠，未破壞既有端點。

## 已解除的原限制
- 原限制 #2（非串流 usage 全 0）→ **解除**：改走 streaming client + drain 收集 `AgentEvent::Usage`，`accumulate_usage` 填實值。
- 原限制 #3（串流 usage 僅 log）→ **解除**：`include_usage=true` 時以 usage-only chunk 上 wire，符合 OpenAI `stream_options.include_usage` 語意；未設時行為不變。
- 原限制 #4（未寫 post-stream audit）→ **解除**：兩路徑皆補 `ResponseCompleted`/`ResponseFailed`。memory 因無 `session_id` 仍 inert（設計如此）。

## 本批修改檔案
- `src/server/openai.rs`[MODIFY] — `StreamOptions`、`ChatCompletionRequest.stream_options`、`ChatCompletionChunk.usage`、`usage_chunk()` + 4 新單元測（openai 模組 19/19）。
- `src/server/handler.rs`[MODIFY] — `chat_completions` 取 `started` + 傳 `include_usage`/`audit`/`audit_ctx`；`openai_buffered_response` 改 drain 模式取 usage + 寫 audit；`openai_stream_response` 加 `include_usage` usage-only chunk + 寫 audit；移除已無使用的 `agent_error_to_openai`；`use std::time::Instant`。

## 待辦（本批範圍外）
- usage/audit 端到端 curl（含 `include_usage:true` 觀察 usage-only chunk、audit sink 落 `ResponseCompleted/Failed`）由主 agent 事後驗；本批以 unit + 對齊既有模式為據，未造假 mock。
- 真實上游成功查詢仍待正確 `DATACENTER_API_BASE` host（規格書 C-3，沿用）。

---

# 續作 — code review findings 修正（2026-07-23）

修 code review 對 `/v1/chat/completions` 的 9 項 findings。**TDD（RED→GREEN）；不破壞既有測試；三 gate 全綠。**
基線：進場 `cargo test --lib` → **185/0**（全 suite 195/0，5 live 測 `#[ignore]`）。

## 逐項處理

### #1 [Important] 非串流 timeout（route.rs）
全域 120s `TimeoutLayer` 會把 `/v1` 非串流長查詢砍成空 body 504。**做法**：`build_router` 拆 `standard`(120s)/ `openai`(**600s**) 兩 sub-router，各自 `.layer(TimeoutLayer)` + 各自 auth，再 `Router::new().merge(standard).merge(openai)`；body limit + 共用 middleware（trace/cors/compression/headers，**已把 timeout 從共用堆疊移出**）套在 merge 後。其他 7 端點維持 120s。
- 測：`route::tests::per_group_timeout_layers_survive_a_merge`——tiny router（50ms/3s）+ sleep(300ms) handler + `oneshot` 驗 504 vs 200，證「per-group timeout 經 merge 仍分別生效」（正是 build_router 的結構）。SSE 不受 body timeout（D1）。

### #2 [Important] map_request 放寬（openai.rs）
- `ChatMessage.content`：自訂 `deserialize_with`（untagged `String | Vec<ContentPart>`）；陣列取 `text` part 串接、非 text 忽略；回應側仍 string。
- role 忽略：新增 `is_ignored_role` 涵蓋 `system` + `developer`。
- 非交替放寬：先合併相鄰同 role（`\n`），再「最後 user→prompt、其前摺 history、開場 assistant 配空 user_prompt」；空→`NoUserMessage`、結尾非 user→`BadShape`（皆 400）。
- `tools`/`tool_choice`：`ChatCompletionRequest` doc 註明接受但忽略（不 deserialize）。
- RED：6 新測（`developer_role_is_ignored_like_system`、`content_parts_array_concatenates_text_parts`、`content_parts_array_maps_through_to_prompt`、`merges_adjacent_same_role_user_turns_into_one_prompt`、`merges_adjacent_same_role_in_history`、`leading_assistant_pairs_with_an_empty_user_prompt`）先跑 **6 FAILED**（行為失敗，非編譯錯）→ 實作後 GREEN。舊 `adjacent_same_role_is_error`（斷言舊 400 行為）改寫為 `merges_...`（新行為）。
- spec.md 映射規則同步更新。

### #3 [Important] lossy sink 正確性（handler.rs buffered + stream）
新增純函式 `resolve_outcome(Result<AgentPayload,AgentError>) -> Result<String,String>`：`Ok(Final)`→answer；`Ok(other)`→Err(無最終結果)；`Err(e)`→Err(`e.to_string()`）。兩路徑 drain **只收 `AgentEvent::Usage`**，answer/failure 改自 `run.await`（`Err(join)`→panic 訊息）。徹底解除對 lossy `try_send` 丟 `Finished`/`Error` 的依賴，並正確涵蓋 unknown-pipeline 早退（engine.rs 不發 Error 事件）。
- 測：`resolve_outcome_reads_answer_and_failures_from_the_run_result`（Final→answer、Capability(unknown pipeline)→failure、非 Final→Err）。端到端（sink 真的丟事件）為離線不可測，見 #9。

### #4 [Minor] refusal include_usage（handler.rs `openai_refusal`）
簽名加 `include_usage`；串流 refusal 於 `[DONE]` 前補**零值** usage-only chunk（refusal 無 LLM 成本）。RED = arity mismatch（`this function takes 5 arguments but 6 were supplied`）。
- 測（讀 SSE body，無需 AppState）：`refusal_stream_appends_a_zero_usage_chunk_when_include_usage`、`refusal_stream_omits_usage_chunk_without_include_usage`、`refusal_non_stream_returns_a_chat_completion_with_the_copy`。

### #5 [Minor] chunk/response OpenAI 欄位（openai.rs）
`Choice`/`ChunkChoice` 加 `logprobs: Option<serde_json::Value>`（**不 skip → 序列化為 `null`**，貼 OpenAI）；`ChatCompletionResponse`/`ChatCompletionChunk` 加 `system_fingerprint: Option<String>`（**同樣不 skip → `null`**，符合 OpenAI envelope 恆含此欄位）。因 `serde_json::Value` 非 `Eq`，四型別移除 `Eq` derive（保留 `PartialEq`；無外部 `Eq` 依賴）。RED（`contains_key` 對缺欄位失敗）→ 2 測 GREEN：`choices_carry_logprobs_null_for_openai_compat`、`response_and_chunk_carry_system_fingerprint_field`。

### #6 [Minor] error status（handler.rs + auth.rs + route.rs）
- `JsonRejection` 分流：新增純函式 `json_rejection_status(StatusCode)->StatusCode`（413→413、415→415、其餘→400），handler 用 `rejection.status()` 餵入 + `error_type_for_status` 取 OpenAI type。
- auth 418→401：**不動共用 `require_bearer`**（D6，避免 breaking 其他 7 端點）。新增 `auth::require_bearer_openai`（**同 constant-time 比對 `GLOBAL_TOKEN`**，失敗回 **401 + OpenAI envelope**）+ 私有 `openai_unauthorized()`；route 的 openai sub-router 掛此 auth，standard sub-router 維持 `require_bearer`（418）。**判定：router 拆 sub-router 後此分離乾淨，故採 401**（非保留 418）。
- 測：`json_rejection_status_keeps_413_and_415_else_400`（純函式）、`real_json_rejections_map_to_413_415_and_400`（**真的驅動 axum `Json` 抽取器**經 tiny router + `oneshot`，證 `rejection.status()` 產生的碼與映射一致，無需 AppState）、`auth::tests::openai_unauthorized_is_401_with_openai_envelope`。

### #7 [Minor] disclaimer 可見（handler.rs）
新增純函式 `with_prefix(&str, String)->String`；`chat_completions` 取回 `StreamPlan::Proceed.prefix` 傳入 buffered/stream，於取得 answer 後 prepend（`{prefix}\n\n{answer}`）。串流/非串流皆套。測：`with_prefix_prepends_a_nonempty_disclaimer_only`。

### #8 [Minor] 斷線 abort —— **保留現狀（不加 abort）**
評估：spawned run task 被 drop 時 tokio 不自動 abort（維持執行）。`build_*_pipeline` 內 MCP 走 `state.mcp.clone()`——`McpHandle` 是 `#[derive(Clone)]` 包 `Peer<RoleClient>` 的**跨請求共用多工 session**（單一 rmcp peer）。mid-tool-call abort 會 drop 進行中的 rmcp 請求 future，對**共用 peer** 的取消清理無法離線確證，有影響其他併發請求 session 的 corruption 風險。且 sibling production path `agent_stream` 亦不 abort。故**保留現狀**，與 `agent_stream` 一致，不引入 corruption 風險（符合 finding 指引「不確定則保留」）。**無 code 變更。**

### #9 [Minor] handler 整合測試 —— 結構限制，誠實標註
`McpHandle` 無 fake（僅 live `connect_http`），`build_*_pipeline` 直接建 `OpenAiLlm`/`McpTool`——handler 層無注入 fake LLM/MCP 的接縫，故「真的跑 pipeline」的 buffered/stream 端到端**離線不可自動測**（既有 `tests/*_datacenter.rs` 全 `#[ignore]` 需 live）。本批已補所有**不需 pipeline** 的 handler 層真實測試（refusal 兩路徑 body、json rejection 經真 axum 抽取器、auth 401 envelope、timeout 經真 router）。pipeline 兩路徑端到端仍待 mock 基礎設施/live，**未造假 mock**。

## 驗收 gate（實跑 evidence，2026-07-23）
| gate | 指令 | 結果 |
|---|---|---|
| type-check | `cargo check` | **PASS**（exit 0，Finished，無 error/warning） |
| lint | `cargo clippy --all-targets -- -D warnings` | **PASS**（exit 0，無 warning） |
| test | `cargo test` | **PASS** — lib **201/0**（185 基線 + 16 新）、eval 4/0、`deployment_contract` 1/0、`eval_cli` 1/0、`runtime_contract` 4/0；**全 suite 211 passed / 0 failed**；5 live 測維持 `#[ignore]`。 |

新增 16 lib 測：openai +7（#2 淨 +5、#5 +2）、handler +7（#3/#4/#6/#7）、auth +1（#6）、route +1（#1）。AC-3 回歸：185→201 純增，`/agent/stream`、`/insight*`、`/report*`、runtime 全綠。

## 本批修改檔案
- `src/server/openai.rs` — `deserialize_content` + `ChatMessage.content` 放寬、`is_ignored_role` + `map_request` 重寫、`ChatCompletionRequest` tools/tool_choice doc、`Choice`/`ChunkChoice` `logprobs`、`ChatCompletionResponse`/`ChatCompletionChunk` `system_fingerprint`（移除 4 型別 `Eq`）、3 constructor 補欄位 + 8 新測。
- `src/server/handler.rs` — `json_rejection_status`/`with_prefix`/`resolve_outcome` 純函式；`chat_completions` body 錯誤分流 + 取 `prefix` + refusal 傳 `include_usage`；`openai_refusal` 加 `include_usage`；`openai_buffered_response`/`openai_stream_response` 改 Usage-only drain + `run.await` 權威 + prepend prefix；doc 更新 + 7 新測。
- `src/server/auth.rs` — `require_bearer_openai` + `openai_unauthorized` + 1 新測。
- `src/server/route.rs` — `build_router` 拆 standard/openai sub-router（120s/600s timeout + 各自 auth）+ `OPENAI_REQUEST_TIMEOUT` 常數 + 1 新測。

---

# 續作 — 第二輪 code review findings 修正（2026-07-23）

修第二輪 review 的 6 項 findings（本節編號 #1–#6 為**第二輪**，與上一節第一輪的 #1–#9 不同源，勿混）。**TDD（RED→GREEN）；不破壞既有測試；三 gate 全綠。**
基線：進場 `cargo test --lib` → **202/0**（全 suite 0 failed，live 測 `#[ignore]`）。

## 逐項處理

### #1 [🔴] history 被丟棄 → 多輪只用最後一則（handler.rs）
- **根因**：engine `ConfiguredAgent::run`（engine.rs:234）`AgentPayload::Initial(p) => (p.prompt, …)` 只取 `p.prompt`，`InitialPrompt.history` 完全不進 LLM；`/v1/chat/completions` 的多輪對話因此塌成只剩最後一則 user。
- **做法（handler 層，未動 engine / `/agent/stream` / falcon）**：抽純函式 `fold_history_into_prompt(&[Exchange], &str) -> String`——history 非空時把各輪 render 成 `User: …\nAssistant: …` 的 transcript，prepend 到目前問題前（`以下是先前的對話紀錄:\n…\n\n目前的問題:\n…`）；history 空則原樣回傳（單輪 byte-for-byte 不變）。`chat_completions` 的 `Proceed` 分支改用它組 `Initial.prompt`，`Initial.history` 留空（engine 本就忽略，避免與 prompt 內 history 重複）。**僅改 `chat_completions` 一處**；`/agent/stream` 建 `InitialPrompt` 的另一處**未動**。
- **RED**：先放 stub（`fold_history_into_prompt` 只回 `prompt.to_string()`，重現 drop）→ `cargo test --lib fold_history` → 多輪/單輪 2 測 FAILED（`prior user turn missing: 那這個月呢?`、`Q1 present` 失敗，觀察到 bug），空 history 測 PASS。
- **GREEN**：實作真正 fold → 3 測全 PASS。測：`fold_history_into_prompt_returns_prompt_unchanged_when_history_empty`、`…_includes_a_single_prior_turn_before_the_question`、`…_preserves_multi_turn_chronological_order`。

### #2 [tradeoff] 斷線 abort —— **保留現狀（無 code 變更）**
延續第一輪 #8 判定：`state.mcp` 為跨請求共用、多工的 rmcp `Peer`（`McpHandle` 包單一 peer）；mid-request abort 會 drop 進行中的 rmcp 請求 future，其對共用 peer 的取消清理語意**離線無法確證**，corruption 風險 > 省下的計算成本，且與 sibling production path `agent_stream`（同樣不 abort）一致。supervised cancellation 列為後續（需先外部驗證 rmcp 取消語意，再引入受控中止）。**不改 code。**

### #3 [🔴] manifest value backtick（.agent/project-manifest.md）
- **根因**：`scripts/manifest-stack.sh` 的 `manifest_stack_value` sed（`s/^- *\`?${key}\`? *: *//p`）只吃掉 **key** 兩側 backtick，**value** 的 markdown backtick 留著 → `manifest_stack_capability` 的 `case` 拿到 `` `true` `` 不匹配 `true)`（噴「must be true or false」exit 2）；`run_manifest_stack_command` 拿到 `` `cargo test` `` 會被 `bash -c` 當**命令替換**執行。
- **做法**：把 `## Stack` / `## Paths` 內所有 value 的 markdown backtick 去掉（raw 值），key 的 backtick 保留。`## skill-commons bootstrap`（platforms/profile，本就無 backtick）未動。
- **驗證（實跑）**：
  - BEFORE：`manifest_stack_capability has_api` → `❌ … (got: \`true\`)` exit 2；`has_ui` 同；`manifest_stack_value test_cmd` → `` [`cargo test`] ``。
  - AFTER：`has_api` → `[true]` exit 0、`has_ui` → `[false]` exit 0、`has_e2e` → `[false]`、`typed_contracts` → `[true]`、`test_cmd` → `[cargo test]`、`lint_cmd` → `[cargo clippy -- -D warnings]`、`source_roots` → `[src, config]`；`bootstrap/check.sh` exit 0（維持乾淨）。

### #4 [🟡] timeout 504 空 body → 非 OpenAI envelope（route.rs）
- **根因**：openai sub-router 的 `tower_http::TimeoutLayer::with_status_code(GATEWAY_TIMEOUT, 600s)` 逾時回**空 body**，OpenAI client / gateway 無法解析。
- **做法**：openai sub-router 改用 `ServiceBuilder`：外層 `axum::error_handling::HandleErrorLayer::new(handle_openai_middleware_error)` 包內層 `tower::timeout::TimeoutLayer::new(600s)`。新增 `handle_openai_middleware_error(BoxError) -> Response`：`Elapsed` → **504 + OpenAI error envelope**（`error_type_for_status(504)=server_error`），其餘 middleware 錯 → 500 + 同 envelope。auth 仍為最外層（最後 `.layer`）。**standard sub-router 的 120s（`tower_http` with_status_code）行為未動**。tower `timeout` feature 於現有 resolved graph 已啟用（`cargo tree -e features` 確認），未改 Cargo.toml。
- **RED**：先放 stub（`handle_openai_middleware_error` 回空 body 504，重現 finding）→ 新測 `openai_timeout_returns_openai_error_envelope`（tiny router + 50ms tower TimeoutLayer + sleep(300ms) handler + `oneshot`）→ FAILED（`EOF while parsing a value`，body 空，status 504 已對）。
- **GREEN**：填入 envelope → PASS（504 且 `error.type=="server_error"`、`error.message` 為字串）。既有 `per_group_timeout_layers_survive_a_merge`（standard 120s）仍 PASS。

### #5 [🟡] include_usage 時 content chunk 缺 `usage:null`（openai.rs）
- **根因**：`ChatCompletionChunk.usage: Option<Usage>` 一律 `skip_serializing_if`，開 `include_usage` 時一般 chunk **完全不帶** usage 欄位；OpenAI 契約要求開啟時**每個 content chunk 帶 `usage: null`**、只有終端 usage-only chunk 帶實值，未開啟時完全不帶。
- **做法**：`usage` 欄位型別改 `Option<Option<Usage>>`（`None`→省略、`Some(None)`→序列化 `null`、`Some(Some(u))`→實值，單一 `skip_serializing_if="Option::is_none"` 三態）。`build_chunks` 加 `include_usage: bool` 參數：開→一般 chunk `Some(None)`（顯式 null），關→`None`（省略）。`usage_chunk` 改 `Some(Some(usage))`。所有 call site（openai.rs 測 ×5、handler `openai_refusal`/`openai_stream_response`）傳入 flag。
- **RED**：先加參數但**忽略**（`let _ = include_usage;`，`usage` 仍 `None`）→ 新測 `content_chunks_carry_null_usage_when_include_usage`（開啟時每 chunk 應含 `usage` key 且為 null）→ FAILED（序列化無 usage key）。
- **GREEN**：型別 + `mk` 依 flag 填 `Some(None)`/`None` → PASS。pin 測更新：`content_chunks_omit_the_usage_field`（關 → 無 usage key，call 傳 `false`）、`content_chunks_carry_null_usage_when_include_usage`（開 → `usage:null`）；`usage_chunk_serializes_with_empty_choices_and_usage`（`Some(Some)` 仍為物件）不變。既有 refusal include_usage 兩測不受影響。

### #6 [🟡] pipeline-construction 失敗 502 無 audit（handler.rs）
- **根因**：`openai_buffered_response`/`openai_stream_response` 的 `build_openai_pipeline` 失敗 early-return 502 未寫 audit（與成功/stage 失敗路徑不一致）。
- **做法**：兩處回 502 前寫 `AuditEvent::ResponseFailed { error_code: message, duration_ms: started.elapsed() }`（warn-only，用已有 `audit`/`audit_ctx`/`started`）。buffered 本為 `async` 直接 `.await`；stream 原為同步 fn，為了在 early-return await audit，**改為 `async fn` + call site `.await`**（成功路徑仍不 await 即回 SSE handle，不阻塞串流）。
- 測：pipeline-build 失敗需真實 wiring 故障（`McpHandle` 無 fake、`build_*_pipeline` 直建 LLM/MCP），離線無接縫可觸發，故**無專屬 unit 測**（與 finding 未要求測一致），以 compile + clippy + 對齊既有 audit 模式為據；成功/stage-失敗的 audit 已有測涵蓋。

## 驗收 gate（實跑 evidence，2026-07-23）
| gate | 指令 | 結果 |
|---|---|---|
| type-check | `cargo check` | **PASS**（exit 0，Finished，無 error/warning） |
| lint | `cargo clippy --all-targets -- -D warnings` | **PASS**（exit 0，0 warning） |
| test | `cargo test` | **PASS** — lib **207/0**（202 基線 + 5 新）、eval 4/0、`deployment_contract` 1/0、`eval_cli` 1/0、`runtime_contract` 4/0；全 suite **0 failed**；5 live 測維持 `#[ignore]`。 |
| manifest-stack | `source manifest-stack.sh && manifest_stack_capability/value …` | **PASS**（has_api=true、has_ui=false、test_cmd=cargo test，無 backtick；見 #3） |

新增 5 lib 測：handler +3（#1 fold_history）、openai +1（#5 null usage）、route +1（#4 timeout envelope）。AC-3 回歸：202→207 純增，`/agent/stream`、`/insight*`、`/report*`、runtime、既有 openai 端點全綠。

> **rustfmt 註記**：repo 基線在本 toolchain 的 rustfmt 下即有 14 處 diff（含未觸及的 `openai_error` handler.rs:703、既有測試 asserts），非本次引入；`cargo fmt` 非驗收 gate，且全庫格式化會 churn 與 findings 無關的既有碼，故**不執行**；新增碼沿用 repo 既有單行 assert 風格（與既有測試一致）。三 gate（test/check/clippy）全綠。

## 本批修改檔案
- `.agent/project-manifest.md` — `## Stack` / `## Paths` value 去 markdown backtick（#3）。
- `src/server/handler.rs` — `fold_history_into_prompt` 純函式 + `chat_completions` 織入 history（#1）；`build_chunks` call site 傳 `include_usage`（#5）；`openai_buffered_response`/`openai_stream_response` pipeline-build 失敗補 `ResponseFailed` audit、`openai_stream_response` 改 `async` + call site `.await`（#6）；+3 新測（#1）。
- `src/server/openai.rs` — `ChatCompletionChunk.usage: Option<Option<Usage>>`、`build_chunks(+include_usage)`、`usage_chunk` `Some(Some)`、call site 更新（#5）；+1 新測、更新 1 pin 測（#5）。
- `src/server/route.rs` — openai sub-router 改 `HandleErrorLayer` + `tower::timeout::TimeoutLayer` + `handle_openai_middleware_error`（#4）；+1 新測。
