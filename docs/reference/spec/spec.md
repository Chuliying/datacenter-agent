# datacenter-agent 現況技術規格

**Spec 版本**：v1.4.0
**對應 Target PRD**：[`../prd.md`](../prd.md) v1.4.0
**狀態**：Current-state contract  
**Source**：[`src/server/dto.rs`](../../../src/server/dto.rs)、[`src/server/route.rs`](../../../src/server/route.rs)、[`src/server/handler.rs`](../../../src/server/handler.rs)、[`src/runtime/orchestrator.rs`](../../../src/runtime/orchestrator.rs)、[`src/runtime/config.rs`](../../../src/runtime/config.rs)

> 本規格只記錄目前程式。未落地的修正與目標設計在 [程式修改計劃](../../../.agent/artifacts/plan/2026-06-29-runtime-correctness/implementation.md)。

## 版本歷史

| 版本 | 日期 | 內容 | 對應 PRD |
|---|---|---|---|
| v1.0.0 | 2026-06-29 | 初版現況快照 | v1.0.0 |
| v1.1.0 | 2026-06-29 | 校正雙路徑、SSE、runtime wiring 與 failure modes | v1.1.0 |
| v1.2.0 | 2026-06-29 | 對照 target Capability/Evidence architecture，記錄現況直接 LLM→MCP gap | v1.2.0 |
| v1.3.0 | 2026-06-30 | 同步 runtime default cutover、rollback-before-config、injection/memory、provider terminal 與 eval process gate | v1.3.0 |
| v1.3.1 | 2026-07-01 | 全文逐條對照程式碼複核，內容確認與現況一致，無需修正；容器化封裝（`Dockerfile`／`chore(docker)` commit）屬部署範疇，不在本 spec（HTTP/runtime contract）涵蓋範圍內 | v1.3.0 |
| v1.4.0 | 2026-07-11 | 對照 commit `813b275`：新增 `/report`、`/report/stream` 報表端點（繞過 runtime、`report_system` prompt、`REPORT_MAX_TOKENS`、`completion_inner`/`prepare_config` 重構）；`member` intent 進入 allowlist；記錄 report path 與 Final LLM tool 持有關係 | v1.4.0 |

## 1. 系統邊界

```text
HTTP client
  → axum Router / bearer / middleware
  → handler
      ├─ legacy /agent:   llm_connector → OpenRouter → MCP
      ├─ runtime /agent:  run_agent_turn → LlmAgentPort → llm_connector → OpenRouter → MCP
      └─ /report:         completion_inner / agent_stream → llm_connector → OpenRouter → MCP（不經 runtime）
```

startup 由 `main.rs` 載入 top-level config、連 MCP、建立 AppState、啟動 Router。`AppState::new` 預設組裝 runtime；明確 `RUNTIME_ENABLED=false/0` 時在讀 capability config 前跳過 runtime build。

## 2. HTTP contract

### 2.1 Routes 與 middleware

| Method | Path | Handler | Bearer | Body cap | Handler timeout |
|---|---|---|---|---|---|
| GET | `/health` | `health` | required | 64 KiB layer | 120s |
| GET | `/ready` | `ready` | required | 64 KiB layer | 120s |
| GET | `/greeting` | `greeting` | required | 64 KiB layer | 120s |
| POST | `/agent` | `agent` | required | 64 KiB | 120s |
| POST | `/agent/stream` | `agent_stream` | required | 64 KiB | 只限制建立 Response 前的 handler future |
| POST | `/report` | `report` | required | 64 KiB | 120s |
| POST | `/report/stream` | `report_stream` | required | 64 KiB | 只限制建立 Response 前的 handler future |

`/report`、`/report/stream` 與 `/agent` 對稱（相同 `AgentRequest` DTO、相同 REST/SSE 回應型別），差別見 §2.5。

middleware 另含 trace、`CorsLayer::very_permissive()`、compression、`X-Content-Type-Options: nosniff`、`Referrer-Policy: no-referrer`。Bearer 失敗回 418 JSON，不使用 401 challenge。

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

