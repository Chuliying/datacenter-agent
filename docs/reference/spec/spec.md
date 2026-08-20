# datacenter-agent 現況技術規格

**Spec 版本**：v1.4.0（對應 crate 0.4.0，2026-08-20 同步）
**對應 Target PRD**：[`../prd.md`](../prd.md) v1.4.0
**狀態**：Current-state contract  
**Source**：[`src/server/dto.rs`](../../../src/server/dto.rs)、[`src/server/route.rs`](../../../src/server/route.rs)、[`src/server/handler.rs`](../../../src/server/handler.rs)、[`src/test_support.rs`](../../../src/test_support.rs)、[`src/runtime/turn.rs`](../../../src/runtime/turn.rs)、[`src/runtime/config.rs`](../../../src/runtime/config.rs)

> 本規格只記錄目前程式。未落地的修正與目標設計在 [程式修改計劃](../../../.agent/artifacts/plan/2026-06-29-runtime-correctness/implementation.md)。

## 版本歷史

| 版本 | 日期 | 內容 | 對應 PRD |
|---|---|---|---|
| v1.0.0 | 2026-06-29 | 初版現況快照 | v1.0.0 |
| v1.1.0 | 2026-06-29 | 校正雙路徑、SSE、runtime wiring 與 failure modes | v1.1.0 |
| v1.2.0 | 2026-06-29 | 對照 target Capability/Evidence architecture，記錄現況直接 LLM→MCP gap | v1.2.0 |
| v1.3.0 | 2026-06-30 | 同步 runtime default cutover、rollback-before-config、injection/memory、provider terminal 與 eval process gate | v1.3.0 |
| v1.3.1 | 2026-07-01 | 全文逐條對照程式碼複核，內容確認與現況一致，無需修正；容器化封裝（`Dockerfile`／`chore(docker)` commit）屬部署範疇，不在本 spec（HTTP/runtime contract）涵蓋範圍內 | v1.3.0 |
| v1.4.0 | 2026-08-20 | 同步 crate 0.4.0：路由表收斂為 5 條＋outer fallback 統一 404（`Router::merge` 帶走 sub-router fallback 的成因）；`AgentResponse` DTO 與 legacy serving path 移除；SSE validation error 改回 HTTP status；channel bounded；`run_agent_turn` 記為 dormant | v1.4.0 |

## 1. 系統邊界

```text
HTTP client
  → axum Router / per-group bearer + timeout / shared middleware
  → handler
      → runtime prelude（plan_stream_turn：audit → guardrails → intent → answer policy → memory）
      → sub-agent pipeline（agent::wiring：fetcher → analyst → charter/composer → …）
      → OpenRouter LLM ⇄ MCP tools
```

startup 由 `main.rs` 載入 top-level config、連 MCP、建立 AppState、啟動 Router。`AppState::new` 預設組裝 runtime；明確 `RUNTIME_ENABLED=false/0` 時在讀 capability config 前跳過 runtime build。

0.4.0 起**沒有 legacy serving path**：兩個 prompt 端點（`/agent/stream`、`/v1/chat/completions`）都要求 runtime，rollback 時回 503，僅剩 `/health`、`/ready`、`/greeting` 可用。`run_agent_turn` 與 `AgentPort` trait 仍存在但 **dormant**——production 只呼叫其前段 `plan_stream_turn`，streaming 由 handler 直接驅動 sub-agent pipeline。

## 2. HTTP contract

### 2.1 Routes 與 middleware

| Method | Path | Handler | Auth 失敗 | Body cap | Group timeout |
|---|---|---|---|---|---|
| GET | `/health` | `health` | 418 JSON | 64 KiB | 120 s |
| GET | `/ready` | `ready` | 418 JSON | 64 KiB | 120 s |
| GET | `/greeting` | `greeting` | 418 JSON | 64 KiB | 120 s |
| POST | `/agent/stream` | `agent_stream` | 418 JSON | 64 KiB | 120 s（只限建立 Response 前的 handler future） |
| POST | `/v1/chat/completions` | `chat_completions` | **401 + OpenAI error envelope** | 64 KiB | **600 s**，逾時回 504 + envelope |

