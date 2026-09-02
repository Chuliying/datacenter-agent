# Falcon 使用者身份與 RBAC 接進 runtime 請求路徑 技術規格

**Story ID**: S-RUNTIME-SEC-02
**Spec 版本**: v1.6.0
**對應 PRD**: `docs/work/runtime-falcon-identity-rbac/prd.md` @ v2.0.0
**Stage**: approved

## Capability Snapshot

| Capability | Value | Evidence |
|---|---|---|
| `has_ui` | false | manifest `has_ui: false`；本 slice 不新增介面 |
| `has_api` | true | manifest `api_reference: docs/reference/index.md`（本服務對外契約）；新增的**外呼**契約來源為 Falcon 串接指南，見 Contracts 的來源標記與 PRD FU-005 |
| `typed_contracts` | true | manifest `types_entry: src/model.rs`；驗證指令 `cargo check` |
| `has_e2e` | false | manifest `has_e2e: false` |

## Version History / 版本歷史

| Version | Updated at | Change | Impact | PRD version | Author |
|---|---|---|---|---|---|
| v1.0.0 | 2026-08-27 10:30 | 初版 | 9 FR / 10 ERR / 26 AC 的技術方案 | PRD v1.7.0 | Chuliying |
| v1.1.0 | 2026-08-27 12:00 | 依第一輪 Gate 2 對抗式審查修正 16 項 | 移除違反 D3 的 `[identity].enabled` flag；`/v1` 新增 `session_id` 傳輸並接上 server memory；釘定 header 名稱與 `code` 列舉；補 `test_support.rs` 的 runtime-wired fixture 與呼叫計數 spy；釘定收窄範圍（charter 豁免、`"*"` 先展開）與 report 判定 predicate；定義告警通道與缺漏聲明注入點；納入 `/greeting` 殘留說明 | PRD v1.7.0 | Chuliying |
| v1.2.0 | 2026-08-27 14:00 | 依第二輪獨立審查修正 12 項 | 缺漏聲明改注入終端答案（原本掛在被終端 `Clear` 抹掉的 transient prefix，SSE 側 AC-009 無法成立）；`authz.insufficient` 新增拒答載體（三種帶 code 的信封都不在 200 出現）；`auth.rs` 補進 Files 與 S5（ERR-001 的 code 產生點）；report turn 記憶過濾改為要求權限全涵蓋；新增 S11a 身份載體接線與 S19 本機開發拓樸；identity middleware 明訂非 POST 直通；S17 補多輪 eval 模式；S10 補腳本化 LLM stub | PRD v1.7.0 | Chuliying |
| v1.3.1 | 2026-08-27 17:30 | 依 QA plan 審查修正測試落點 | S16 從外部 `tests/identity_contract.rs` 改為 crate 內 `#[cfg(test)]` 模組：`test_support` 是 `#[cfg(test)] pub(crate)`（`src/lib.rs:27-28`），外部 test crate 不可達，原落點會讓 30 條 L4 測試全部編不過 | PRD v1.8.0 | Chuliying |
| v1.3.0 | 2026-08-27 15:00 | 依使用者範圍縮減決定移出兩項 | 移出非逐字 summary 邊界（S14 縮減、S17 刪除）與 `/v1` 的 `session_id` 傳輸與 memory 接線（S12 刪除）；保留 report 降級產出。21 步 37h → 19 步 32h | PRD v1.8.0 | Chuliying |
| v1.6.0 | 2026-09-02 16:30 | 對齊 PRD v2.0.0 與交付後審查發現 | 補上實作已落地但 spec 漏記的部分：`PermissionsFailure` 為五個 variant、ERR-010（`identity.upstream_conflict`）回到 Errors 表（v1.4.0 的「移除 ERR-010」已被 v1.7.0 的 Falcon 維運回覆推翻）、S16 的 router 層測試實際落在 `route.rs` 的 `#[cfg(test)]` 而非新檔；新增 D-018（insight 上界必須涵蓋嚴格 intent，開機檢查）與 D-019（外呼不跟隨 redirect） | PRD v2.0.0 | Chuliying |
| v1.5.0 | 2026-08-31 09:30 | 依 dev 環境實測修訂 401 分類契約 | 指南三個版本皆無七列 `auth.*` 表（已查證），但免憑證探測證實端點確實回 `error_code`；D-016 由 D-017 取代，改為白名單分類（僅 `auth.token_invalid` 可續期，未知與缺漏皆終端）；`identity.token_refreshable` / `identity.token_terminal` 拆為 `identity.token_refreshable` 與 `identity.token_terminal`；ERR-003 拆出 ERR-008 | PRD v1.9.0 | Chuliying |
| v1.4.0 | 2026-08-28 09:00 | 依使用者指定的 Falcon 指南 commit 重訂外呼失敗契約 | 採用指南明載的 Bearer transport、`200` response shape 與 permissions `401`；移除未被該 endpoint 承諾的七列 `auth.*`、ERR-008、ERR-010 與 upstream code parsing；`roles` 改為物件 shape；S0 改為已完成的來源核對 | PRD v1.9.0 | Chuliying |

## Preflight 記錄

| 檢查 | 結果 |
|---|---|
| Knowledge Boundary | 跳過：manifest 無 `knowledge_boundary` 欄位 |
| Follow-ups 阻擋掃描 | 無阻擋型待確認 FU。PRD v1.9.0 的 FU-001/002/003/006/007 為 Non-blocking，FU-004 已併入 FU-007，FU-005 已依指定指南 commit 關閉 |
| Project Manifest | 已讀 |
| System Context | 已讀 `.agent/knowledge/system-context.md` |
| System Map | 同上；發現三處過時（八條路由、`no rate-limit middleware`、缺 `/v1/chat/completions`），經使用者選擇後就地修正並通過 `lint-docs.sh` |
| API Reference | 已讀 `docs/reference/index.md`（0.4.0、5 條端點） |
| Environment Rules | manifest 無該欄位；改讀 `.agent/guardrails.md`：secret 只從環境變數載入、不 log token、**AppState 是唯一共享狀態**、不改 probe 回應格式 |
| PRD 機器檢查 | `check-prd.py --tier team` → PASS（fails=0） |
| Base 分支差異 | `git rev-list --left-right --count HEAD...origin/main` → `0 11`。未整合，工作樹未改動 |
| UI Token / Mockup | N/A (has_ui=false) |

## D3 與 feature flag

身份層在本 slice 交付的 binary 中**無條件生效**，沒有任何啟用開關。這直接來自 D3（全面 fail-closed、無例外），也是 AC-015 只能是部署前一次性檢查的原因：交付後的 binary 沒有能重建「未含身份層」前提的開關。

- 三階段部署的向前相容由**舊 binary** 提供（它忽略未知 header），不由新 binary 的 flag 提供。
- 測試以 `PermissionsProvider` stub 與測試用 pepper 取得可控性，不以 production flag 取得。
- 本機開發需要一個 stub permissions 端點與 `ACTOR_KEY_PEPPER`；拓樸與腳本由 S19 交付（S10 的 in-process trait stub 只覆蓋測試，不覆蓋 `cargo run`）。

