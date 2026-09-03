# HTTP Endpoints — 現況契約

> ← [Reference root](../index.md)  
> **Source**：[`src/server/route.rs`](../../../src/server/route.rs)、[`src/server/auth.rs`](../../../src/server/auth.rs)、[`src/server/handler.rs`](../../../src/server/handler.rs)、[`src/server/dto.rs`](../../../src/server/dto.rs)、[`src/server/openai.rs`](../../../src/server/openai.rs)

## 路由表

Router 由兩個 sub-router `merge` 而成，各自帶自己的 timeout 與 auth layer。**共 6 條。**

### Standard group（5 條，120 s timeout，`require_bearer` → 418）

| Method | Path | Handler | Runtime prelude | Detail |
|---|---|---|---|---|
| GET | `/health` | `health` | — | [health](./health.md) |
| GET | `/ready` | `ready` | — | [ready](./ready.md) |
| GET | `/greeting` | `greeting` | — | [greeting](./greeting.md) |
| POST | `/agent/stream` | `agent_stream` | ✅ | [agent-stream](./agent-stream.md) |
| POST | `/ss-chat/stream` | `ss_chat_stream` | ✅（answer policy 覆寫） | [ss-chat-stream](./ss-chat-stream.md) |

### OpenAI group（1 條，600 s timeout，`require_bearer_openai` → 401）

| Method | Path | Handler | Runtime prelude | Detail |
|---|---|---|---|---|
| POST | `/v1/chat/completions` | `chat_completions` | ✅ | [chat-completions](./chat-completions.md) |

> **已退役的端點**：`POST /agent`（`ea2bcef`）、`POST /insight`、`POST /insight/stream`、
> `POST /report`、`POST /report/stream`（work item
> [`retire-superseded-agent-endpoints`](../../work/retire-superseded-agent-endpoints/prd.md)）。
> 這些路徑現在回 `404`，且不因 `Authorization` 正確與否而不同，不洩漏 token 有效性。
>
> **這由外層的顯式 fallback（`route.rs` 的 `unmatched_path`）保證，2026-08-19 才補上。** 在那之前
> 這句敘述是錯的：`Router::merge` 會把 sub-router 的 fallback 一併帶過來，於是未命中路徑落在
> OpenAI group 的 `require_bearer_openai` **之後**——不帶 token 回 `401`、帶正確 token 回 `404`，
> 任何路徑都成了「這個 token 有效嗎」的探測器。現由
> `route.rs` 的 `retired_paths_return_404_regardless_of_authorization` 與
> `unmatched_paths_do_not_leak_token_validity` 兩個 Router 層測試釘住。
>
> 遷移路徑：串流用 `/agent/stream`（依 intent 自動路由 insight / report pipeline）；
> 非串流用 `/v1/chat/completions`。

兩個 group 的 auth layer 都套在各自 sub-router 上，因此 scope 是明確的。在 `merge` 之後、
於外層新增 route 會**同時繞過兩個 auth layer**；新增端點時必須有 Router-level auth test。
（外層的 `fallback` 是刻意的例外——它必須在 auth 之外，未命中路徑才不會先被認證擋下而洩漏
token 有效性；它不服務任何真實端點。）

## 認證

兩條 auth path，行為刻意不同：

| Group | Middleware | 失敗 status | 失敗 body |
|---|---|---|---|
| Standard（5 條） | `require_bearer` | `418 I'm a teapot` | 專案 JSON error body |
| `/v1/chat/completions` | `require_bearer_openai` | `401 Unauthorized` | OpenAI error envelope `{"error":{"message","type"}}` |

`/v1` 走 401 是為了讓 agentgateway 這類 OpenAI-compatible client 能正確辨識認證失敗；
standard group 維持既有的 418 契約（見 agentgateway spec D6）。

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
| `DefaultBodyLimit` | max 64 KiB | oversized JSON 的最終 status 沒有 Router test |

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

## 三條 agent 執行路徑

三者都經過 runtime prelude（`plan_stream_turn`：guardrails → intent → answer policy → memory
→ audit），差別在對外形狀、pipeline 選擇方式，以及 answer policy：

| 路徑 | 端點 | Pipeline 選擇 | 對外形狀 |
|---|---|---|---|
| 原生串流（EOMC） | `/agent/stream` | 依 resolved intent 路由（`wants_report_pipeline`） | 專案 SSE frame（含 `stage` / `usage` / `intent.resolved`） |
| 原生串流（星星電力） | `/ss-chat/stream` | 固定 SS chat pipeline，不做 intent routing | 同上（同一段 `run_chat_stream`） |
| OpenAI 相容 | `/v1/chat/completions` | 同 `/agent/stream` | `chat.completion` 或偽串流 `chat.completion.chunk` |

`/ss-chat/stream` 是唯一覆寫 answer policy 的端點：它用 `AlwaysAnswerPolicy` 取代配置的
`RuleAnswerPolicy`，因為 runtime 的 intent pack 是 EV 充電領域的，SS 問題會被當成 `off_scope`
拒答。prompt injection 拒答仍然保留，prelude 其餘部分不變。詳見
[ss-chat-stream](./ss-chat-stream.md#與-agentstream-的兩點差異)。

> 過去存在第三類「直接驅動 pipeline、繞過 runtime」的端點（`/insight`、`/report` 系列），
> 已於本次退役。**現在所有接受 user prompt 的端點都經過 prelude**，不存在無防護入口。

三者都需要 runtime 啟用（`RUNTIME_ENABLED`，預設 on）；rollback 時回 `503`。
`RUNTIME_ENABLED=false` 下只有 `/health`、`/ready`、`/greeting` 可用——該 flag 已不具備
「切換到無 runtime 的替代路徑」的意義，存廢見 work item 的 FU-003。

## Prompt cap 現況

單一來源：runtime config `thresholds.input.max_prompt_chars`，目前 **4 000** chars，
由 prelude 的 `runtime::guardrails::input_guard::validate_prompt` 執行。
handler 層過去的 2 000-char cap（`USER_PROMPT_LENGTH_CAP`）已隨退役端點一併移除。

`/v1/chat/completions` 只取 `messages` 尾端最後一個 `user` 的文字，再送入 prelude；更早的
transcript 不會折入，因此只選中的文字受 4 000 cap 約束（見
[chat-completions](./chat-completions.md)）。

## Probe 現況

`/health`、`/ready` 目前都要 bearer。Kubernetes HTTP probes 可設定 headers，因此不能單憑
「有認證」判斷部署不相容；repo 沒有 deployment manifest，實際 probe header 設定未知。
Target policy 與決策狀態見 [PRD FR-011](../prd.md)。

## Coverage gaps

2026-08-19 起 `src/test_support.rs` 提供 Router 層 fixture（記憶體內 stub MCP server，`AppState`
不再需要 live 連線），已用它固定「未命中路徑一律 404 且不因 Authorization 而異」。仍未固定：

- 5 條 standard route 的 auth scope、418 body/header，以及 `/v1` 的 401 envelope。
- malformed/missing JSON 與 >64 KiB status。
- timeout 與 SSE body lifetime（120 s / 600 s 兩組）。
- CORS allowlist/credential behavior。

測試計劃見 [QA](../tests/qa-plan.md#8-required-next-tests)。