Router 由兩個 sub-router merge 而成：standard 群（前四條，`require_bearer`、120 s `TimeoutLayer`）與 OpenAI 群（`require_bearer_openai`、600 s tower timeout + `HandleErrorLayer` 產生 envelope）。auth 與 timeout 都 layer 在各自群上，不在外層。

**未匹配路徑**：外層 router 在兩次 merge 之後設明確 `fallback`，對所有未匹配路徑回統一 `404`（body 刻意為空——任一群的 error envelope 都不該擁有它），**與 `Authorization` header 無關**。這是路由層行為，不是 auth 行為。成因：`Router::merge` 會把 sub-router 的 fallback 一起帶走，0.4.0 之前 merged fallback 是 OpenAI 群的、包在該群 auth layer 內，導致任何未匹配路徑無 token 回 401、有效 token 回 404——每條路徑都成為 token 有效性的 oracle。router-level 迴歸測試：`retired_paths_return_404_regardless_of_authorization`、`unmatched_paths_do_not_leak_token_validity`、`surviving_paths_are_still_routed`（[`src/server/route.rs`](../../../src/server/route.rs)）。

退役路徑 `/insight`、`/insight/stream`、`/report`、`/report/stream` 與更早的 `POST /agent` 都由上述 fallback 回 404。

shared middleware（包住兩群）：trace、`CorsLayer::very_permissive()`、compression、`X-Content-Type-Options: nosniff`、`Referrer-Policy: no-referrer`、64 KiB `DefaultBodyLimit`。standard 群 bearer 失敗回 418 JSON，不使用 401 challenge；OpenAI 群失敗回 401 + envelope（該群的 wire 相容需求）。

### 2.2 Request DTO

等價 Rust 定義：

```rust
pub struct AgentRequest {
    #[serde(default)]
    pub history: Vec<History>,
    pub prompt: String,
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub option_id: Option<String>,
}
```

JSON 使用 snake_case 欄位；只有 `prompt` 必填。所有 `JsonRejection` 目前被 `From<JsonRejection>` 映射為 HTTP 400。

### 2.3 非串流 response

`AgentResponse` DTO（`{user_prompt, model_response, intent}`）已於 0.4.0 **移除**——它只服務已退役的非串流端點。現況唯一的非串流 prompt 回應是 `/v1/chat/completions` 的標準 OpenAI `chat.completion` envelope；wire 細節見 [`../endpoints/chat-completions.md`](../endpoints/chat-completions.md)。

### 2.4 SSE wire

`StreamFrame` 使用 `#[serde(tag = "event", rename_all = "lowercase")]`，共 **9 種 variant**，`/agent/stream` 全部外送（`insight_frames` 映射 `AgentEvent` → `StreamFrame`）：

| event | payload | 說明 |
|---|---|---|
| `intent.resolved` | `IntentResolvedData`（camelCase） | prelude 解析結果，只在 stream 開頭一次 |
| `stage` | `StageData` | sub-agent stage 轉換（started/success/failure） |
| `token` | string | 答案增量 |
| `tool_call` | `ToolCallData` | 模型的 tool-call 意圖 |
| `tool_args` | `ToolArgsData` | tool 參數增量 |
| `usage` | `UsageData` | token 用量 |
| `clear` | — | 清除已送答案 buffer |
| `done` | — | 正常終止 |
| `error` | string | 錯誤終止 |

v1.3.x 記錄的「`ToolCalled`/`ToolResult` 不映射到外部 SSE」描述的是已 dormant 的 runtime `TurnEvent` 路徑，**不適用**現行 handler 直驅 pipeline 的 wire。逐 frame 契約與範例見 [`../endpoints/agent-stream.md`](../endpoints/agent-stream.md)。

## 3. Serving path 與 rollback

### 3.1 Runtime 必要性