## Files

| Path | Operation | Purpose |
|---|---|---|
| `src/server/identity.rs` | NEW | 身份 middleware：取 header、呼叫 provider、推導 `actor_key`、寫入 request extensions；ERR-002/003/004 的狀態與 code 決定點 |
| `src/server/falcon.rs` | NEW | `PermissionsProvider` trait + Falcon HTTP 實作 + 正/負向 cache；依指南的 `200` response 與 generic `401` 解析 |
| `src/server/actor.rs` | NEW | `actor_key` 推導與 pepper 載入 |
| `src/server/authz.rs` | NEW | 三方交集、default-deny、report 例外、`"*"` 展開 |
| `src/server/codes.rs` | NEW | 釘定的 `code` 列舉常數，三種信封共用 |
| `src/server/rate_limit.rs` | MODIFY | 內層 per-actor keyed bucket（上界 + LRU 淘汰）；audit 標明拒絕層 |
| `src/server/route.rs` | MODIFY | identity layer 置於 bearer 之內、外層限流之後、內層限流之前；**只掛在兩條 prompt 路由**，不掛 standard family 根部 |
| `src/server/auth.rs` | MODIFY | `require_bearer` 的 418 teapot body 與 `openai_unauthorized` 的 401 body 是 ERR-001 的**產生點**，需帶上 `auth.service_token_invalid` |
| `src/server/error.rs` | MODIFY | `ErrorBody` 加法式新增 `code` |
| `src/server/openai.rs` | MODIFY | `OpenAiErrorBody` 新增 `code` 與 `x_refusal_code`；`map_request` 只取最後一則 user message（不新增 `session_id`，見 PRD FU-006）|
| `src/server/dto.rs` | MODIFY | `StreamFrame::Error` 新增 `code`；`AgentRequest.history` 被忽略 |
| `src/server/handler.rs` | MODIFY | 兩 handler 取身份；四個 pipeline 建構點改傳收窄後 grant；不呼叫 `fold_history_into_prompt`；缺漏聲明注入終端答案 |
| `src/server/greeting.rs` | MODIFY | 僅補文件註解：說明其 boot-time 全量 grant 與身份層的關係（見 Errors 的殘留列） |
| `src/server/mod.rs` | MODIFY | 匯出新模組 |
| `src/config.rs` | MODIFY | 新增 `[identity]`（**不含 enabled**）、`[server.rate_limit.per_actor]`、`[authz.permission_tools]`、`[authz.intent_tools]`、`[report.grants]`；boot 驗證映射表全覆蓋與 `[report.grants]` 對 advertised MCP 集合 |
| `src/appstate.rs` | MODIFY | 載入 pepper（缺失即啟動失敗）、建構並持有 `Arc<dyn PermissionsProvider>`、外層 limiter 必啟用檢查 |
| `src/runtime/audit.rs` | MODIFY | 新增獨立 opaque actor 欄位；新增 `PermissionDegraded` 與 `IdentityAlarm` 事件 |
| `src/runtime/memory/context.rs` | MODIFY | 逐 turn 依當次權限過濾；回報略過筆數 |
| `src/runtime/turn.rs` | MODIFY | **`AgentTurnInput` 新增 `identity: Option<IdentityContext>` 作為身份載體**；`apply_memory_context`（`turn.rs:449`）與 `append_memory_turn_if_enabled`（`turn.rs:521`）目前寫死 `actor_id: None`（`turn.rs:462`、`535`）且 `build_session_memory_context` 不收身份或權限輸入——三者簽章都要改；`actor_id` 填實值。`user_summary` 與 `answer_summary` 維持現況（summary 邊界移出本刀，見 PRD FU-007）|
| `src/test_support.rs` | MODIFY | runtime-wired `app_state()` 變體、`PermissionsProvider` stub、LLM 與 MCP 呼叫計數 spy、測試 pepper |
| `config/config.toml` | MODIFY | 新增上述 config 段；`[report.grants]` 含六個 tool |
| `config/prompt_guide/fetcher_system.md` | MODIFY | 移除寫死的六個 tool 名稱，改 grant-aware 敘述 |
| `.env.example` | MODIFY | 新增 `ACTOR_KEY_PEPPER`、`FALCON_API_BASE_URL` |
| `docs/reference/endpoints/agent-stream.md` | MODIFY | 新 header、`code` 列舉、三種身份失敗語義 |
| `docs/reference/endpoints/chat-completions.md` | MODIFY | 同上，另更新 History 折入章節：明載授權模式下只採最後一則 `user` message、為單輪、**無 `session_id`**（PRD FU-006）|
| `docs/work/runtime-falcon-identity-rbac/handoff-consumers.md` | NEW | C1–C9（九項）、permissions `401` 與 runtime internal code 的處置矩陣、三階段部署順序 |
| `src/server/route.rs`（原規劃為 `src/server/identity_contract_tests.rs` NEW） | MODIFY | router 層整合測試，以 `#[cfg(test)]` 模組掛在 crate 內。**不可放 `tests/`**：`src/lib.rs:27-28` 是 `#[cfg(test)] pub(crate) mod test_support;`，外部 test crate 編譯 lib 時沒有 `cfg(test)`、只看得到 `pub` 項目，因此 `test_support` 與 `rate_limit.rs` 的 seam helper 從 `tests/` 完全不可達。實作時併入 `route.rs` 既有的 `#[cfg(test)]` 模組（`identity_layer_only_covers_prompt_routes…`、`ss_chat_route_requires_identity…`），不另開檔 |

## Contracts

### 使用者身份 header（釘定）

| 項目 | 值 |
|---|---|
| Header 名 | `X-Falcon-Authorization` |
| 值格式 | `Bearer <使用者的 Falcon access token>` |
| 為何不用 `Authorization` | 該 header 已被服務層的 `GLOBAL_TOKEN` 佔用（`src/server/auth.rs`），兩者必須並存 |
| 可否設定 | **否**。名稱是公開契約，寫死為常數；設定化會讓契約隨環境而異 |

### `code` 列舉（釘定，三種信封共用）

| `code` | HTTP | ERR | 消費端處置 |
|---|---|---|---|
| `auth.service_token_invalid` | 418 / 401 | ERR-001 | 修正 service token；**不**續期 |
| `identity.header_missing` | 401 | ERR-002 | 補 header；**不**續期 |
| `identity.token_refreshable` | 401 | ERR-003 | 最多 silent-refresh 後重送**一次**；再失敗即終端 |
| `identity.token_terminal` | 401 | ERR-008 | 終端。重新登入或管理員處理；**不得**進 refresh 路徑 |
| `identity.upstream_unavailable` | 503 | ERR-004 | 依退避重試 |
| `authz.insufficient` | 200 | ERR-005 | 終端，顯示拒答內容 |
| `rate_limit.global` | 429 | ERR-006 外層 | 依 `Retry-After` 退避 |
| `rate_limit.actor` | 429 | ERR-006 內層 | 同上 |

