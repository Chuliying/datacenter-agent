# Falcon 使用者身份與 RBAC 接進 runtime 請求路徑 QA 方案

**PRD**: `docs/work/runtime-falcon-identity-rbac/prd.md` v1.9.0
**Spec**: `docs/work/runtime-falcon-identity-rbac/spec.md` v1.4.0
**Capabilities**: `has_ui=false` · `has_api=true` · `typed_contracts=true` · `has_e2e=false`

**Falcon 外呼契約 SSOT**: [`External_Delegated_API_Integration.md`](https://github.com/HDRenewables/falcon-backend/blob/c5d0408b5e1847bf165e8c5f5859ef61da93e02d/docs/External_Delegated_API_Integration.md) @ `c5d0408b5e1847bf165e8c5f5859ef61da93e02d`；permissions endpoint 僅以文件化的 200 shape 與 401 驗證，不測試未附來源的 upstream error-code 表。

---

## Selected Seam

| Field | Decision |
|---|---|
| Spec seam ID | SEAM-001 |
| Spec reference | spec.md#selected-seam |

QA 不改寫 seam 決策。主要行為在 SEAM-001（axum router 邊界整合測試）驗證；L2 覆蓋 spec 列出的三項 residual risk，加上 spec Test Strategy 另行指定的啟動失敗與信封序列化。

**測試落點（重要）**：L4 測試必須寫在 **crate 內的 cfg-test 模組**，不可放在外部 `tests/` crate。`src/lib.rs:27-28` 把 `test_support` 宣告為 cfg-test 且 crate-private，外部 test crate 編譯 lib 時沒有 cfg(test)、只看得到 pub 項目，因此 `app_state()`、`stub_mcp()`、`TEST_TOKEN` 與 `rate_limit.rs` 的 seam helper 從 `tests/` **完全不可達**。spec v1.4.0 的 S16 已隨此修正。

---

## Challenge Spec：可測試性檢視

開始設計案例前先質疑 spec。四項發現，兩項需要 spec 或 PRD 補件，兩項已由 spec 處理：

| # | 問題 | 判定 |
|---|---|---|
| Q1 | **AC-002「token 不出現在任何持久化或觀測輸出」怎麼測？** tracing 輸出要能被攔截才能斷言 | 可測。`src/runtime/audit.rs` 的測試已有 `CapturingSink` 模式可沿用；tracing 側需在 S16 加一個 subscriber capture layer。**需求：S16 的 fixture 要包含 tracing capture，spec 的 S10 目前只列 LLM/MCP 呼叫計數 spy** |
| Q2 | **FR-004 的「降級報告不得含無權限主題的數字」無自動承接點。** spec 已宣告交給 QA plan | 由本方案的 TC-M01 承接（人工／探索式）。這是**已知的自動化缺口**，不是遺漏 |
| Q3 | AC-009 需要 pipeline 跑完（含 `emit_report` schema 合法 payload） | spec S10 已編列腳本化 chat-completions stub（4h），可行 |
| Q4 | AC-015 是部署前一次性檢查，對象是舊 binary | spec 已宣告；由本方案的 TC-D01 以部署檢查承接，不列為迴歸測試 |

**結論**：spec 可測，不退回。Q1 是一行 fixture 補充，記在下方 Mock/Fixture Plan 與 Gate 自檢，實作時併入 S10。

---

## 1. Traceability

| Source | Behavior / Risk | Test ID | Level | Evidence |
|--------|-----------------|---------|-------|----------|
| AC-001 | 權限來自 Falcon 且不擴權；只呼叫 permissions 一個端點 | TC-001 + TC-C01 | L4 + 元件 | `cargo test`。TC-001 只能斷言「trait 恰被呼叫一次」（trait 呼叫不帶 URL）；「只打一個端點」由 TC-C01 對真實 HTTP client 斷言 |
| AC-002 | 使用者 token 不進 log / tracing / audit / SQLite | TC-002 | L4 | `cargo test`；tracing capture + audit CapturingSink + DB 掃描 |
| AC-003 | 正向 cache 命中不再外呼 | TC-003 | L4 | `cargo test`；provider stub 呼叫次數 == 1 |
| AC-004 | `actor_key` 是穩定 opaque 假名（已知答案 HMAC） | TC-004 | L2 | `cargo test` |
| AC-005 | session memory 以 `actor_key` 隔離 | TC-005 | L4 | `cargo test`；兩 actor 同 session id |
| AC-006 | 無權限 intent 在 LLM/MCP 之前被拒 | TC-006 | L4 | `cargo test`；LLM/MCP 呼叫計數 == 0 |
| AC-007 | 收窄只減不增（交集性質） | TC-007 | L2 | `cargo test` |
| AC-008 | 事業部父層不隱含子頁 | TC-008 | L4 | `cargo test` |
| AC-009 | report 降級並聲明缺漏 | TC-009 | L4 | `cargo test`；斷言**終端** frame 內容 |
| AC-010 | 三種身份失敗的 `code` 互異 | TC-010 | L4 | `cargo test` |
| AC-011 | Falcon 不可用時 fail-closed 非降級 | TC-011 | L4 | `cargo test` |
| AC-012 | per-actor 限流互不影響 | TC-012 | L4 | `cargo test` |
| AC-013 | rate limit audit 帶 actor 不帶敏感欄位 | TC-013 | L4 | `cargo test`；CapturingSink |
| AC-014 | pepper 缺失啟動失敗 | TC-014 | L2 | `cargo test` |
| AC-015 | 補 header 對舊 binary 向前相容 | TC-D01 | 部署檢查 | 手動；對象為當前 main 的 binary |
| AC-016 | 兩條路徑身份與權限行為一致 | TC-016 | L4 | `cargo test` |
| AC-018 | 外層全域上限仍生效 | TC-018 | L4 | `cargo test` |
| AC-020 | Falcon permissions `401` 的負向 cache 命中重放同一 internal 類別 | TC-020 | L4 | `cargo test`；不依賴未承諾的 upstream `error_code` |
| AC-021 | limiter 未啟用時拒絕啟動 | TC-021 | L2 | `cargo test` |
| AC-022 | 授權判定以 intent 所需 tool 為基準 | TC-022 | L4 | `cargo test` |
| AC-023 | 權限撤銷後舊脈絡不進 LLM | TC-023 | L4 | `cargo test` |
| AC-024 | 授權模式下 client history 被忽略 | TC-024 | L4 | `cargo test` |
| AC-025 | 401 負向 cache 命中重放同類別 | TC-025 | L4 | `cargo test` |
| AC-026 | OpenAI 路徑只採最後一則 user message | TC-026 | L4 | `cargo test` |
| ERR-001 | service token 無效 → 418 / 401 + code | TC-ERR01 | L4 | `cargo test` |
| ERR-002 | 缺身份 header → 401 不可 refresh | TC-ERR02 | L4 | 併入 TC-010 斷言 |
| ERR-003 | 401 `auth.token_invalid` → `identity.token_refreshable` | TC-ERR03 | L4 | 併入 TC-010；另有 wire 層測試直接驗 401 body 的 code 讀取 |
| ERR-008 | 401 其餘任何 code（含未知、缺漏）→ `identity.token_terminal` | TC-ERR08 | L4 | 白名單分類，未知一律終端 |
| ERR-004 | 上游不可用 → 503 可重試 | TC-ERR04 | L4 | 併入 TC-011 斷言（分類正確性由 TC-C01 覆蓋）|
| spec `src/server/falcon.rs` 的 HTTP 實作 | URL、Bearer transport、逾時、200 解析、generic 401 分類 | TC-C01 | 元件 | `cargo test`；真實 client 對本機腳本化 listener |
| ERR-005 | 權限不足 → 200 + `authz.insufficient` 載體 | TC-ERR05 | L4 | `cargo test`；SSE `Refusal` frame 與 `/v1` 的 `x_refusal_code` |
| ERR-006 | 限流 → 429 + Retry-After + 拒絕層 | TC-ERR06 | L4 | 併入 TC-012 / TC-018 |
| ERR-007 | pepper 缺失或過短 → 啟動失敗 | TC-ERR07 | L2 | 併入 TC-014；另加「過短」案例 |
| ERR-009 | limiter 未啟用 → 啟動失敗 | TC-ERR09 | L2 | 併入 TC-021 |

---

## 2. Test Cases

代表性案例；其餘依 Traceability 表的 Given-When-Then 直接取自 PRD 的 AC 原文。

### TC-002: 使用者 token 不出現在任何持久化或觀測輸出

**Source**: AC-002 / spec: `src/server/falcon.rs` 的 cache key 推導
**Level**: L4
**Applicability**: always
**Precondition**: runtime-wired fixture、provider stub 回一組權限、**全域安裝**的 tracing capture layer（寫入共享 buffer）、audit `CapturingSink` 已裝
**Steps**:

```text
Given 一個帶已知字串 token 的請求已完整處理完成
When 檢查 tracing 輸出與 audit records
Then 該 token 字串在兩者中出現次數皆為 0
```

**Implementation notes**:
- **SQLite 掃描為 N/A**：`SqliteRuntimeStore` 在 `src/` 內零呼叫者（只有它自己的檔案與 `tests/runtime_store_sqlite.rs` 自建 store），PRD FR-002 boundary 也明載本刀不產生任何 ledger 或 session 寫入。掃一個請求路徑不會開啟的 store 是**空洞斷言**，會在未來有 slice 接上洩漏的 store 時繼續假過。該子句改由 FU（SQLite session repository 接線）那一刀承接。
- **cache key 斷言移到 L2**：router 層無法列舉內部 cache 的 key。改在 `src/server/falcon.rs` 的單元測試斷言「key 為 token 的 hash 且不等於原文」。
- **tracing capture 必須是全域安裝的 layer 寫入共享 buffer**，不可用 per-test scoped subscriber——spawned task（MCP stub session、串流 task）跑在測試 task 的 default-subscriber scope 之外，scoped subscriber 會**漏掉正是 AC-002 要抓的洩漏**。Q1 的補充不是「一行」。

### TC-009: report 降級並聲明缺漏

**Source**: AC-009 / spec: 缺漏聲明注入點（釘定）
**Level**: L4
**Applicability**: always
**Precondition**: 使用者只有 finance 權限；腳本化 chat-completions stub 依序回 fetcher 的 `tool_calls` 與 schema 合法的 `emit_report` payload
**Steps**:

```text
Given 只有財務讀取權限的使用者要求完整報告
When 收集 /agent/stream 的全部 SSE frame 直到 Done
Then 對非授權 endpoint 的 MCP 呼叫次數為 0，終端 Token frame 的內容含被省略的主題名稱，且 CapturingSink 收到一筆列出同樣主題名稱的 PermissionDegraded 事件
```

**Implementation notes**: **必須斷言 `Clear` 之後的終端 frame**，不可斷言中間的 transient token——`handler.rs:1155-1158` 的終端 `Clear` 會抹掉先前送出的內容。同一情境在 `/v1` 斷言 `with_prefix` 後的回答。

### TC-023: 權限撤銷後舊脈絡不再進入 LLM

**Source**: AC-023 / spec: `src/runtime/memory/context.rs` 的逐 turn 過濾
**Level**: L4
**Applicability**: always
**Precondition**: 同一 `session_id` 先以含 bizdev 權限完成一輪會員主題對話；隨後 provider stub 改回不含 bizdev 的權限集合，並以 S1 的可設定正向 TTL 設為 0 使 cache 立即逾期（**不得用 wall-clock sleep**）
**Steps**:

```text
Given session memory 已有一筆會員主題 turn，且該使用者的業務發展權限已被撤銷
When 該使用者送出下一個請求
Then 送進 LLM 的 prompt 不含該筆 turn 的內容，且 audit 記錄略過筆數為 1
```

**Implementation notes**: 「送進 LLM 的 prompt」由腳本化 stub 記錄其收到的 request body 取得。

### TC-ERR05: 權限不足的拒答帶可程式判斷的 code

**Source**: ERR-005 / spec: `authz.insufficient` 的載體（釘定）
**Level**: L4
**Precondition**: 使用者權限與該 intent 所需 tool 無交集
**Steps**:

```text
Given effective 集合為空
When 分別對 /agent/stream 與 /v1/chat/completions 送出該請求
Then 兩者都回 200；SSE 在 Done 之前出現帶 authz.insufficient 的 Refusal frame，/v1 的 completion 物件帶同值的 x_refusal_code
```

### TC-ERR03: permissions endpoint 的 generic 401

**Source**: ERR-003 / 指定版 Falcon 指南第 2 節
**Level**: L4
**Precondition**: provider stub 回 `401`，body 可帶任意未被契約承諾的欄位；同一 token 的負向 cache 可命中
**Steps**:

```text
Given Falcon permissions endpoint 回 401，且 body 中可能出現 runtime 不應依賴的 error_code
When 以同一 token 連送兩個請求
Then 首次與 cache replay 都回同一個 401 internal code，重放的是分類後的結果而非上游字串，第二次不再呼叫 Falcon
```

### TC-C01: 真實 Falcon HTTP client 的契約

**Source**: spec `src/server/falcon.rs` 的 HTTP 實作與指定版 Falcon 指南（AC-001 的另一半）
**Level**: 元件（真實 client 對本機腳本化 HTTP listener，非 trait 替身）
**Applicability**: always
**Precondition**: 本機 listener 綁隨機 port，`FALCON_API_BASE_URL` 指向它。技術與 chat-completions stub 相同，不新增依賴（不違反 D-002 對 wiremock 的排除）
**Steps**:

```text
Given 真實的 FalconPermissionsProvider 指向本機 listener
When 以一組已知 token 呼叫一次
Then listener 收到的路徑恰為 /api/auth/me/permissions、Authorization header 為該 token 的 Bearer 形式，且沒有任何其他路徑被請求
```

**額外斷言**（同一 listener 依序腳本化）：

| 情境 | 期望 |
|---|---|
| 200 + 合法 body | 解析出 `user_id`、物件形式的 `roles` 與只含 `can_read` 為真的 effective permission code 集合 |
| `401` body 帶 `auth.token_invalid` | `identity.token_refreshable` |
| `401` body 帶其他 code、未知 code、或無 code | `identity.token_terminal`（白名單，未知一律終端）|
| 逾時 | 分類為不可用（可重試），且不超過設定的 5 秒 |
| 5xx | 同上 |
| 200 但 body 無法解析 | 同上（ERR-004 的畸形 200 分支）|

**Implementation notes**: 這條補的是 trait 替身**測不到**的東西——URL 組裝、Bearer transport、逾時設定、body 解析與 generic 401 分類。少了它，整套測試可以全綠而第一次打真實 Falcon 就因路徑錯、缺 Bearer 前綴或欄位名不符而失敗。

### TC-D01: 補 header 對未含身份層的 binary 向前相容

**Source**: AC-015
**Level**: 部署前一次性檢查（非迴歸測試）
**Precondition**: 部署當前 `origin/main` 的 binary
**Steps**:

```text
Given 目前線上版本的 runtime，也就是尚未含本 PRD 身份層的 binary
When 對它送出一個額外帶著 X-Falcon-Authorization 的請求
Then 處理結果與未帶該 header 時完全一致
```

**Implementation notes**: 交付後的 binary 無法重建此前提（D3 不設 flag），因此**不可**寫成 `cargo test` 案例。列入部署 runbook 的第二階段檢查項。

### TC-M01: 降級報告的無權限主題數字洩漏（人工／探索式）

**Source**: PRD FR-004 boundary
**Level**: 人工
**Precondition**: 使用者只有 finance 權限；以真實 LLM 產出降級報告
**Steps**:

```text
Given 只有財務權限的使用者取得降級報告
When 人工檢視報告全文
Then 報告不含任何營運或會員主題的具體數字
```

**Implementation notes**: 自由生成內容無法以契約測試判定，範圍縮減也移除了唯一的 eval 步驟。這是**已宣告的自動化缺口**（spec「追溯缺口」章節），以人工抽驗承接。抽驗頻率：每次 report prompt 相關變更後至少一次。

---

## 3. Boundary Tests

| Test ID | Boundary | Expected behavior | Source |
|---------|----------|-------------------|--------|
| TC-B01 | 非 POST 進入受限路由 | 回 405，不消耗任一層允入容量，且**不進行身份解析**（既有 `wrong_method_requests_do_not_consume_capacity` 仍過） | spec Errors 邊界列 |
| TC-B02 | `pepper` 存在但短於 32 bytes | 啟動失敗，錯誤指出長度下限且不含 pepper 內容 | ERR-007 |
| TC-B03 | 映射表未涵蓋 `intent_allowlist` 的某個 intent | **啟動**失敗，非執行期 default-deny | FR-003 邊界 |
| TC-B04 | `[report.grants]` 含 advertised 集合外的 wire name | 啟動失敗 | spec S1 |
| TC-B05 | fetcher grant 為 `["*"]` | 先經 `expand_grant` 展開再取交集，charter 的 `emit_chart` 不被清空 | spec D-013 |
| TC-B06 | 只有其他事業部權限（如 startrade-power） | `effective` 為空 → ERR-005，屬預期結果 | FR-003 邊界 |
| TC-B07 | `unknown` intent | 無 required tools，永遠落拒答分支 | FR-003 邊界 |
| TC-B08 | 已存的 report intent turn 遇上部分權限使用者 | 整筆略過（要求全涵蓋） | spec Errors 邊界列 |
| TC-B09 | 不帶身份 header 時的 `/health`、`/ready`、`/greeting` | 仍為 200（identity layer 只掛兩條 prompt 路由） | spec S6 |
| TC-B10 | per-actor bucket 數量達上界 | LRU 淘汰；被淘汰的 actor 回到滿額，外層總量上限不因此放寬 | FR-005 邊界 |
| TC-B11 | 正／負向權限 cache 條目數達上界 | LRU 淘汰；塞入 N+1 個相異 token hash 後，最舊的條目被淘汰，該 token 再送會重新觸發一次上游呼叫。**沒有這條，無上界的 cache 實作會通過整份方案** | spec Errors「邊界：cache 命中」 |
| TC-B12 | top intent 為 `revenue`、candidate 含 `report`（如「營收報告」）且使用者只有部分權限 | 走 report 降級產出，**不是**嚴格判定的拒答。守 spec D-014：閘門若退回 `intent == report`（D-014 明列的被否決選項），這類 mixed prompt 會被錯誤擋下而整份方案仍全綠 | spec D-014 / `handler.rs:129-135` |

---

## 4. Capability-specific Tests

| Capability | Required tests | Evidence |
|------------|----------------|----------|
| `has_ui` | N/A (has_ui=false) | N/A |
| `has_api` | L4 整合契約：以 `PermissionsProvider` 受控替身覆蓋 `GET /api/auth/me/permissions` 的文件化 200、generic 401 與不可用情境；替身回應形狀必須符合 spec 的 `FalconPermissionsResponse`、`FalconRole` 與 `PermissionItem` | `cargo test` |
| `typed_contracts` | `cargo check`；信封序列化測試確認 `error` / `data` 既有欄位不變 | `cargo check` + `cargo test` |
| `has_e2e` | N/A (has_e2e=false)；manifest 無 e2e 指令 | N/A |

---

## 5. Test Matrix

| Level | Scope | Count | Command / Evidence |
|-------|-------|------:|--------------------|
| L2 Unit | spec 三項 residual risk（`actor_key` 已知答案 HMAC、指南 200 shape／generic 401 分類與 cache key、三方交集含 `"*"` 展開與 charter 豁免）＋ spec Test Strategy 另列的信封序列化 ＋ 啟動失敗 TC-014／TC-021 ＋ 開機驗證邊界 TC-B02／TC-B03／TC-B04 | 10 | `cargo test` |
| L3 Component | N/A (has_ui=false) | 0 | N/A |
| L4 Integration | SEAM-001：19 條 AC ＋ 2 條 ERR 專屬案例（TC-ERR01／05，其餘折入）＋ 9 條邊界（TC-B01、B05–B12；B02/B03/B04 為開機失敗，歸 L2）| 30 | `cargo test` |
| L5 E2E | N/A (has_e2e=false) | 0 | N/A |
| 元件 | TC-C01：真實 Falcon HTTP client 對本機腳本化 listener | 1 | `cargo test` |
| 非自動化 | TC-D01（部署檢查）、TC-M01（人工抽驗） | 2 | runbook / 人工紀錄 |

---

## 6. Mock / Fixture Plan

| Dependency | Strategy | Contract source |
|------------|----------|-----------------|
| Falcon `GET /api/auth/me/permissions`（L4 用）| `PermissionsProvider` trait 的 in-process 受控替身（spec D-002）。可程式化回文件化 200、generic 401 或不可用，並記錄**呼叫次數**——trait 呼叫不帶 URL，URL 斷言只能由 TC-C01 提供 | 指定版 `External_Delegated_API_Integration.md`；permissions endpoint 沒有逐列 upstream `error_code` 表 |
| Falcon 端點（TC-C01 用）| 本機腳本化 HTTP listener，`FALCON_API_BASE_URL` 指向它，驅動**真實** client | 同上；200 的 `roles` 使用物件 shape |
| OpenRouter / LLM | 腳本化 chat-completions stub，經 `base_url` 指向本機（`src/agent/llm.rs:241-249`）。需能依序回 fetcher 的 `tool_calls` 與 schema 合法的 `emit_report` payload | spec S10 |
| MCP server | 沿用既有 `test_support::stub_mcp()` | `src/test_support.rs` |
| tracing 輸出 | **全域安裝**的 capture layer 寫入共享 buffer（S10 需補，Q1）。不可用 per-test scoped subscriber——spawned task 在其 scope 之外 | AC-002 |
| audit sink | 沿用 `src/runtime/audit.rs` 測試的 `CapturingSink` 模式 | AC-013 |
| SQLite | **N/A**：`SqliteRuntimeStore` 在 `src/` 內零呼叫者，本刀不產生任何寫入，掃描它是空洞斷言 | 見 TC-002 備註 |

**Mock 安全**：替身回應必須符合指定指南的 `FalconPermissionsResponse`（`roles` 是物件陣列；`permissions` 含 `name`、可為 null 的 `page_path`、`can_read`、`can_write`）；401 不得被測試偽造為未在指南承諾的逐列 code 契約。`emit_report` payload 必須通過既有 schema 驗證，否則測到的是 stub 的 bug 而非產品行為。

---

## Machine traceability gate

- [x] Every PRD AC maps to at least one test（24 條全映射；AC-015 映到部署檢查 TC-D01 並已宣告）。
- [x] Every PRD ERR maps to at least one error or recovery test（ERR-001..009 全覆蓋；v1.9.0 移除未被 permissions 契約承諾的 ERR-008／ERR-010）。
- [x] Every test has a source ID and level。
- [x] Capability-disabled sections are marked N/A（`has_ui`、`has_e2e`）。
- [x] Mock or fixture data follows the Spec/API contract（見 Mock 安全）。
- [x] Representative test evidence is executable or has a concrete command（`cargo test` / `cargo check`）。

### 兩項已宣告的自動化缺口

| 缺口 | 承接方式 | 原因 |
|---|---|---|
| AC-015 | TC-D01 部署前一次性檢查 | D3 不設 flag，交付後的 binary 無法重建「未含身份層」前提 |
| FR-004 數字洩漏 | TC-M01 人工抽驗 | 自由生成內容無法契約判定；範圍縮減移除了唯一的 eval 步驟 |

### 一項給實作的 fixture 補充

Q1：`src/test_support.rs` 的 fixture 除 spec S10 已列的 LLM/MCP 呼叫計數 spy 外，需再加 **全域安裝的 tracing capture layer**，否則 AC-002 的「不進 log」無法斷言。**這不是一行補充**：per-test scoped subscriber 看不到 spawned task 的輸出，會漏掉正是該 AC 要抓的洩漏。實作時併入 S10，並相應調整估時。

---

## Related Documents

| Document | Link |
|----------|------|
| PRD | `docs/work/runtime-falcon-identity-rbac/prd.md` |
| Spec | `docs/work/runtime-falcon-identity-rbac/spec.md` |