`runtime_enabled_from_env` 以 trim 後 case-insensitive 比對 `RUNTIME_DISABLED_VALUES = ["false", "0", "no", "off", "disabled"]`（`src/appstate.rs`），五種拼法都觸發 rollback；其他值與未設均啟用 runtime。0.4.0 起 rollback **不再選擇替代 serving path**：兩個 prompt 端點都回 503（`runtime disabled (RUNTIME_ENABLED=false)` 訊息；OpenAI 端點以 envelope 包裝），僅剩 `/health`、`/ready`、`/greeting` 可用。rollback 時 `AppRuntime` 為 None 且在讀 capability config 前跳過 runtime build，壞 config 不阻擋 startup（`explicit_rollback_skips_invalid_runtime_config`）；runtime enabled 但 top-level `[runtime]` 缺失則 startup fail，不靜默降級。

### 3.2 Prompt validation

兩端點共用 runtime prelude 的 `input_guard`，cap 單一來源 `thresholds.input.max_prompt_chars`（4000）。

| Path | 空／超長的外部行為 |
|---|---|
| `/agent/stream` | prelude 在建立 SSE Response **之前**執行；`StreamPlan::Error` 經 `status_to_app_error` 映射為對應 HTTP status（如 400），**不是** 200 + error frame |
| `/v1/chat/completions` | 同一 prelude；`StreamPlan::Error` 映射為對應 status + OpenAI error envelope |

v1.3.x 記錄的「SSE 已回 Response 才驗證、錯誤是 HTTP 200 + error frame」已不成立。64 KiB body limit 位於 Router；目前沒有 route-level test 固定 oversized JSON 的最終 status。因 `JsonRejection` 統一轉 `AppError::BadRequest`，不可只靠 middleware 宣稱一定是 413。

### 3.3 Timeout

timeout 以 sub-router 群為單位：standard 群 120 s（`TimeoutLayer::with_status_code` → 504 空 body），OpenAI 群 600 s（tower timeout + `HandleErrorLayer` → 504 + envelope；非串流路徑要等整條 pipeline 跑完，600 s 是必要的）。兩群的 layer 在 merge 後各自存活（`per_group_timeout_layers_survive_a_merge`）；envelope 行為由 `openai_timeout_returns_openai_error_envelope` 固定。SSE handler 建立 Response 後的 body/producer 不受 handler timeout 保證；若 turn 需全程 deadline，現況沒有獨立的 body timeout/cancellation contract。

## 4. Runtime contracts

### 4.1 Core traits and enums

```rust
#[async_trait]
pub trait AgentPort: Send + Sync {
    async fn stream_turn(
        &self,
        input: AgentTurnInput,
    ) -> RuntimeResult<BoxStream<'static, AgentTurnFrame>>;
}

pub enum AgentTurnOutcome {
    Final { response: String, intent: String },
    Refused { reason: String, copy: String },
    Error { code: String, status: u16 },
    Aborted { response: String },
}

pub enum TurnEvent {
    IntentResolved { intent: String, candidate_intents: Vec<String> },
    Token { data: String },
    Clear,
    Done,
    Error { data: String },
}
```

### 4.2 Shared orchestration

兩個 prompt 端點都呼叫 **`plan_stream_turn`**（runtime prelude：audit request → prompt validation → input pipeline → optional LLM normalizer → answer policy → optional memory context），拿到 `StreamPlan` 後由 **handler 自己**驅動 intent-selected sub-agent pipeline（`agent::wiring`）並做 streaming／envelope 組裝。

`run_agent_turn` 與 `AgentPort` trait 仍在 [`src/runtime/turn.rs`](../../../src/runtime/turn.rs) 但 **dormant**——沒有 production caller；handler 傳入的 `UnusedAgentPort` + no-op emit 不會被 prelude 執行。把 pipeline 收回 `AgentPort` seam 之後是計劃工作，不是現況。

