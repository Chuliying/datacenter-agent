# Plan: runtime-falcon-identity-rbac

## Plan

- Format: canonical-v2
- Concurrency: 1
- Intent: 把 Falcon 使用者身份與 RBAC 接進 runtime 請求路徑，使 rate limit、session memory 與編排都以已驗證的使用者為依據。
- Scope: spec v1.4.0 的 S0–S19（S12、S17 已隨範圍縮減移除）。身份解析、actor_key、雙層限流、權限收窄編排、記憶脈絡過濾、錯誤碼契約、消費端 handoff 與本機拓樸。
- Non-goals: 非逐字 summary 邊界（PRD FU-007）；/v1 的多輪脈絡（PRD FU-006）；資料層 RBAC（FU-002）；消費端 repo 的實際改動（僅交付 handoff 文件）。

## Sources

- local: ../prd.md
- local: ../spec.md
- local: ../qa-plan.md
- user: prd-v1.8.0-approved-and-spec-v1.3.1-approved-2026-08-27
- user: scope-reduction-keep-report-degradation-2026-08-27
- user: falcon-guide-ssot-generic-permissions-401-2026-08-28
- external: https://github.com/HDRenewables/falcon-backend/blob/c5d0408b5e1847bf165e8c5f5859ef61da93e02d/docs/External_Delegated_API_Integration.md

## Tasks

### T01 | S0 Falcon permissions 契約核對

- Status: done
- Depends On: []

#### Intent

讀取指定版 Falcon 串接指南，核對 permissions endpoint 的 Bearer transport、200 response shape、401 語義，以及 external-login／external-refresh 的錯誤表不可套用到 permissions endpoint。
#### Expected Result

permissions endpoint 的 Bearer transport、`200` response shape 與 invalid／expired／revoked token 的 `401` 均與指定版指南一致；指南未承諾該 endpoint 的逐列 `error_code` 表，且 `external_auth.*` 僅屬其他兩支端點。
#### Definition of Done

- Bearer header 與 endpoint path 逐段比對通過
- `200` response 的 `user_id`、role object 與 permission fields 逐段比對通過
- permissions endpoint 的 `401` 語義逐段比對通過
- `external_auth.*` 的 endpoint 邊界已確認，未混入 permissions 契約
- 契約決策已寫回 PRD FR-001、spec Contracts 與 D-016

#### Verification

- 指定版指南人工比對

#### Verification Evidence

- PASS | external:https://github.com/HDRenewables/falcon-backend/blob/c5d0408b5e1847bf165e8c5f5859ef61da93e02d/docs/External_Delegated_API_Integration.md | 指定版指南人工比對

### T02 | S1 config 與開機驗證

- Status: done
- Depends On: []

#### Intent

新增 identity、per-actor 限流、兩張授權映射表與 report grant 的設定段，並加上開機驗證。
#### Expected Result

config 可解析；映射表未全覆蓋或 report grant 引用不存在的 tool 時啟動失敗。
#### Definition of Done

- `[identity]` 段（base URL、正向 TTL 60s、負向 TTL 10s、逾時 5s；無 enabled 開關）
- `[server.rate_limit.per_actor]` 段，無隱含預設
- `[authz.permission_tools]` 與 `[authz.intent_tools]`，後者覆蓋 intent_allowlist 全部項目
- `[report.grants]` 含六個 tool
- 開機驗證涵蓋映射表全覆蓋與 report grant 對 advertised MCP 集合

#### Verification

- cargo check
- cargo test

#### Verification Evidence

- PASS | local:../implement-report.md | cargo check
- PASS | local:../implement-report.md | cargo test

### T03 | S2 actor_key 推導與 pepper 載入

- Status: done
- Depends On: [T02]

#### Intent

實作 pepper HMAC 的 actor_key 推導，pepper 缺失或過短即啟動失敗。
#### Expected Result

同一 user_id 加同一 pepper 恆得同一 actor_key；pepper 不合規時服務拒絕啟動且錯誤不含 pepper 內容。
#### Definition of Done

- ActorKey 為 `v1:` 前綴加 base64url HMAC-SHA256 截斷 32 字元
- ACTOR_KEY_PEPPER 缺失、為空或短於 32 bytes 即啟動失敗
- 錯誤訊息指出變數名與長度下限，不含 pepper 任何內容

#### Verification

- cargo test

#### Verification Evidence

- PASS | local:../implement-report.md | cargo test

