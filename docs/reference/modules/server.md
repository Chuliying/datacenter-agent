# 模組：`server`（HTTP 層）

> ← 回 [模組總覽](./index.md) ｜ [專案主體](../index.md)
>
> **Source**：[`src/server/mod.rs`](../../../src/server/mod.rs)

## 職責
封裝整個 HTTP 介面：路由組裝、middleware、認證、handler、DTO、錯誤映射、OpenAI 相容層、
greeting 背景任務。是 runtime 核心與 sub-agent 層對外的唯一接觸面。

## 結構

子檔案分工見 [`src/server/mod.rs`](../../../src/server/mod.rs) 的 `//!`（route／handler／openai／dto／auth／error／greeting，加上身份與授權新增的 actor／authz／codes／falcon／identity，共十二個子模組）。本頁只記 doc comment 沒說的部分：

- `dto.rs` 的 `AgentResponse` 隨非串流端點退役一併**移除**（0.4.0 breaking）。
- <a id="auth"></a><a id="greeting"></a>greeting 由開機背景 task 跑兩階段 pipeline（fetcher → analyst）填 `AppState::greetings`。
- <a id="identity"></a>身份與授權的四個新模組：`falcon.rs`（permissions client 與正/負向 cache）、`identity.rs`（middleware）、`actor.rs`（pepper HMAC 假名）、`authz.rs`（三方交集判定）；`codes.rs` 是三種信封共用的穩定 `code` 常數。

## 兩條 agent 執行路徑

| 路徑 | Handler | 編排 |
|---|---|---|
| 原生串流 | `agent_stream` | `plan_stream_turn` prelude → 依 resolved intent 選 pipeline → SSE |
| OpenAI 相容 | `chat_completions` | 同 prelude → buffered / 偽串流封裝 |

**兩者都經過 prelude**，因此本模組不再有繞過 guardrails / audit 的 prompt 入口。
過去的第三類「直接驅動 pipeline」handler（`insight` / `insight_stream` / `report` /
`report_stream`）已於 work item
[`retire-superseded-agent-endpoints`](../../work/retire-superseded-agent-endpoints/prd.md) 移除，
連帶移除 `insight_initial`、`final_answer`、`insight_error_to_app_error`、
handler 層的 `validate_prompt` 與 `USER_PROMPT_LENGTH_CAP`。

共用 helper：`insight_frames`（`AgentEvent` → `StreamFrame`）、`wants_report_pipeline`、
`status_to_app_error`、`with_prefix`、`UnusedAgentPort`、
`INSIGHT_STREAM_BUFFER`。

> 命名註記：`insight_frames` 與 `INSIGHT_STREAM_BUFFER` 保留了 `insight` 字樣，
> 但它們服務的是兩條 pipeline 共用的事件映射與 channel，與已退役的端點無關（work item FU-002）。

## 認證細節

**兩條 auth path**，行為刻意不同：

| Middleware | 套用範圍 | 失敗 status | 失敗 body |
|---|---|---|---|
| `require_bearer` | standard sub-router（4 條，含 `/health`、`/ready`） | `418 I'm a teapot` | 茶壺訊息 + `auth.service_token_invalid` |
| `require_bearer_openai` | `/v1/chat/completions` | `401 Unauthorized` | OpenAI envelope `{"error":{"message","type","code"}}` |

`/v1` 走 401 是為了讓 agentgateway 這類 OpenAI-compatible client 正確辨識認證失敗；
其餘 standard routes 維持既有 418 契約。

共通：scheme 名稱大小寫不敏感（RFC 6750）、token 用 `constant_time_eq` 比對（防 timing attack）。
兩條 prompt route 另需 `X-Falcon-Authorization: Bearer <FALCON_ACCESS_TOKEN>`；身份 middleware
位於 service bearer／global limiter 之後、per-actor limiter 之前，並回傳
`identity.header_missing`、`identity.token_refreshable` / `identity.token_terminal` 或 `identity.upstream_unavailable`。
permissions endpoint 未承諾的 upstream `error_code` 只作為 runtime 內部判定輸入，不轉送給消費端；非 `POST` 直通 method
router，以保留既有 405。

auth layer 套在**各自的 sub-router** 上，scope 明確。但在 `merge` 之後於外層新增 route 會
**同時繞過兩個 auth layer**——新增端點必須有 Router-level auth test。

Kubernetes probe 可配置 headers；實際相容性取決於 deployment profile，repo 無部署檔可判定。

## 錯誤映射

`AppError`（`BadRequest 400` / `BadGateway 502` / `ServiceUnavailable 503`）為 HTTP 對外錯誤；
runtime 內部的 `RuntimeError` 經 `runtime_error_to_app_error` 轉成 `AppError`。
sub-agent 層的 `AgentError` 在 `/agent/stream` 由 `resolve_outcome` 收斂成 SSE `error` frame；
在 `/v1/chat/completions` 收斂成 OpenAI envelope。原本把它映射成 HTTP status 的
`insight_error_to_app_error`（`Capability` → 502、其餘 → 503）隨非串流端點一併移除。

`JsonRejection` 的處理**兩端點群不同**：

- `/agent/stream`：經 `impl From<JsonRejection> for AppError` 一律轉 **400**，
  因此 >64 KiB body 也是 400 而非 413（掩蓋了 extractor 的原始 status）。
- `/v1/chat/completions`：經 `json_rejection_status` 依 extractor 自己的 status
  分流成 413 / 415 / 400。

詳見 [runtime error](./runtime-error.md)。

## Timeout

分兩組，因為 `merge` 會保留各 sub-router 自己的 layer（`route.rs` 有
`per_group_timeout_layers_survive_a_merge` test 固定此行為）：

| Group | 值 | 逾時 body |
|---|---|---|
| standard | 120 s | 空 |
| OpenAI | 600 s | OpenAI envelope（經 `HandleErrorLayer`） |

兩者都只限制 handler 建立 Response 之前，**不限制**已回傳 Response 的 SSE body。
SSE channel 為有界但有損（`try_send`），cancellation gaps 見
[`/agent/stream`](../endpoints/agent-stream.md)。

## 相關
- 每個端點細節 → [端點總覽](../endpoints/index.md)
- sub-agent pipeline → [agent](./agent.md)
- runtime 編排 → [turn](./runtime-turn.md)
- 共享狀態 `AppState` / `AppRuntime` → [專案主體 · 啟動與組裝](../index.md#4-啟動與組裝top-level-接線)