ERR-007（pepper）與 ERR-009（limiter 未啟用）是啟動失敗，無 HTTP 表示，不在列舉內。列舉常數集中於 `src/server/codes.rs`，新增值需同步兩份 endpoint 文件。Falcon permissions endpoint 的 upstream `error_code` 不屬於本列舉；只有本服務 internal `code` 對消費端公開。

**`authz.insufficient` 的載體（釘定）**：ERR-005 是 200 拒答，SSE 為 `Token` + `Done`（`Refused` 形狀，`handler.rs:252-262`）、`/v1` 為正常 `chat.completion`，三種帶 `code` 的錯誤信封在 200 都不會出現。因此需要獨立載體，否則 FR-008 要求的「至少要能區分權限不足（終端）」無處落地：

| 路徑 | 載體 |
|---|---|
| `/agent/stream` | `Done` 之前新增一個帶 `code` 的 `StreamFrame::Refusal { code }` 變體（加法式；既有消費端忽略未知 `event` 值） |
| `/v1/chat/completions` | 在 `chat.completion` 物件上新增非標準欄位 `x_refusal_code`；OpenAI client 對多餘回應欄位一般容忍，且既有欄位不變 |

兩者的 `code` 皆為 `authz.insufficient`。

### Data Contracts (`typed_contracts: true`)

| Contract | Source | Shape / fields | Evidence |
|---|---|---|---|
| `ActorKey` | `src/server/actor.rs` NEW | newtype over `String`；`"v1:" + base64url(HMAC-SHA256(pepper, "falcon-user:" + user_id))` 取前 32 字元 | `cargo check`；已知答案測試 |
| `FalconPermissionsResponse` | `src/server/falcon.rs` NEW | `{ user_id: i64, roles: Vec<FalconRole>, permissions: Vec<PermissionItem> }`；對齊指南第 2 節的 200 response | `cargo check`；指定版指南 |
| `FalconRole` | `src/server/falcon.rs` NEW | `{ id, code, name, default_pages: Vec<String> }` | `cargo check`；指定版指南 |
| `Permissions` | `src/server/falcon.rs` NEW | `{ user_id: i64, codes: HashSet<String> }`；只收 `can_read == true` 的 effective permissions | `cargo check` |
| `PermissionItem` | `src/server/falcon.rs` NEW | `{ code, name, category, page_path: Option<String>, can_read, can_write }` | `cargo check`；指定版指南 |
| `IdentityContext` | `src/server/identity.rs` NEW | `{ actor_key: ActorKey, permissions: Permissions }`；經 request extensions 傳遞 | `cargo check` |
| `PermissionsFailure` | `src/server/falcon.rs` NEW | 五個 variant：`UnauthorizedRefreshable`（401 `auth.token_invalid`）、`UnauthorizedTerminal`（其餘 401，含未知與缺漏）、`UnauthorizedPlatformCanary`（401 `auth.invalid_platform`：對呼叫端終端、對我們是告警）、`UpstreamConflict`（400 `auth.conflicting_credentials`，ERR-010）、`Unavailable`（transport、5xx、畸形 200）。不攜帶 upstream code 原文 | `cargo check` |
| `ErrorBody` / `OpenAiErrorBody` / `StreamFrame::Error` | `error.rs` / `openai.rs` / `dto.rs` MODIFY | 各新增 `code`，既有欄位不變 | `cargo check`；序列化測試 |
| `AuditCtx` / `AuditRecord` | `src/runtime/audit.rs` MODIFY | 新增獨立 opaque actor 欄位；**不**併入 `AuditActor { ip, user_agent }` | `cargo check` |

### API Contract (`has_api: true`)