### T04 | S3 Falcon permissions client 與快取

- Status: done
- Depends On: [T01, T02]

#### Intent

實作 PermissionsProvider trait、HTTP 實作、指南文件化的 200 response 與 generic 401 分類，以及正/負向快取與 internal code 常數集。
#### Expected Result

200 response 的 roles／permissions 解析正確；permissions 401 一律分類為 unauthorized；同一 token 在 TTL 內重複請求不再外呼且重放原 internal failure 類別；快取有條目上界與 LRU 淘汰。
#### Definition of Done

- PermissionsProvider trait 與 HTTP 實作
- 指南 200 shape（含 role objects）解析；401 依 `error_code` 白名單分類（僅 `auth.token_invalid` 可續期，未知與缺漏皆終端）
- 正向快取 TTL 60s、負向快取 TTL 10s，key 為 token hash 且 value 不含 token
- 正負向快取皆有條目上界與 LRU 淘汰
- codes.rs 的 internal code 常數（不含 upstream error-code enum）

#### Verification

- cargo test

#### Verification Evidence

- PASS | local:../implement-report.md | cargo test

### T05 | S4 identity middleware

- Status: done
- Depends On: [T03, T04]

#### Intent

實作身份 middleware：取 header、呼叫 provider、推導 actor_key、寫入 request extensions。
#### Expected Result

缺 header、permissions 401、upstream unavailable 回互異的 internal code；非 POST 直通。
#### Definition of Done

- X-Falcon-Authorization 缺失回 401 identity.header_missing
- permissions endpoint 401 回 401 identity.token_refreshable / identity.token_terminal
- timeout、連線失敗、5xx、畸形 200 回 503 identity.upstream_unavailable
- 讀取 upstream `error_code` 作為內部判定輸入，但不轉送給消費端
- 非 POST 比照 enforce 直通，既有 405 契約不變
- IdentityContext 寫入 request extensions

#### Verification

- cargo test

#### Verification Evidence

- PASS | local:../implement-report.md | cargo test

### T06 | S5 三種信封的 code 欄位

- Status: done
- Depends On: [T04]

#### Intent

在三種錯誤信封加法式新增 code，並在 auth.rs 的兩個 body 產生點帶上 service token 的 code。
#### Expected Result

既有消費端只讀 error / data 的行為不變；ERR-001 的 418 與 401 都帶 code。
#### Definition of Done

- ErrorBody、OpenAiErrorBody、StreamFrame::Error 各新增 code
- StreamFrame 新增 Refusal 變體，chat.completion 新增 x_refusal_code
- auth.rs 的 418 teapot body 與 openai_unauthorized 帶 auth.service_token_invalid
- 序列化測試確認既有欄位名稱與語義不變

#### Verification

- cargo test

#### Verification Evidence

- PASS | local:../implement-report.md | cargo test

### T07 | S6 route 掛載 identity layer

- Status: done
- Depends On: [T05]

#### Intent

把 identity layer 掛在兩條 prompt 路由的 bearer 之內、外層限流之後。
#### Expected Result

probe 路由不受身份層影響；兩條 prompt 路由在缺身份時回 401。
#### Definition of Done

- identity layer 只掛 /agent/stream 與 /v1/chat/completions
- 不帶身份 header 時 /health、/ready、/greeting 仍為 200
- 兩條 prompt 路由回 401
- tests/route_contract.rs 仍通過

#### Verification

- cargo test

#### Verification Evidence

- PASS | local:../implement-report.md | cargo test

### T08 | S7 內層 per-actor 限流與開機耦合

- Status: done
- Depends On: [T05, T07, T11]

#### Intent

新增內層 per-actor bucket 並掛載；外層 limiter 未啟用時拒絕啟動。
#### Expected Result

兩個 actor 互不影響；外層總量上限仍生效；audit 標明拒絕層。
#### Definition of Done

- per-actor keyed bucket，含數量上界與 LRU 淘汰
- 在兩條 prompt 路由掛載內層 middleware
- audit event 標明拒絕層，外層拒絕不含 actor_key
- limiter 未啟用時啟動失敗

#### Verification

- cargo test

#### Verification Evidence

- PASS | local:../implement-report.md | cargo test

### T09 | S8 audit 的獨立 actor 欄位

- Status: done
- Depends On: [T03]

#### Intent

在 AuditCtx、AuditRecord 與 sinks 新增獨立 opaque actor 欄位，並新增兩個事件。
#### Expected Result