### 2.3 REST response

```rust
pub struct AgentResponse {
    pub user_prompt: String,
    pub model_response: String,
    pub intent: String,
}
```

```json
{
  "user_prompt": "本月充電量？",
  "model_response": "...",
  "intent": "charging"
}
```

legacy path 的 `intent` 固定為 `"unknown"`。runtime `Final` 使用 normalized intent；`Refused`、`Aborted` 回 `"unknown"`。

### 2.4 SSE wire

`StreamFrame` 使用 `#[serde(tag = "event", rename_all = "lowercase")]`；`IntentResolved` 額外 rename 為 `intent.resolved`，payload 使用 camelCase。

```text
data: {"event":"intent.resolved","data":{"intent":"charging","candidateIntents":["charging"]}}
data: {"event":"token","data":"..."}
data: {"event":"clear"}
data: {"event":"done"}
data: {"event":"error","data":"..."}
```

`intent.resolved` 只由 runtime path 產生。`ToolCalled`/`ToolResult` 不映射到外部 SSE。

### 2.5 報表端點 `/report`

`report` / `report_stream`（`src/server/handler.rs`）與 `/agent` 對稱，但：

- **繞過 runtime**：`report` 直接呼叫 `completion_inner` → `llm_connector::generate`；`report_stream` 直接呼叫 `llm_connector::agent_stream`。兩者都**不看 `state.runtime`**，因此不經 input pipeline、injection/answer policy、memory 與 runtime audit；`should_use_runtime` 分支只作用於 `/agent`。
- **system prompt**：使用 `state.prompts.report_system`（`report_system` prompt，boot 時由 `PromptBank::from_app_config` 載入，缺失即 startup fail）；`/agent` 用 `agent_system`。
- **output ceiling**：`REPORT_MAX_TOKENS = 16_384`（handler 常數）覆寫 `LlmDefaults::max_tokens`，以容納完整 HTML 文件；經 `prepare_config` 的 `max_tokens_override` 傳入。
- **prompt cap**：走 `completion_inner` → `prepare_config` → `validate_prompt`，沿用 legacy `USER_PROMPT_LENGTH_CAP = 2000`（**非** runtime 4000）。
- **輸出**：`report_system` 契約要求回覆單一 info string 為 `falcon-report` 的 fenced code block，內含完整 self-contained HTML（設計系統 `<style>` verbatim、KPI/表格/meta 由 tool 資料寫入、bar/line 圖表資料 inline 於 `<script>`，唯一外部資源為 Chart.js CDN）；chit-chat 則回純 Markdown。SSE 變體逐 token 串流該區塊，wire 契約與 `/agent/stream` 相同。

重構：`prepare_config(state, req, base_system, max_tokens_override)` 與新的 `completion_inner(state, req, base_system, max_tokens_override)` 由 `/agent`、`/agent/stream`、`/report`、`/report/stream` 共用。

## 3. 雙路徑差異

### 3.1 Route selection

`runtime_enabled_from_env` 只有 trim 後 case-insensitive `false` 或字串 `0` 視為 rollback；其他值與未設均啟用 runtime。rollback 時 `AppRuntime` 為 None，handler 走 legacy；runtime enabled 但 top-level `[runtime]` 缺失則 startup fail，不靜默降級。

### 3.2 Prompt validation

| Path | Limit source | Current cap | 空／超長的外部行為 |
|---|---|---:|---|
| legacy `/agent` | `USER_PROMPT_LENGTH_CAP` | 2000 | HTTP 400 |
| legacy `/agent/stream` | same helper | 2000 | HTTP 400 before SSE |
| runtime `/agent` | `thresholds.input.max_prompt_chars` | 4000 | `AgentTurnOutcome::Error{status:400}` → HTTP 400 |
| runtime `/agent/stream` | same runtime guard | 4000 | handler 已回 SSE Response；送 `error` frame，HTTP 200 |
| `/report` | `USER_PROMPT_LENGTH_CAP` | 2000 | HTTP 400 |
| `/report/stream` | same helper | 2000 | HTTP 400 before SSE |