**外呼**（新增；來源為指定版 [`External_Delegated_API_Integration.md`](https://github.com/HDRenewables/falcon-backend/blob/c5d0408b5e1847bf165e8c5f5859ef61da93e02d/docs/External_Delegated_API_Integration.md)，commit `c5d0408b5e1847bf165e8c5f5859ef61da93e02d`）：

| Operation | Request | Success response | Error response | Auth / permission |
|---|---|---|---|---|
| `GET {FALCON_API_BASE_URL}/api/auth/me/permissions` | header `Authorization: Bearer <使用者 token>`；無 body | `200` `{ "user_id": 123, "roles": [{ "id": 10, "code": "viewer", "name": "檢視者", "default_pages": ["/dashboard/"] }], "permissions": [{ "code": "admin:permissions", "name": "權限管理", "category": "admin", "page_path": "/admin/permissions", "can_read": true, "can_write": false }] }` | `401`：指南只說 access token 無效、過期或撤銷時回 401；未承諾 permissions endpoint 的 `error_code` 表。transport／5xx／畸形 200 由 runtime 歸為不可用 | 權限完全依被代理使用者既有 RBAC；runtime 不持有 external client API key，不呼叫 `/external-login` 或 `/external-refresh` |

**對內**（加法式變更）：

| Operation | Request | Success response | Error response | Auth / permission |
|---|---|---|---|---|
| `POST /agent/stream` | 既有 body + `X-Falcon-Authorization`；`history` 被忽略 | 不變（SSE） | 信封新增 `code` | service bearer + 使用者身份 header |
| `POST /v1/chat/completions` | 同上；只採最後一則 `user` message。**不新增 `session_id`；授權模式下為單輪**（PRD FU-006）| 不變 | OpenAI 信封新增 `code`；拒答加 `x_refusal_code` | 同上 |

**契約來源標記（v1.5.0 修訂）**：指南**不是**本項的唯一 SSOT，因為它落後於實作。

已獨立查證：`External_Delegated_API_Integration.md` 在 `c5d0408`、分支 `EM-241`、分支
`ST-7178-alembic-merge` 三處皆為 175 行、`auth.missing_token` 出現次數為 0，且該檔在
default branch `temp_solution` 上已不存在。其第 2 節只寫「access token 無效、過期或撤銷時
回 401」。所以「指南第 2 節有一張七列表格」在任何版本都不成立。

但對 dev 環境的免憑證探測（2026-08-31，`https://dev.hdre-eomc.com/api/auth/me/permissions`，
唯讀、不帶真實憑證、不改變狀態）證明該端點**確實**回傳 `error_code`：

| 探測 | 回應 |
|---|---|
| 不帶 token | `401` `{"detail": "未登入", "error_code": "auth.missing_token"}` |
| 垃圾 Bearer | `401` `{"detail": "Token 無效或已過期", "error_code": "auth.token_invalid"}` |
| Cookie + Bearer | `401` `auth.token_invalid`（兩者皆無效時先判 token） |
| 只帶 Cookie | `401` `auth.token_invalid`（cookie transport 亦被接受） |

因此：2 個 code 已實測逐字確認；其餘 5 個（`auth.conflicting_credentials`、
`auth.token_revoked`、`auth.user_not_found`、`auth.user_inactive`、`auth.invalid_platform`）
來自後端團隊轉述，需要伺服端狀態、無法自行製造。**指南應補上這張表**，這是要回報後端的項目。


### UI Design

N/A (has_ui=false)

## Flow / Data Flow

```text
request
  -> service bearer gate            EXISTING require_bearer / require_bearer_openai
  -> 外層全域 burst limiter          EXISTING，位置不變：身份解析之前
  -> identity middleware            NEW src/server/identity.rs，只掛兩條 prompt 路由
       -> X-Falcon-Authorization 缺 => ERR-002 identity.header_missing
       -> cache 查詢（正向 / 負向，key = token hash）
       -> miss => GET /api/auth/me/permissions
            200                          => Permissions
            401（invalid／expired／revoked）=> ERR-003 identity.token_refreshable / identity.token_terminal
                                            （依 error_code 白名單分類）
            逾時 / 5xx / 畸形 200         => ERR-004 identity.upstream_unavailable
       -> actor_key = HMAC(pepper, "falcon-user:" + user_id)
       -> req.extensions_mut().insert(IdentityContext)
  -> 內層 per-actor limiter          MODIFY rate_limit.rs => ERR-006 rate_limit.actor
  -> JSON extraction                 EXISTING
  -> handler
       -> 忽略 AgentRequest.history；/v1 不呼叫 fold_history_into_prompt
       -> prelude（intent 解析）      EXISTING plan_stream_turn
       -> 判定 predicate：
            wants_report_pipeline(normalized) 為真 => report 閘門與 report grant 上界
            否則                                   => 該 intent 的 required tools 嚴格判定
       -> effective = expand(boot data grant) ∩ permission grants ∩ intent required
            空 => ERR-005 authz.insufficient（200 拒答，不呼叫 LLM / MCP）
            report 分支：非空即降級，缺漏主題併入終端答案聲明（非 transient prefix，見下方注入點）
       -> build_{insight,report}_pipeline(effective, charter_grant, ...)
            charter grant 不參與收窄（emit_chart 為 code-backed，非資料 tool）
       -> memory 讀取：逐 turn 依當次權限過濾
       -> memory 寫入：user_summary / answer_summary 維持現況（summary 邊界移出，PRD FU-007）
                       scope.actor_id = actor_key
```

**API-to-UI transformer**: N/A (has_ui=false)

### 收窄範圍（釘定）

| 對象 | 是否收窄 | 理由 |
|---|---|---|
| fetcher 資料 grant（`[insight.grants].fetcher`） | 是 | 這是 MCP 資料 tool 的上界 |
| report 資料 grant（`[report.grants]`，六個） | 是 | 同上，report 路徑專用 |
| charter grant（`["emit_chart"]`，config.toml:94） | **否** | `emit_chart` 是 code-backed 的結構化輸出 tool，不取資料；無任何 permission code 對應它，納入交集會使所有使用者的圖表功能失效 |
| composer grant（`emit_report`） | 否 | 同上 |

`"*"` 的處理：fetcher 的預設 grant 是 `["*"]`（`src/config.rs` 的 `default_fetcher_grant`），字串層次與 `"*"` 取交集無意義。交集前先以既有的 `expand_grant`（`src/agent/wiring.rs:484`）對 advertised MCP tool 集合展開，再與 permission grants 取交集。該函式目前是**私有**（`fn expand_grant`，無 `pub`），S9 需一併改為 crate 可見。

### 缺漏聲明的注入點（釘定）

**不可**掛在既有的 answer-policy `prefix` 上。`prefix` 在 SSE 是 transient：`AgentEvent::Finished` 展開為 `[Clear, Token{assistant}, Done]`（`handler.rs:1155-1158`），終端 `Clear` 會抹掉先前送出的 prefix token——`handler.rs:340-341` 的註解明文如此。把聲明放在那裡，SSE 端的最終畫面不會有它，而 `/v1` 的 `with_prefix` 卻會保留，兩條路徑因此不一致，AC-009 在 SSE 側無法成立。

釘定作法：聲明併入**終端答案本身**，因此必然存活於 `Clear` 之後。

| 路徑 | 注入點 |
|---|---|
| `/agent/stream` | 併入 `Clear` 之後的終端 `StreamFrame::Token`（即 `AgentEvent::Finished` 的 `assistant` 內容前綴），不另發 transient token |
| `/v1/chat/completions` | 沿用 `with_prefix`（`handler.rs:470`）前置到回答 |

兩者都寫一筆 `AuditEvent::PermissionDegraded { omitted_topics }`。S11 的驗證斷言**最終 frame 的內容**，而非中間 token。

### 告警通道（釘定）

「告警」= `tracing::error!` 加一筆 `AuditEvent::IdentityAlarm { kind }`，經既有 `AuditSink` 輸出，與限流拒絕同一條路徑。身份 middleware 不因 permissions endpoint 未承諾的 upstream `error_code` 產生特定告警；若未來指南新增需要告警的語義，必須另開契約變更。事件不含 token、prompt、response 或 IP。

## Errors / Boundaries

| Case | Trigger | Expected behavior | Recovery |
|---|---|---|---|
| 正常 | 身份解析成功、判定通過 | 進編排；memory 以 `actor_key` 隔離讀寫 | N/A |
| ERR-001 | `GLOBAL_TOKEN` 缺失或不符 | 沿用既有：standard 418、`/v1` 401 + OpenAI 信封，加 `auth.service_token_invalid`。不呼叫 Falcon | 修正 service token；不續期 |
| ERR-002 | bearer 有效但無 `X-Falcon-Authorization` | 401 + `identity.header_missing`。不寫 memory、不呼叫 LLM/MCP | 補 header；不續期 |
| ERR-003 | 上游 401 且 `error_code == auth.token_invalid` | 401 + `identity.token_refreshable`。寫負向 cache | 前端 silent-refresh 後重送**一次**；再失敗即終端 |
| ERR-008 | 上游 401 且為其他任何 `error_code`，**含未知或缺漏** | 401 + `identity.token_terminal`。寫負向 cache | 無可由續期完成的復原路徑；重新登入或管理員處理 |
| ERR-004 | 逾時、連線失敗、5xx，或 200 但 body 無法解析 | 503 + `identity.upstream_unavailable`。不降級為匿名、不用逾期 cache。寫負向 cache | 依退避重試；恢復延遲上界 10 秒 |
| ERR-005 | `effective` 為空 | 200 + `authz.insufficient` 拒答，指出缺哪一類權限。不呼叫 LLM/MCP、不寫 memory | 申請權限；生效延遲上界 60 秒 |
| ERR-006 | 外層全域超限或內層 per-actor 超限 | 429 + 整數 `Retry-After` + `Cache-Control: no-store` + per-family 信封 + `rate_limit.global` 或 `rate_limit.actor`。恰好一筆標明拒絕層的 audit event；外層拒絕**無** `actor_key` | 依 `Retry-After` 退避 |
| ERR-007 | 啟動時 `ACTOR_KEY_PEPPER` 缺失、為空或 < 32 bytes | 啟動失敗；錯誤指出變數名與長度下限，不含 pepper 內容 | 補設定後重啟 |
| ERR-010 | 上游 400 且 `error_code == auth.conflicting_credentials` | 500 + `identity.upstream_conflict` + 告警。**不**寫 cache——這是 runtime 請求建構錯誤，不是這個 token 的屬性，快取它會在修好後仍重放舊判定 | 修正 runtime 外呼；使用者無需動作 |
| ERR-011 | 啟動時某個嚴格 intent 的 required tools 不被 `[insight.grants].fetcher` 涵蓋 | 啟動失敗，錯誤指出 intent 與 tool 名。理由見 D-018 | 補 `[insight.grants].fetcher` 或改 `[authz.intent_tools]` |
| ERR-009 | 啟動時 `[server.rate_limit].enabled` 為 false | 啟動失敗。身份層無條件生效，因此外層 limiter 是**必要**組態而非可選 | 啟用 `[server.rate_limit]` 後重啟 |
| 邊界：cache 命中 | 同一 token hash 在 TTL 內重複請求 | 正向命中不打 Falcon；負向命中重放**原失敗類別**（同 status、同 code），不打 Falcon。正、負向 cache 都需有條目數上界與 LRU 淘汰——與 per-actor bucket 同理，僅靠外層 limiter 間接約束不足，換 token 灌可推高條目數 | N/A |
| 邊界：父層權限 | 只有事業部父層權限 | `effective` 為空 → ERR-005 | 申請子頁權限 |
| 邊界：未映射項目 | 未知 intent、未映射的 permission code 或 tool wire name | default-deny；映射表未全覆蓋為**啟動**失敗 | 補齊 config |
| 邊界：非 POST | 非 POST 進入受限路由 | 不消耗任一層允入容量，**且不進行身份解析**：identity middleware 比照 `enforce`（`rate_limit.rs:89-91`）對非 POST 直通，讓 method router 的 405 契約不變。否則 authed `GET /agent/stream` 會回 401 而非 405，直接打破既有測試 `wrong_method_requests_do_not_consume_capacity`（`rate_limit.rs:302-324`） | N/A |
| 邊界：report intent 的記憶過濾 | 已存的 `report` intent turn 遇上部分權限使用者 | **要求全涵蓋才保留**：report turn 的 required 是六個 tool，權限未全涵蓋即整筆略過。理由：該 turn 的 `answer_summary` 可能含寫入時較寬權限下取得的全部主題內容，「交集非空即保留」會讓被撤銷主題的內容透過舊 turn 持續進入脈絡，直接違反 FR-009 的「撤銷即生效」。代價是剛拿到降級報告的人下一輪追問時該筆不在脈絡內——但使用者自己的 UI 歷史仍看得到 | N/A |
| 殘留：`/greeting` | `build_one_greeting`（`src/server/greeting.rs:36-65`）在 **boot 時**以完整的 `[insight.grants].fetcher` 產生問候語，之後對任何持有 service bearer 者供應 | PRD 明列 `/greeting` 不受本刀影響，故不改行為。但這是一條**不經身份層**的殘留揭露路徑：只有財務權限的使用者仍會讀到由會員/營運資料衍生的問候語。identity layer 因此**只掛在兩條 prompt 路由**，不掛 standard family 根部——掛錯會讓 `/health`、`/ready` 一併需要身份 header | 若要收斂，屬另一個 slice；此處僅記錄 |

## Decisions

| ID | Decision | Rationale | Rejected | Reversibility |
|---|---|---|---|---|
| D-001 | 收窄改在 `handler.rs` 的四個 pipeline 建構點傳入收窄後的 `&[String]` | pipeline 本就 per-request 建構（`handler.rs:277`/`292`/`536`/`550`），傳入的就是 wire-name slice | 以 `ToolRegistry::resolve` 為 seam：`.resolve(` 在 `src/` 內零呼叫者，`tools.rs:30` 自述未接線 | 改回傳原 grant |
| D-002 | `PermissionsProvider` 為 trait object，HTTP 實作與測試 stub 並存，由 `AppState` 持有 | 與既有 `dyn AuditSink`、`dyn SessionMemoryStore` 同構；guardrails 要求 AppState 為唯一共享狀態 | 引入 `wiremock`：新增依賴且拖慢測試 | 移除 trait |
| D-003 | identity middleware 在 bearer 之內、外層限流之後、內層限流之前 | 外層留在身份解析之前才擋得住 ERR-002/003/004/008 這類在內層之前就結束的流量 | 兩層都移到身份之後：該類流量完全不受限流約束 | layer 順序可調 |
| D-004 | 外層 limiter 必須啟用，以**開機檢查**強制（ERR-009） | 只寫文件的部署前提在誤部署下無效；與 pepper 缺失即失敗一致 | 只在 runbook 寫 | 移除檢查 |
| ~~D-005~~ | **Superseded by D-015（v1.3.0 範圍縮減）**。原內容：`user_summary` 由 canonical(intent, slots) 組成 | `sanitize_field` 對短、無敏感樣式的提問是恆等函數，前移不構成非逐字邊界；`sanitize.rs:5-6` 自述該邊界屬 FU-006。槽位值取自封閉設定詞彙表（`slots.rs`），自由文字結構上無法進入 | 只把 `sanitize_field` 前移 | 改回寫 `raw_input` |
| ~~D-006~~ | **Superseded by D-015（v1.3.0 範圍縮減）**。原內容：`answer_summary` 只導入 `sanitize_field` | 非逐字化回答會使多輪追問近乎失效；今天該欄位是 `response.to_string()`、零處理 | 兩側都非逐字 | 移除呼叫 |
| D-007 | `actor_key` 進 audit 用新增的獨立欄位 | `AuditActor` 只有 ip / user_agent，且 AC-013 禁止 audit 帶 IP | 塞進 `AuditActor` | 移除欄位 |
| D-008 | 三種信封的 `code` 為加法式新增，常數集中於 `codes.rs` | 缺 header 與 upstream permissions 401 的 HTTP status 都是 401，沒有 internal `code` 就無法分流；既有消費端只讀 `error` / `data`，行為不變 | 改寫既有欄位語義 | 移除欄位 |
| D-009 | `/v1` 授權模式下只採最後一則 `user` message，不折入 | 僅忽略 `history` 欄位無效——`map_request` 先映射成 history，`handler.rs` 再折回 `prompt` 與 `raw_input` | 只忽略 `history` 欄位 | 恢復折入 |
| D-010 | report 取得獨立 grant 上界（六個 tool） | `[insight.grants].fetcher` 當時只有五個且註解排除 `bill_member_analysis`，而 `fetcher_system.md:20` 要求六個——第六個從未 advertise，是既存 bug。**v1.6.0 補述**：只補 report 這一側並不夠，見 D-018 | 沿用五個 | 改回共用 |
| **D-011** | **身份層無條件生效，不提供任何啟用開關** | D3 明訂全面 fail-closed、無 feature flag。一個能讓交付後 binary 退回 legacy 模式的開關會使 AC-015 的「不可重跑」理由失效，也讓預設組態可能靜默關掉 RBAC。三階段部署的向前相容由舊 binary 提供 | `[identity].enabled` 設定開關（v1.0.0 曾誤採） | 需修改 D3 並取得使用者明確簽核 |
| **D-012** | **`/v1` 在授權模式下為單輪；不新增 `session_id`、不接線 memory** | 2026-08-27 範圍縮減。`/agent/stream` 是原始需求的入口；agentgateway 的多輪不是。重要的是**不能**在 handoff 叫 agentgateway「改送 `session_id`」——`map_request` 寫死 `session_id: None`（`openai.rs:204`）、`handler.rs:583` 註明 memory inert，送了也無效 | 本刀新增 `session_id` 傳輸並接線 memory（v1.1.0 曾採） | 見 PRD FU-006 |
| **D-013** | **收窄只作用於 fetcher / report 資料 grant；charter 與 composer 豁免；`"*"` 先以 `expand_grant` 展開再取交集** | `emit_chart` / `emit_report` 是 code-backed 輸出 tool，無 permission code 對應，納入交集會讓所有使用者失去圖表與報告產出；`default_fetcher_grant` 是 `["*"]`，字串層次取交集無意義 | 對所有 grant 一律取交集 | 調整收窄集合 |
| **D-015** | **summary 邊界移出本刀：`user_summary` 與 `answer_summary` 維持現況** | 2026-08-27 範圍縮減，優先交付身份與授權本體。**但本刀把 `actor_key` 接上 memory，因此逐字內容從「落在不可歸戶的 anonymous 桶」變成「可歸戶到假名 actor」——這是本刀自身造成的隱私回歸**，必須在 SQLite session repository 接線（持久化）之前關閉 | 保留原 FR-007 | 見 PRD FU-007 |
| **D-014** | **授權判定與 report grant 上界都以 `wants_report_pipeline` 為 predicate** | 該函式（`handler.rs:129-135`）以「report 為 top **或** candidate」路由，因此「營收報告」的 top intent 是 `revenue` 而仍走 report pipeline。若閘門改用 `intent == report`，這類請求會被 revenue 的嚴格判定擋下，與 FR-004 承諾的降級產出矛盾 | 閘門用 `intent == report` | 改回嚴格比較 |
| ~~D-016~~ | **Superseded by D-017（v1.5.0）**。原內容：只以 200 shape 與 generic 401 分類，依 upstream `error_code` 白名單分類（僅 `auth.token_invalid` 可續期）。該決定對指南判讀正確——指南確實沒有那張表——但指南落後於實作 |
| **D-017** | **依 `error_code` 區分可續期與終端，採白名單：僅 `auth.token_invalid` 為可續期，其餘一切（含未知與缺漏）為終端** | 論據不是「指南寫了」而是「API 實際提供了這個資訊，不用它就會做出無限 refresh 迴圈」：七種失敗中只有一種能靠 refresh 解決，收斂成單一 code 會讓前端對停用帳號一路重試。白名單而非黑名單，是因為這些 code **未被任何版本的指南承諾**，隨時可能新增或改名；最壞情況因此是多一次重新登入，而不是迴圈 | 黑名單（列舉終端 code，其餘視為可續期）：未知 code 會落進可續期，正好是要防的那一側 | 改回單一 `Unauthorized` 變體即可 |
| **D-018** | **`[insight.grants].fetcher` 必須涵蓋每個嚴格 intent 的 required tools，並以開機檢查強制（ERR-011）** | D-010 只把第六個 tool 補進 report 上界，`[authz.intent_tools].member` 卻同時要求 `member_analysis` 與 `bill_member_analysis`。非 report 路徑的規則是 `omitted_tools.is_empty()`，所以少一個 tool 不是「收窄」而是**對每一位使用者的永久拒答**，而且外顯成「權限不足」——與真正的權限問題無從分辨。兩張表各自合法、合起來不成立，正是開機該擋的形狀 | 只在 review 時人工比對兩張表 | 移除 `validate_intent_reachability` |
| **D-019** | **外呼 Falcon 的 HTTP client 設 `redirect::Policy::none()`，3xx 歸為 `Unavailable`** | reqwest 預設跟隨最多 10 跳，且**同 origin** 的轉址會保留 `Authorization` header——Falcon 前方任何一層回 302，就會把使用者的 bearer 送到一個沒有被審查過的 URL。PRD 安全欄明文承諾「程式層不存在把該 token 轉發到其他 URL 的路徑」，預設 policy 讓那句話不成立 | 依賴 Falcon 不會轉址 | 改回預設 policy |

## Steps

| Step | Files | Action | Depends on | Estimate | Verification |
|---|---|---|---|---|---|
| S0 | — | 前置核對：讀取指定 Falcon 指南 commit，確認 permissions endpoint 的 Bearer transport、200 response shape 與 401 語義；確認它沒有逐列 upstream `error_code` 契約，並將 `/external-login`／`/external-refresh` 的 `external_auth.*` 排除 | none | 30min | 指定版指南逐段人工比對；結果寫回 PRD/spec，S3 依 generic 401 實作 |
| S1 | `src/config.rs` MODIFY、`config/config.toml` MODIFY、`.env.example` MODIFY | 新增 `[identity]`（base URL、正向 TTL 60s、負向 TTL 10s、逾時 5s；**無 enabled**）、`[server.rate_limit.per_actor]`、`[authz.permission_tools]`、`[authz.intent_tools]`、`[report.grants]`。boot 驗證：兩張映射表全覆蓋 `intent_allowlist`、`[report.grants]` 的每個 wire name 都在 advertised MCP 集合內 | none | 1h | `cargo check`；缺項 config 解析失敗測試；`[report.grants]` 含不存在 tool 時啟動失敗 |
| S2 | `src/server/actor.rs` NEW、`src/appstate.rs` MODIFY | `ActorKey` 推導與 pepper 載入；缺失/空/<32 bytes 即啟動失敗，錯誤不含 pepper 內容 | S1 | 45min | 已知答案 HMAC 單元測試（AC-004）；pepper 缺失啟動失敗（AC-014） |
| S3 | `src/server/falcon.rs` NEW、`src/server/codes.rs` NEW | `PermissionsProvider` trait、HTTP 實作、指南 200 response 與 generic 401 解析、正/負向 cache（key = token hash，value 不含 token，負向項記錄 internal failure 類別）、`code` 常數集 | S0,S1 | 2.5h | 200 shape／401／transport failure 單元測試；cache 命中不外呼（AC-003/020/025） |
| S4 | `src/server/identity.rs` NEW、`src/server/mod.rs` MODIFY | identity middleware；**非 POST 直通**（比照 `enforce`）；ERR-002/003/004 的狀態與 code 決定點 | S2,S3 | 1.5h | router 層：缺 header、upstream 401、upstream unavailable 回不同 code（AC-010）；既有 `wrong_method_requests_do_not_consume_capacity` 仍過 |
| S5 | `src/server/error.rs`、`src/server/openai.rs`、`src/server/dto.rs`、`src/server/auth.rs` MODIFY | 三信封加法式新增 `code`；`StreamFrame` 新增 `Refusal { code }`；`chat.completion` 新增 `x_refusal_code`；**`auth.rs` 的兩個 body 產生點帶上 `auth.service_token_invalid`**（只改信封 struct 不會讓該 code 上線） | S3 | 1.5h | 序列化測試確認 `error` / `data` 欄位不變；ERR-001 的 418 與 401 都帶 code |
| S6 | `src/server/route.rs` MODIFY | identity layer 掛在**兩條 prompt 路由**、bearer 之內、外層限流之後 | S4 | 45min | `tests/route_contract.rs` 仍過；新增斷言：不帶身份 header 時 `/health`、`/ready`、`/greeting` 仍為 200，兩條 prompt 路由為 401（僅斷言「非 404」不足以抓到掛錯層） |
| S7 | `src/server/rate_limit.rs`、`src/appstate.rs`、`src/server/route.rs` MODIFY | 內層 per-actor keyed bucket（上界 + LRU 淘汰）；**在兩條 prompt 路由掛載內層 middleware**（S6 只掛 identity）；audit 標明拒絕層；外層 limiter 必啟用的開機檢查 | S4,S6,S10 | 2.5h | AC-012、AC-018、AC-021（AC-012/018 需兩個相異 actor，故相依 S10 的 provider stub） |
| S8 | `src/runtime/audit.rs` MODIFY | 獨立 opaque actor 欄位；新增 `PermissionDegraded`、`IdentityAlarm` 事件；tracing / stdout sink 同步 | S2 | 1h | AC-013：序列化後的 record 帶 `actor_key` 且不含 IP、user agent、session id、token、prompt、response |
| S9 | `src/server/authz.rs` NEW、`src/agent/wiring.rs` MODIFY | 三方交集、default-deny、`"*"` 經 `expand_grant` 展開（該函式需改為 crate 可見）、charter/composer 豁免、report predicate 用 `wants_report_pipeline` | S1 | 2h | AC-007、AC-008、AC-022；charter grant 不被清空的迴歸測試 |
| S10 | `src/test_support.rs` MODIFY | runtime-wired `app_state()` 變體（現有版本是 `runtime: None`，`test_support.rs:142`）、`PermissionsProvider` stub、LLM 與 MCP 呼叫計數 spy、測試 pepper，**以及一個腳本化的 chat-completions stub**：LLM 是經 `async_openai` 以 `base_url` 連出（`src/agent/llm.rs:241-249`），而 AC-009（降級報告含缺漏聲明）與 AC-005/AC-023 在 `/agent/stream` 上的兩次請求記憶驗證都需要 pipeline **跑完**，也就是要能腳本化驅動 fetcher 的 `tool_calls` 與 schema 合法的 `emit_report` payload。零呼叫類的否定斷言只需 spy，正向路徑則需要這個 stub | S2,S3 | 4h | 以既有 `rate_limit.rs` 測試模式驗證 fixture 可跑通一條 prompt 路由並完成終端答案 |
| S11 | `src/server/handler.rs`、`src/server/greeting.rs` MODIFY | 兩 handler 取身份；四個 pipeline 建構點改傳收窄後 grant；report 改用 `[report.grants]`；不呼叫 `fold_history_into_prompt`；忽略 `history`；缺漏聲明併入**終端答案**（SSE 併入 `Clear` 之後的終端 Token；`/v1` 用 `with_prefix`），不得掛在會被終端 `Clear` 抹掉的 transient prefix；greeting 補註解 | S4,S9,S10 | 2.5h | AC-006、AC-009、AC-016、AC-024 |
| S11a | `src/runtime/turn.rs`、`src/server/handler.rs` MODIFY | **身份載體接線**：`AgentTurnInput` 新增 `identity` 欄位，`apply_memory_context`、`append_memory_turn_if_enabled` 與 `plan_stream_turn` 簽章隨之調整，兩個 handler 呼叫點（含 `handler.rs:385` 的直接 append）帶入 `IdentityContext`。這是 S13 與 S14 的前置——沒有載體，權限過濾與 `actor_id` 填值都無從取得輸入 | S4,S11 | 1.5h | `cargo check`；`actor_id` 不再為 `None` 的單元斷言 |
| S13 | `src/runtime/memory/context.rs`、`src/runtime/turn.rs` MODIFY | 逐 turn 依當次權限過濾；`unknown` 或缺 intent 一律略過；**`report` intent turn 要求權限全涵蓋才保留**；略過筆數進 audit，內容不進 | S9,S11a | 1.5h | AC-023；report turn 在部分權限下被略過的斷言 |
| S14 | `src/runtime/turn.rs` MODIFY | `scope.actor_id` 填實值（`turn.rs:462`、`535` 目前寫死 `None`）。**不動 `user_summary` 與 `answer_summary`**——summary 邊界已移出本刀，見 PRD FU-007 | S1,S2,S11a | 0.5h | AC-005 |
| S15 | `config/prompt_guide/fetcher_system.md` MODIFY | 移除寫死的六個 tool 名稱，改 grant-aware 敘述 | S11 | 20min | 人工 read-through；report 降級測試不再要求呼叫未授權 tool |
| S16 | `src/server/identity_contract_tests.rs` NEW、`src/server/mod.rs` MODIFY | router 層整合測試，覆蓋 SEAM-001 承擔的 AC 與 ERR-004/005/006/010。以 crate 內 `#[cfg(test)]` 模組實作，沿用 `rate_limit.rs` 既有的 `app_state()` → `build_router` → `oneshot` 模式 | S1–S11a, S13–S15 | 4h | `cargo test` 全綠 |
| S19 | `scripts/dev-falcon-stub.md` NEW、`.env.example` MODIFY | 本機開發拓樸：一個可設定 base URL 的 mock permissions 端點（最小 HTTP stub 的啟動說明與範例回應）、必要環境變數清單（pepper、`FALCON_API_BASE_URL`、啟用 `[server.rate_limit]`）。PRD FR-006 boundary 明列此為 spec 需設計的項目，且身份層無條件生效後 `cargo run` 硬性需要它。另記錄 pepper 輪替的 runbook 步驟（PRD 風險表要求） | S1,S2 | 1.5h | 依文件可在本機成功啟動並完成一次授權請求 |
| S18 | `docs/reference/endpoints/agent-stream.md`、`docs/reference/endpoints/chat-completions.md` MODIFY、`docs/work/runtime-falcon-identity-rbac/handoff-consumers.md` NEW | 公開契約更新（header 名、`code` 列舉、三種身份失敗、`/v1` 的 history 章節與單輪聲明）+ 消費端 handoff | S5,S11 | 2h | `lint-docs.sh` |

**總估時**：約 32 小時（19 個步驟：S0–S11、S11a、S13–S16、S18、S19；S12 與 S17 已隨範圍縮減移除，編號保留空號以免既有引用失效）。

### 追溯缺口：降級報告的數字洩漏檢查

PRD FR-004 的 boundary 要求檢查「降級後的報告不得包含任何無權限主題的數字」。這是自由生成內容的性質，
無法以契約測試判定；原本規劃由 eval suite 承接，但範圍縮減已移除唯一的 eval 步驟（S17）。
本 spec 因此不提供實作層承接點，該檢查落在 QA plan 的人工或探索式項目，與 AC-015 的處理方式相同。
PRD FR-004 已同步標註。

### 本刀不做（2026-08-27 範圍縮減）

| 移出項 | 原步驟 | 去向 |
|---|---|---|
| 非逐字 summary 邊界（原 PRD FR-007） | S14 的一半、S17 全部 | PRD FU-007。**含本刀造成的隱私回歸**：`actor_key` 接上 memory 後，逐字提問與回答變成可歸戶到假名 actor。必須在持久化 session memory 前關閉 |
| `/v1` 的 `session_id` 傳輸與 memory 接線 | S12 全部 | PRD FU-006。該端點仍 fail-closed、仍只採最後一則 `user` message，但授權模式下為單輪 |

**保留**：FR-004 的 report 權限降級產出（使用者明示「降級先讓他上線」），因此缺漏聲明注入點、`PermissionDegraded` audit 與 S10 的腳本化 LLM stub 都留在本刀。

## Selected Seam

| Field | Decision |
|---|---|
| Seam ID | SEAM-001 |
| Selected boundary | Integration — axum router 邊界（`build_router(state)` + `tower::ServiceExt::oneshot`），搭配 runtime-wired fixture 與 `PermissionsProvider` stub。測試置於 **crate 內** cfg-test 模組，非外部 tests crate——`test_support` 在 `src/lib.rs:27-28` 宣告為 cfg-test 且 crate-private，外部 test crate 看不到它 |
| Repository evidence | `src/server/rate_limit.rs` 的 cfg-test 模組已建立此 harness：`crate::test_support::app_state()` → 覆寫 state → `build_router(state)` → `oneshot`，含 `authed()`、`body_json()`、`assert_429_headers()` 與 `TEST_TOKEN`。**現有 fixture 是 `runtime: None`（`test_support.rs:142`），本 slice 大多數 AC 需要 runtime 已接線的變體，該擴充由 S10 交付並列入 Files** |
| Lower-seam rationale | 本 slice 的行為幾乎都是跨層的：middleware 順序（bearer → 外層限流 → 身份 → 內層限流 → extractor）、狀態碼與信封 `code`、以及「未呼叫 LLM/MCP」這類否定性斷言，只有在完整 router 上才成立。單元測試無法證明 layer 順序，也無法證明 ERR-002 發生在內層限流之前 |
| Residual lower-level checks | 單元層保留三項獨立風險：`actor_key` 的已知答案 HMAC（S2）、指南 200 shape／generic 401 分類與 cache key（S3）、三方交集與 default-deny 含 `"*"` 展開（S9）。皆為純函式，router 層測會放大成本而不增證據 |
| Reliability and execution cost | 穩定：provider 與 LLM 皆為 stub 故無真實網路；限流測試沿用既有的長 refill 週期手法使桶在測試內不回補，無時鐘等待。**成本分兩級**：否定斷言類（未呼叫 LLM/MCP、狀態碼與 code、layer 順序）與既有 `rate_limit.rs` 測試同級；AC-009 與 AC-005/AC-023 的兩次請求記憶驗證需要腳本化 chat-completions stub 驅動完整 pipeline，成本較高，已反映在 S10 的 4h 與 S16 的 4h。全部在 `cargo test` 內完成，無額外環境負擔 |

## Test Strategy

| Level | Scope | Command / evidence | Applicability |
|---|---|---|---|
| Unit | AC-004、AC-014、AC-007；指南 200 shape／generic 401 分類與 cache key；`"*"` 展開與 charter 豁免；信封序列化 | `cargo test` | required |
| Integration | SEAM-001：AC-001/002/003/005/006/008/009/010/011/012/013/016/018/020/021/022/023/024/025/026 | `cargo test`（crate 內 cfg-test） | required（`has_api: true`，身份層是外部服務契約邊界） |
| Component | N/A | N/A | N/A (has_ui=false) |
| E2E | N/A | N/A | N/A (has_e2e=false) |

### ERR 追溯

| ERR | 承接層級 | 證據 |
|---|---|---|
| ERR-001 | Integration | 既有 bearer 測試 + 新增 `code` 斷言 |
| ERR-002 / ERR-003 | Integration | S16：缺 header 與 permissions 401 的 `code` 互異（AC-010） |
| ERR-004 | Integration | S16：provider stub 回逾時 → 503 且未降級為匿名（AC-011） |
| ERR-005 | Integration | S16：AC-006、AC-008、AC-022 |
| ERR-006 | Integration | S16：AC-012、AC-018、AC-013 |
| ERR-007 | Unit | S2：pepper 缺失啟動失敗（AC-014） |
| ERR-009 | Unit | S7：limiter 未啟用啟動失敗（AC-021） |

### AC 追溯缺口

AC-015（補 header 對未含身份層的 binary 向前相容）是**部署前一次性檢查**，對象是當前 main 的 binary，不是交付後可重跑的迴歸測試——D3 不設 flag，交付後的 binary 一律強制（見 D-011）。QA plan 需以部署檢查而非測試案例承接。

## References

| Document | Path |
|---|---|
| PRD | `docs/work/runtime-falcon-identity-rbac/prd.md` @ v1.9.0 |
| Brainstorm | `docs/work/runtime-falcon-identity-rbac/brainstorm.md` |
| Architecture map | `.agent/knowledge/system-context.md`（本次已修正三處過時） |
| API reference | `docs/reference/index.md`；端點契約 `docs/reference/endpoints/` |
| Guardrails | `.agent/guardrails.md` |
| 前一刀 PRD | `docs/work/runtime-user-session-rate-limit/prd.md` |
| Design tokens | N/A (has_ui=false) |

## Gate 2 自檢

- [x] Capability Snapshot 與 manifest 一致。
- [x] Files 覆蓋每個 FR，並含 v1.1.0 新增的 `test_support.rs`、`codes.rs`、`greeting.rs`。
- [x] Contracts 已填：header 名與 internal `code` 列舉皆已釘定為常數而非設定；API Contract 以指定版 Falcon 指南作為外呼來源，明載 200 shape、generic 401 與 `external_auth.*` 的端點邊界；UI Design 標 N/A。
- [x] Flow / Data Flow、Errors（ERR-001..009 + 六項邊界 + `/greeting` 殘留；ERR-008/010 已由 v1.9.0 移除）、Decisions（D-001..016，其中 D-005、D-006 已由 D-015 取代）、Steps（S0–S11、S11a、S13–S16、S18、S19）完整。
- [x] Selected Seam 有 repository evidence、fixture 缺口的明確補救步驟（S10）、lower-seam rationale、residual checks 與成本判斷。
- [x] Test Strategy trace PRD AC **與 ERR**，capability false 項目為 N/A，並標出 AC-015 的追溯缺口與承接方式。
- [x] Gate 2 evidence 可被 QA / implement 下游讀取。