序列化後的 record 帶 actor_key 且不含 IP、user agent、session id、token、prompt、response。
#### Definition of Done

- 新增獨立 opaque actor 欄位，不併入 AuditActor
- AuditRecord 與 tracing / stdout sink 同步新增
- 新增 PermissionDegraded 與 IdentityAlarm 事件

#### Verification

- cargo test

#### Verification Evidence

- PASS | local:../implement-report.md | cargo test

### T10 | S9 三方交集授權判定

- Status: done
- Depends On: [T02]

#### Intent

實作 boot ∩ permission ∩ intent required 的判定，含 default-deny、萬用字元展開與 report 例外。
#### Expected Result

只有財務權限的使用者問會員主題被擋；charter grant 不被清空；混合 prompt 走 report 降級。
#### Definition of Done

- 三方交集與 default-deny
- 萬用字元先經 expand_grant 展開再取交集，該函式改為 crate 可見
- charter 與 composer grant 豁免收窄
- report 判定 predicate 使用 wants_report_pipeline，非嚴格 intent 比較

#### Verification

- cargo test

#### Verification Evidence

- PASS | local:../implement-report.md | cargo test

### T11 | S10 測試 fixture 與替身

- Status: done
- Depends On: [T03, T04]

#### Intent

擴充 test_support：runtime-wired app_state、PermissionsProvider stub、呼叫計數 spy、全域 tracing capture、腳本化 chat-completions stub。
#### Expected Result

fixture 可在 crate 內 cfg-test 模組跑通一條 prompt 路由並產出終端答案。
#### Definition of Done

- runtime-wired app_state 變體（現況為 runtime: None）
- PermissionsProvider stub 可程式化回文件化 200、generic 401 與不可用結果，並記錄呼叫次數
- LLM 與 MCP 呼叫計數 spy
- 全域安裝的 tracing capture layer 寫入共享 buffer，能看見 spawned task 的輸出
- 腳本化 chat-completions stub，可依序驅動 fetcher 的 tool_calls 與 schema 合法的 emit_report

#### Verification

- cargo test

#### Verification Evidence

- PASS | local:../implement-report.md | cargo test

### T12 | S11 handler 取身份與收窄 grant

- Status: done
- Depends On: [T05, T10, T11]

#### Intent

兩個 handler 取身份、四個 pipeline 建構點改傳收窄後 grant、忽略 client 脈絡、注入缺漏聲明。
#### Expected Result

無權限 intent 在呼叫 LLM 與 MCP 之前被拒；降級報告的缺漏聲明存活於終端答案。
#### Definition of Done

- 兩 handler 從 extensions 取 IdentityContext
- 四個 pipeline 建構點改傳收窄後 grant
- report 改用 [report.grants]
- 不呼叫 fold_history_into_prompt；忽略 AgentRequest.history
- 缺漏聲明併入終端答案，SSE 併入 Clear 之後的終端 Token，/v1 用 with_prefix
- 發出 PermissionDegraded 事件

#### Verification

- cargo test

#### Verification Evidence

- PASS | local:../implement-report.md | cargo test

### T13 | S11a 身份載體接線

- Status: done
- Depends On: [T05, T12]

#### Intent

在 AgentTurnInput 新增 identity 欄位並調整 prelude 三處簽章與兩個 handler 呼叫點。
#### Expected Result

actor_id 不再為 None；權限過濾與 actor_key 填值都取得到輸入。
#### Definition of Done

- AgentTurnInput 新增 identity 欄位
- apply_memory_context、append_memory_turn_if_enabled、plan_stream_turn 簽章調整
- 兩個 handler 呼叫點（含直接 append 的那處）帶入身份

#### Verification

- cargo check
- cargo test

#### Verification Evidence

- PASS | local:../implement-report.md | cargo check
- PASS | local:../implement-report.md | cargo test

### T14 | S13 記憶脈絡的權限過濾

- Status: done
- Depends On: [T10, T13]

#### Intent

組裝脈絡時逐 turn 依當次權限過濾，report turn 要求全涵蓋。
#### Expected Result

權限撤銷後舊脈絡不再進入 LLM，且略過筆數進 audit、內容不進。
#### Definition of Done

- 逐 turn 依當次權限判定
- unknown 或缺 intent 一律略過
- report intent turn 要求權限全涵蓋才保留
- 略過筆數進 audit，被略過的內容不進

