# Brainstorm — Falcon 使用者身份與 RBAC 接進 runtime

Mode: Standard（3 questions, budget spent）
Date: 2026-08-26

## 問題

runtime 的 SQLite ledger、session memory 與 rate limit 全部以 `actor_key` 為軸心設計，
但 HTTP 路徑從來沒有產生過它；編排也不看使用者權限。這正是
`runtime-user-session-rate-limit` PRD 延後的 "request-path integration slice"。

## Repo evidence

| 位置 | 現況 |
|---|---|
| `src/server/auth.rs` | 只有單一 `GLOBAL_TOKEN` bearer；runtime 沒有使用者身份概念 |
| `src/runtime/store/sqlite.rs` | `users` / `sessions` / `monthly_budgets` / `budget_reservations` 均以 `actor_key` 為鍵 |
| grep `actor_key` in `src/` | 生產路徑零產生者；只有 `tests/runtime_store_sqlite.rs` 在傳 |
| `src/runtime/turn.rs:462,535` | `SessionMemoryScope.actor_id` 一律 `None` → memory 落在共用 `"anonymous:<session_id>"` |
| `src/server/rate_limit.rs:112` | process-local 全域 bucket；`AuditCtx.actor: None` |
| PRD FU-004 | 明訂 trusted actor contract 出現前不做 per-actor 限流，且拒絕信任 forwarding header |
| falcon-client `agent-runtime/identity.ts` | 已有 `resolveActor()`（打 `/api/auth/me`、60s cache）產出 `{userId, role, authStatus}` |
| falcon-client `agent-client.ts` | 下游只送 `prompt` / `history` / `session_id` / `option_id`——actor 未傳出 |
| `src/agent/tools.rs:219` | `ToolRegistry::resolve(grants)` 是權限 seam，但 `[insight.grants]` 為 boot-time 靜態 |
| `DATACENTER_MCP_URL` | 單一 MCP 連線，無 per-user 憑證 |
| falcon-client `getMenuPermissions.ts` | `PermissionItem { code, name, category, page_path, can_read, can_write }`——功能/頁面級，無 row/案場 scope |

## 決定

| # | 決定 | 理由 |
|---|---|---|
| D1 | BFF 把使用者 Falcon `access_token` 往下傳；runtime 呼叫 `GET /api/auth/me/permissions` 自驗，取得 `user_id` + `permissions[]`（短 TTL cache） | 權限來自權威來源且即時；`GLOBAL_TOKEN` 外洩不再等於可冒用任何使用者。runtime 不需要 external client API key |
| D2 | 權限在進 LLM 之前生效：per-request 收窄 `ToolRegistry` grants；intent 落在無權限範圍直接拒答 | fail-closed 在最前面，不燒 token、不打 MCP |
| D3 | 全面 fail-closed，無例外：解析不到身份即 401，不寫 memory、不記 ledger、不呼叫 LLM | 不留繞過額度的旁路 |
| D4 | Falcon permission code → MCP endpoint 一張顯式映射表（PRD 需 domain 確認） | Falcon RBAC 與 tool 開關同為功能級，粒度對得上 |
| D5 | `actor_key = "v1:" + base64url(HMAC-SHA256(pepper, "falcon-user:" + user_id))[..32]`；pepper 由 `ACTOR_KEY_PEPPER` 提供、≥32 bytes、boot 缺失即 fail-closed；日常不輪替，視為 break-glass | `user_id` 是小整數，無 pepper 的純 hash 可彩虹表反推。版本前綴置於 HMAC 之外，輪替後新舊資料自然分家 |

## In scope

- 新的使用者身份 header 契約（`Authorization` 已被 `GLOBAL_TOKEN` 佔用）
- `/api/auth/me/permissions` client + 短 TTL cache（key 為 token hash，不存 token）
- `actor_key` 假名化並貫穿 session memory scope、monthly ledger、audit
- per-request grant 收窄與 intent 閘門；Falcon permission code → MCP endpoint 映射表
- per-actor rate limit（把 FU-004 從全域桶升級）
- `/agent/stream` 與 `/v1/chat/completions` 兩條路徑行為一致
- 消費端 handoff 文件（falcon-client BFF + agentgateway）

## Out of scope

- 資料層 RBAC（MCP server 端的 per-user 憑證）
- token refresh（留在 BFF）
- multi-replica ledger（FU-002，Platform 持有）
- 案場/row 級權限（Falcon RBAC 本身無此概念）

## 風險與待解

- **R1 消費端順序**：補 header 是向前相容的（runtime 忽略未知 header），所以不需要 runtime feature flag，只需部署順序：消費端補 → 觀測確認流量都帶了 → runtime 開 fail-closed。agentgateway `/v1/chat/completions` 目前完全無使用者身份，是它的新需求。
- **R2 殘餘缺口**：功能級隔離由編排層完整覆蓋；唯一殘餘是繞過 runtime 直接打 `DATACENTER_MCP_URL`，屬網路邊界（MCP 不對外）而非編排問題。
- **R3 confused deputy**：使用者 token 的作用域是該使用者在 Falcon 的全部權限（含 `can_write`），不只 permissions 端點。約束需入 spec：只打該單一端點、程式層不存在轉發其他 Falcon URL 的路徑、驗證後即丟、不落 log / audit / SQLite。
- **R4 兩層 token 的 401 歸屬**：`GLOBAL_TOKEN` 失效與使用者 token 過期必須以不同 error code 區分，BFF 才知道要不要 refresh 重送。
- **R5 pepper 輪替代價**：actor_key 一變，進行中 session 會 `OwnershipConflict` 當場斷線。輪替 runbook 必須含「清 `sessions` 表或 BFF 同步換 `session_id`」，並選在月初與 Asia/Taipei 月度重置對齊；舊 `v1:` ledger 原地保留當稽核證據，不做資料遷移。

## Handoff

team-sprint profile → `prd-interview`。PRD 訪談需確認 D4 映射表的 endpoint 歸屬。