SSE streaming 現況：**bounded** `tokio::sync::mpsc::channel(8192)` + spawned orchestrator task；handler 在 stream 收尾時 `run.await` 觀察 JoinError 並寫 memory／terminal audit。client disconnect 的明確 abort/cancellation contract 與 slow-consumer 行為仍未以測試固定。

### 4.3 Input pipeline

`InputPipeline::run_with_config` 目前固定呼叫：

```text
normalize_text → injection detector → classify_intent → extract_slots
```

`InputPipeline.stages` 與 `RuntimeConfig.assembly.input_stages` 不參與 dispatch。config 可調 intent/lexicon/部分 thresholds，但 time parsing、option mapping 與其他規則仍有 Rust 實作。

### 4.4 Injection and answer policy

`InjectionDetector` 在 config load 時編譯一次，request pipeline 命中時產生 `prompt_injection_detected` warning；`RuleAnswerPolicy` 拒絕且不呼叫 upstream。injection refusal 不持久化，memory context 也重用同一 detector 過濾。

`RuleAnswerPolicy` 的 refusal/disclaimer thresholds 讀 `thresholds.confidence.answer_gray/answer_normal`；numeric range/order validation 尚未完整。

### 4.5 Memory

`SessionMemoryScope` 型別支援 `actor_id`，但 production load/append 都傳 None。key 因此實際是 anonymous + client session id。context formatter 以 request normalization + capability `InjectionDetector` 做 whole-field filtering；總長超過 `max_memory_context_chars` 時回 None，沒有 partial truncation。

### 4.6 Audit

`AuditRecord` 保留 raw `session_id`，只有 actor IP/user-agent（若存在）會 hash。`StdoutAuditSink` 直接序列化 record；`redact_secrets` helper 未被 sink 呼叫。REST/SSE handler 的 `AuditCtx.actor` 目前均為 None。

### 4.7 Registry

| Builder / config area | Production wiring |
|---|---|
| answer policy | yes |
| memory | yes |
| audit sink | yes |
| LLM normalizer | yes，預設 disabled/no-op |
| input stages | builder 只回 ID vector；AppState 未用 |
| extractors / guardrails | validation metadata；沒有 dispatch |
| evaluators | `NoopEvaluator`，production/eval runner 未用 registry evaluator pipeline |

## 5. LLM/MCP adapter behavior

`llm_connector::agent_stream` 讀 OpenAI-compatible chunks、累積 content/tool calls、執行 MCP 並將結果回灌下一輪。

已知 failure semantics：

- transport stream `Err` 會 emit `LlmEvent::Error`。
- final turn 只接受 `finish_reason=stop`；tool turn 只接受相容的 tool finish + 完整 calls。自然 EOF、length、content filter 或不相容 terminal 都 emit Error。
- `generate` 若 inner stream 結束且沒收到 Done/Error，仍回 `Ok(out)`。
- MCP `CallToolResult.is_error=true` 只記 warning，`call_tool_text` 回 `Ok(text)`；caller 因此 emit `ToolResult { ok: true }`。

## 6. Capability / Evidence architecture gap

Target PRD v1.2.0 定義：Capability Registry → controlled Gateway/Tool Hub → Evidence Hub/Evidence Pack → Prompt Builder → tool-less Final LLM → Output Validator。

目前程式相反：

```text
GenerationConfig + discovered MCP tool schemas
    → OpenRouter LLM
    → model emits tool calls
    → McpHandle.call_tool_text
    → tool result回灌同一 LLM
    → final text
```

現況不存在下列 type/port/module contract：

- `SkillPackage` / versioned capability resolution。
- `EvidencePack` / evidence item / citation / provenance / freshness / classification / digest。
- `EvidenceHub` / retrieval planner。
- `CapabilityGateway` / per-tool policy、scope、credential、budget mediation。
- deterministic `PromptBuilder`。
- 不持有 tool/MCP/DB/RAG handle 的 `FinalLlmPort`。
- `OutputValidator` 的 schema/citation validation與bounded repair。