#### Verification

- cargo test

#### Verification Evidence

- PASS | local:../implement-report.md | cargo test

### T15 | S14 session scope 填入 actor_key

- Status: done
- Depends On: [T02, T03, T13]

#### Intent

把 scope.actor_id 從寫死的 None 改為實際 actor_key。
#### Expected Result

不同 actor 使用同一 session id 時取不到對方任何已存值。
#### Definition of Done

- scope.actor_id 填實值
- user_summary 與 answer_summary 維持現況（summary 邊界已移出，見 PRD FU-007）

#### Verification

- cargo test

#### Verification Evidence

- PASS | local:../implement-report.md | cargo test

### T16 | S15 fetcher prompt 改為 grant-aware

- Status: done
- Depends On: [T12]

#### Intent

移除 prompt 中寫死的六個 tool 名稱，改為依授予的 tool 敘述。
#### Expected Result

降級情境下 prompt 不再要求呼叫未授權的 tool。
#### Definition of Done

- 移除寫死的六個 tool 名稱
- 改為 grant-aware 敘述

#### Verification

- cargo test

#### Verification Evidence

- PASS | local:../implement-report.md | cargo test

### T17 | S16 router 層整合測試

- Status: done
- Depends On: [T02, T03, T04, T05, T06, T07, T08, T09, T10, T11, T12, T13, T14, T15, T16]

#### Intent

在 crate 內 cfg-test 模組實作 SEAM-001 的整合測試，覆蓋 QA plan 指派的案例。
#### Expected Result

QA plan 的 L2、L4、元件三類案例全綠。
#### Definition of Done

- 測試置於 crate 內 cfg-test 模組，非外部 tests crate
- 覆蓋 QA plan 的 19 條 AC、2 條專屬 ERR、9 條 L4 邊界
- TC-C01 以真實 client 對本機腳本化 listener 驗證 URL、Bearer transport、逾時、200 shape 與 generic 401 分類
- TC-009 斷言終端 frame 而非中間 token
- TC-B11 快取 LRU 與 TC-B12 混合 intent 路由

#### Verification

- cargo test
- cargo clippy -- -D warnings

#### Verification Evidence

- PASS | local:../implement-report.md | cargo test
- PASS | local:../implement-report.md | cargo clippy -- -D warnings

### T18 | S18 公開契約與消費端 handoff

- Status: done
- Depends On: [T06, T12]

#### Intent

更新兩份 endpoint 文件並產出消費端 handoff。
#### Expected Result

消費端可依文件完成改動；文件不含任何憑證值。
#### Definition of Done

- agent-stream.md 與 chat-completions.md 更新 header、code 列舉與三種身份失敗
- chat-completions.md 明載授權模式下為單輪、無 session_id
- handoff-consumers.md 列出 C1–C9 與 permissions 401／internal code 的處置矩陣與三階段部署順序

#### Verification

- bash .agent/skills/_shared/scripts/lint-docs.sh

#### Verification Evidence

- PASS | local:../implement-report.md | bash .agent/skills/_shared/scripts/lint-docs.sh

### T19 | S19 本機開發拓樸

- Status: done
- Depends On: [T02, T03]

#### Intent

提供本機 mock permissions 端點的啟動說明與必要環境變數，並記錄 pepper 輪替步驟。
#### Expected Result

依文件可在本機啟動並完成一次授權請求。
#### Definition of Done

- 可設定 base URL 的 mock permissions 端點說明與範例回應
- 必要環境變數清單
- pepper 輪替的 runbook 步驟

#### Verification

- 人工依文件執行一次本機啟動

#### Verification Evidence

- PASS | local:../../../../scripts/dev-falcon-stub.md | 人工依文件執行一次本機啟動

## Change Log

- 2026-08-28: Completed T04-T18. Added the documented Falcon permissions client/cache, identity and actor-scoped request path, two-layer limiting, authorization-aware runtime and memory handling, test doubles crossing the handler/runtime/MCP/LLM boundaries, public contract docs, and consumer handoff. Fresh plan check reports 19/19 done with no blockers.
- 2026-08-28: User selected the specified Falcon guide commit as SSOT. Revised PRD v1.9.0 and spec v1.4.0 to use the documented permissions 200 shape and generic 401; removed the unsupported seven-code/ERR-008/ERR-010 contract and completed T01.
- 2026-08-27: Created the canonical plan from spec v1.3.1 steps S0-S19.
