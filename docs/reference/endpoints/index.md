# HTTP Endpoints — 現況契約

> ← [Reference root](../index.md)  
> **Source**：[`src/server/route.rs`](../../../src/server/route.rs)、[`src/server/auth.rs`](../../../src/server/auth.rs)、[`src/server/handler.rs`](../../../src/server/handler.rs)、[`src/server/dto.rs`](../../../src/server/dto.rs)

## 路由表

| Method | Path | Handler | Bearer | Detail |
|---|---|---|---|---|
| GET | `/health` | `health` | required | [health](./health.md) |
| GET | `/ready` | `ready` | required | [ready](./ready.md) |
| GET | `/greeting` | `greeting` | required | [greeting](./greeting.md) |
| POST | `/agent` | `agent` | required | [agent](./agent.md) |
| POST | `/agent/stream` | `agent_stream` | required | [agent-stream](./agent-stream.md) |

五條 route 都在 `.layer(require_bearer)` 前加入 Router，因此都被該 layer 包住；`auth::check` 沒有 path exemption。在 auth layer 後新增 route 會繞過它，新增端點時必須有 Router-level auth test。

## 認證

- Header：`Authorization: Bearer <GLOBAL_TOKEN>`。
- scheme 名稱接受 case-insensitive `Bearer`。
- token 以 `constant_time_eq` 比對。
- 缺少、格式錯誤或 token 不同：`418 I'm a teapot` + JSON error body。
- 沒有 `WWW-Authenticate` response header。

PRD 的完成樣貌要求標準化 auth/CORS/probe policy；目前行為在決策與 migration 完成前仍是上述契約。

## Middleware 現況

| Layer | Current behavior | Scope caveat |
|---|---|---|
| `TraceLayer` | HTTP tracing | 全 routes |
| `CorsLayer::very_permissive()` | mirror request origin/method/headers 並允許 credentials | 沒有 origin allowlist |
| `CompressionLayer` | response compression | SSE 是否實際壓縮依 body/header semantics |
| `TimeoutLayer` | 120 秒未完成 handler future 時回 504 | 不限制已建立 Response 後的 SSE body |
| response headers | `nosniff`、`no-referrer` | 全 responses through layer |
| `DefaultBodyLimit` | max 64 KiB | oversized JSON 的最終 status 沒有 Router test；`JsonRejection` 目前統一轉 400 |
| bearer middleware | 全五 routes | 失敗 418 |

## Agent 雙路徑

| Contract | Legacy（預設） | Runtime（true/1） |
|---|---|---|
| prompt cap | 2000 chars | config，EV pack 為 4000 chars |
| `/agent` structural error | HTTP 400 | HTTP 400 |
| `/agent/stream` structural error | HTTP 400 before stream | HTTP 200 SSE `error` frame |
| REST intent | `unknown` | Final resolved；Refused/Aborted unknown |
| `intent.resolved` | no | yes，answer token 前 |
| memory/audit/policy | no runtime components | partial runtime components |

`RUNTIME_ENABLED` 只控制 handler branch；AppState startup 仍先載入 runtime config。因此「flag off」不是 runtime config 損壞時的完整 startup rollback。

## Probe 現況

`/health`、`/ready` 目前都要 bearer。Kubernetes HTTP probes可設定 headers，因此不能單憑「有認證」判斷部署不相容；repo 沒有 deployment manifest，實際 probe header 設定未知。Target policy 與決策狀態見 [PRD FR-011](../prd.md)。

## Coverage gaps

目前沒有 Router oneshot suite 固定下列外部契約：

- 五條 route 的 auth scope、418 body/header。
- malformed/missing JSON 與 >64 KiB status。
- legacy/runtime REST/SSE prompt boundaries。
- timeout 與 SSE body lifetime。
- CORS allowlist/credential behavior。

測試計劃見 [QA](../tests/qa-plan.md#8-required-next-tests)。