目前 `LlmAgentPort` 持有 `tools: Arc<Vec<ChatCompletionTool>>` 與 `McpHandle`，因此無法滿足 Final LLM isolation。這是 target architecture 的**缺口**，不是 current implementation contract。

## 7. Error mapping

| Source | Mapping |
|---|---|
| `AppError::BadRequest` | 400 + `{ "error": ... }` |
| `AppError::BadGateway` | 502 + error body |
| `AppError::ServiceUnavailable` | 503 + error body |
| auth rejection（standard 群） | 418 + error body，繞過 `AppError` |
| auth rejection（OpenAI 群） | 401 + OpenAI error envelope（`require_bearer_openai`） |
| 未匹配路徑 | 404、空 body、與 Authorization 無關（outer fallback，見 §2.1） |
| timeout before Response | 504（standard 空 body；OpenAI + envelope） |
| runtime `InputRequired/InputTooLong` | 400 |
| runtime `Upstream` | 502 |
| other runtime errors | 503 |

完整 upstream error chain 目前可能進 502 body或 SSE error frame；這是現況，不是建議的安全目標。

## 8. Eval contract

| Mode | Actual scope |
|---|---|
| pipeline-only | 3 個 fixtures，直接執行 `InputPipeline`，驗 intent/slots |
| response replay | artifact-based deterministic checks |
| response live | provider-backed；需要明確授權與外部服務 |

`src/bin/eval.rs` 在 `run(mode)` 回 Err 或 `EvalReport.failed > 0` 時 exit 1；integration test 以 failing replay 驗證 process status。

## 9. Current config values

| Key | Value / behavior |
|---|---|
| runtime prompt cap | 4000 |
| answer policy effective thresholds | config `answer_gray` / `answer_normal`（目前 0.5 / 0.7） |
| intent allowlist | `unknown`, `revenue`, `charging`, `site-build`, `member`, `report`（`report` 亦驅動 pipeline routing，見 `wants_report_pipeline`） |
| memory max turns | 5 |
| memory context chars | 1200 |
| runtime enabled env | default true；`false`/`0`/`no`/`off`/`disabled`（case-insensitive）都 rollback |

## 10. Verification evidence and gaps

2026-08-20 fresh `cargo test` 為 **220 passed、0 failed、6 ignored**（lib 208 ＋ bin/eval 4 ＋ integration 8；ignored 為 5 個 live LLM/MCP test 與 1 個 doc test）。`cargo fmt --check` 通過；`eval --pipeline-only` passed=3。這不等於所有 HTTP/async failure mode 已被覆蓋。

0.4.0 **已補上**的覆蓋：router-level 路由表迴歸（退役路徑 404、存活路徑仍被路由）、未匹配路徑對 Authorization 的一致性、per-group timeout 在 merge 後存活、OpenAI timeout envelope（`src/server/route.rs` 五個測試，依 `src/test_support.rs` 的 stub MCP fixture 組出真 `AppState`）。

主要 coverage gaps：

- Router-level auth **envelope**（418/401 body 本身）、body cap、JSON rejection 的最終 status。
- runtime SSE 的 frame contract（validation 已改 pre-stream HTTP status，frame 序列仍無 route-level test）。
- slow consumer、client disconnect、producer cancellation 的外部契約。
- 真 provider transport 的 EOF/truncation integration（finish-state unit contract 已有）。
- MCP `is_error` 到 audit `ok` 的語意。
- eval evaluator quality semantics（process exit gate 已有）。
- production audit redaction、actor/session isolation。
- Evidence Pack schema/integrity/freshness/citation、gateway policy、Prompt Builder與 Final LLM dependency isolation。

來源與逐項分類見 [QA 現況](../tests/qa-plan.md)。

## 11. Related documents

- [Reference root](../index.md)
- [Reverse PRD](../prd.md)
- [QA evidence](../tests/qa-plan.md)
- [Code change plan](../../../.agent/artifacts/plan/2026-06-29-runtime-correctness/implementation.md)
