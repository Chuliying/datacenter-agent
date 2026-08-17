# HTTP Endpoints — 現況契約

> ← [Reference root](../index.md)  
> **Source**：[`src/server/route.rs`](../../../src/server/route.rs)、[`src/server/auth.rs`](../../../src/server/auth.rs)、[`src/server/handler.rs`](../../../src/server/handler.rs)、[`src/server/dto.rs`](../../../src/server/dto.rs)、[`src/server/openai.rs`](../../../src/server/openai.rs)

## 路由表

Router 由兩個 sub-router `merge` 而成，各自帶自己的 timeout 與 auth layer。

### Standard group（8 條，120 s timeout，`require_bearer` → 418）

| Method | Path | Handler | Runtime turn | Detail |
|---|---|---|---|---|
| GET | `/health` | `health` | — | [health](./health.md) |
| GET | `/ready` | `ready` | — | [ready](./ready.md) |
| GET | `/greeting` | `greeting` | — | [greeting](./greeting.md) |
| POST | `/insight` | `insight` | 繞過 | [insight](./insight.md) |
| POST | `/insight/stream` | `insight_stream` | 繞過 | [insight-stream](./insight-stream.md) |
| POST | `/report` | `report` | 繞過 | [report](./report.md) |
| POST | `/report/stream` | `report_stream` | 繞過 | [report-stream](./report-stream.md) |
| POST | `/agent/stream` | `agent_stream` | **經過** | [agent-stream](./agent-stream.md) |

### OpenAI group（1 條，600 s timeout，`require_bearer_openai` → 401）

| Method | Path | Handler | Runtime turn | Detail |
|---|---|---|---|---|
| POST | `/v1/chat/completions` | `chat_completions` | **經過**（prelude） | [chat-completions](./chat-completions.md) |

> **沒有 `POST /agent`。** 該路由已被 `/insight` 取代（commit `ea2bcef`）。舊的 `agent.md`
> 已移除；歷史契約見 git 歷史。

兩個 group 的 auth layer 都套在各自 sub-router 上，因此 scope 是明確的。在 `merge` 之後、
於外層新增 route 會**同時繞過兩個 auth layer**；新增端點時必須有 Router-level auth test。

## 認證

兩條 auth path，行為刻意不同：

| Group | Middleware | 失敗 status | 失敗 body |
|---|---|---|---|
| Standard（8 條） | `require_bearer` | `418 I'm a teapot` | 專案 JSON error body |
| `/v1/chat/completions` | `require_bearer_openai` | `401 Unauthorized` | OpenAI error envelope `{"error":{"message","type"}}` |

`/v1` 走 401 是為了讓 agentgateway 這類 OpenAI-compatible client 能正確辨識認證失敗；
其餘 8 條維持既有的 418 契約（見 agentgateway spec D6）。

共通行為：

- Header：`Authorization: Bearer <GLOBAL_TOKEN>`。
- scheme 名稱接受 case-insensitive `Bearer`。
- token 以 `constant_time_eq` 比對。
- 沒有 `WWW-Authenticate` response header。

## Middleware 現況

### 共用 stack（`shared`，套在兩個 group 外）

| Layer | Current behavior | Scope caveat |
|---|---|---|
| `TraceLayer` | HTTP tracing | 全 routes |
| `CorsLayer::very_permissive()` | mirror request origin/method/headers 並允許 credentials | 沒有 origin allowlist |
| `CompressionLayer` | response compression | SSE 是否實際壓縮依 body/header semantics |
| `SetResponseHeaderLayer` | `nosniff`、`no-referrer` | 全 responses through layer |
| `DefaultBodyLimit` | max 64 KiB | oversized JSON 的最終 status 沒有 Router test；`JsonRejection` 目前統一轉 400 |

### 分組 timeout

| Group | Layer | 逾時行為 |
|---|---|---|
| Standard | `tower_http` `TimeoutLayer::with_status_code` | 120 s → `504`，空 body |
| OpenAI | `tower` `TimeoutLayer` + `HandleErrorLayer` | 600 s → `504` + OpenAI error envelope（非空 body） |

`/v1/chat/completions` 的 600 s 是必要的：非串流路徑要等**整條** sub-agent pipeline 跑完才能回
`chat.completion`，經常超過 120 s。串流路徑很快就交出 SSE handle，因此兩者都不會被 request
timeout 砍掉。

兩個 group 各自帶 timeout 而非共用，是因為 `merge` 會保留各 sub-router 自己的 layer；
`route.rs` 有 `per_group_timeout_layers_survive_a_merge` test 固定這個行為。

## 三種 agent 執行路徑

這是目前最容易誤解的地方——**三條路徑的編排層級不同**：

| 路徑 | 端點 | 編排 | guardrails / intent / memory / audit |
|---|---|---|---|
| 直接 pipeline | `/insight`、`/insight/stream`、`/report`、`/report/stream` | `build_insight_pipeline` / `build_report_pipeline` 直接跑 `Orchestrator` | **全部繞過** |
| Runtime 路由 | `/agent/stream` | `plan_stream_turn` prelude → 依 resolved intent 選 insight 或 report pipeline | 經過 |
| OpenAI 相容 | `/v1/chat/completions` | 同 prelude，再走 buffered / 偽串流封裝 | 經過 |

直接 pipeline 端點把 `AgentResponse.intent` 硬寫成 `"unknown"`（沒有 intent 分類可用）。
把 pipeline 收到 runtime `AgentPort` 之後是 plan §9 的工作，目前**尚未**進行。

`/agent/stream` 需要 runtime 啟用（`RUNTIME_ENABLED`，預設 on）；rollback 時回 `503`，
呼叫端應改用 `/insight/stream` 或 `/report/stream`。

## Prompt cap 現況

| 路徑 | Cap | 位置 |
|---|---|---|
| `/insight`、`/report` 及其 stream | 2 000 chars | handler `validate_prompt`（`USER_PROMPT_LENGTH_CAP`） |
| Runtime prelude（`/agent/stream`、`/v1`） | config `thresholds.input.max_prompt_chars`，目前 4 000 | runtime prelude |

`/v1/chat/completions` 會先把 `messages` 的 history 折入 prompt **再**過 prelude，因此折入後的
長度一併受 4 000 cap 約束（見 [chat-completions](./chat-completions.md)）。

## Probe 現況

`/health`、`/ready` 目前都要 bearer。Kubernetes HTTP probes 可設定 headers，因此不能單憑
「有認證」判斷部署不相容；repo 沒有 deployment manifest，實際 probe header 設定未知。
Target policy 與決策狀態見 [PRD FR-011](../prd.md)。

## Coverage gaps

目前沒有 Router oneshot suite 固定下列外部契約：

- 8 條 standard route 的 auth scope、418 body/header，以及 `/v1` 的 401 envelope。
- malformed/missing JSON 與 >64 KiB status。
- 直接 pipeline 端點與 runtime 路徑的 prompt boundary 差異（2 000 vs 4 000）。
- timeout 與 SSE body lifetime（120 s / 600 s 兩組）。
- CORS allowlist/credential behavior。

測試計劃見 [QA](../tests/qa-plan.md#8-required-next-tests)。