64 KiB body limit 位於 Router；目前沒有 route-level test 固定 oversized JSON 的最終 status。因 `JsonRejection` 統一轉 `AppError::BadRequest`，不可只靠 middleware 宣稱一定是 413。

### 3.3 Timeout

`TimeoutLayer` 包住 handler future。REST slow request 可在 120 秒回 504；SSE handler 建立 Response 後的 body/producer 不受這個 timeout 保證。若 turn 需全程 deadline，現況沒有獨立的 body timeout/cancellation contract。

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

REST 與 runtime SSE 都呼叫 `run_agent_turn`：REST 使用 no-op emit 並讀 outcome；SSE 將 emit 寫入 channel。prelude 順序為 audit request → prompt validation → input pipeline → optional LLM normalizer → answer policy → optional memory context。

SSE adapter 現況使用 `tokio::sync::mpsc::unbounded_channel` 與 spawned producer。send failure 被忽略；client disconnect 時沒有明確 abort/cancellation，因此不存在 backpressure 與「斷線即停止上游成本」保證。

### 4.3 Input pipeline

`InputPipeline::run_with_config` 目前固定呼叫：

```text
normalize_text → injection detector → classify_intent → extract_slots
```

`InputPipeline.stages` 與 `RuntimeConfig.assembly.input_stages` 不參與 dispatch。config 可調 intent/lexicon/部分 thresholds，但 time parsing、option mapping 與其他規則仍有 Rust 實作。

intent allowlist 於 commit `813b275` 加入 `member`（keywords 會員/會員數/會員成長/註冊/留存/活躍會員/membership/member）；lexicon 刻意排除泛用「成長」字，使 `營收成長` 仍歸 `revenue` 而非 `member`。pipeline 單元測試 `member_growth_prompt_classifies_as_member_and_is_answerable`、`revenue_growth_prompt_stays_on_revenue_not_member` 固定此行為（此路徑只作用於 runtime `/agent`，`/report` 不經 intent 分類）。

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

`/report` 端點放大了這個缺口：它直接呼叫 `llm_connector`（同樣把 MCP tool schemas 交給 Final LLM），且完全繞過 runtime 的 injection/answer policy/memory/audit。target architecture 下，報表生成應同屬受控 gateway → Evidence Pack → tool-less Final LLM 流程；FR-015 的 Privacy Proxy 亦需涵蓋 `/report` 出境路徑。

## 7. Error mapping

| Source | Mapping |
|---|---|
| `AppError::BadRequest` | 400 + `{ "error": ... }` |
| `AppError::BadGateway` | 502 + error body |
| `AppError::ServiceUnavailable` | 503 + error body |
| auth rejection | 418 + error body，繞過 `AppError` |
| timeout before Response | 504 |
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
| intent allowlist | `unknown`, `revenue`, `charging`, `site-build`, `member` |
| memory max turns | 5 |
| memory context chars | 1200 |
| runtime enabled env | default true；只有明確 `false`/`0` rollback |
| report system prompt | `report_system`（`prompt_guide/report_system.md`）；boot 時必載，缺失即 startup fail |
| report output ceiling | `REPORT_MAX_TOKENS = 16384`（handler 常數，`/report` 覆寫 `LlmDefaults::max_tokens`） |

## 10. Verification evidence and gaps

2026-06-30 fresh `cargo test` 為 92 passed、2 ignored、0 failed。這不等於所有 HTTP/async failure mode 已被覆蓋。

主要 coverage gaps：

- Router-level auth、body cap、timeout、JSON rejection。
- runtime SSE validation 的 HTTP status/frame contract。
- slow consumer、client disconnect、producer cancellation、JoinError。
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
- [Privacy Proxy 功能 spec](../features/privacy-proxy/spec.md)（FR-015，規劃中）
- [Code change plan](../../../.agent/artifacts/plan/2026-06-29-runtime-correctness/implementation.md)
