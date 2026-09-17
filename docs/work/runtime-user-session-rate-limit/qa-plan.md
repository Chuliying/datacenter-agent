# Runtime SQLite Persistence and Layered Rate-Limit POC QA 方案

**PRD**: `docs/work/runtime-user-session-rate-limit/prd.md` v0.5.0
**Spec**: `docs/work/runtime-user-session-rate-limit/spec.md` v1.0.0
**Capabilities**: `has_ui=false` · `has_api=true` · `typed_contracts=true` · `has_e2e=false`

---

## 1. Traceability

| Source | Behavior / Risk | Test ID | Level | Evidence |
|--------|-----------------|---------|-------|----------|
| AC-001 | 關檔重開後 user/session/summary/reservation 完整保留 | TC-001 | L4 | `cargo test --test runtime_store_sqlite reopen` |
| AC-002 | 跨 actor 存取回 ownership conflict 且零資料外洩 | TC-002 | L4 | `cargo test --test runtime_store_sqlite ownership` |
| AC-003 | 六筆含敏感 fixture 的 summary → 只存五筆淨化後結果 | TC-003 | L4 | `cargo test --test runtime_store_sqlite minimize` |
| AC-004 | 兩併發 reserve 剩額 1M 各搶 750k → 恰一成功 | TC-004 | L4 | `cargo test --test runtime_store_sqlite concurrent_reserve` |
| AC-005 | 同 reservation 重複 settle → 第二次 AlreadySettled | TC-005 | L4 | `cargo test --test runtime_store_sqlite idempotent_settle` |
| AC-006 | 台北月界:15:59:59Z→2026-08、16:00:00Z→2026-09、reset 正確 | TC-006 | L2 | `cargo test month` |
| AC-007 | Runbook 明列單 replica、persistent volume、opt-in 與未實作邊界 | TC-007 | L1 | 人工 read-through checklist(文件驗收) |
| AC-008 | bucket 空時認證請求 → 429,不進 handler/extractor/LLM/MCP/SQLite | TC-008 | L4 | `cargo test --test rate_limit_ingress reject_before_work` |
| AC-009 | bucket 空時 `/health` `/ready` `/greeting` 不受限、shape 不變 | TC-009 | L4 | `cargo test --test rate_limit_ingress probes_bypass` |
| AC-010 | 一次拒絕 = 恰一筆 audit 事件、零 SQLite 寫入、欄位無敏感值 | TC-010 | L4 | `cargo test --test rate_limit_ingress audit_bounded` |
| AC-011 | 已知超額結算記實際 spent、月 overdrawn、新 reserve 全拒 | TC-011 | L4 | `cargo test --test runtime_store_sqlite over_settlement` |
| AC-012 | 30 天 + 1 秒後 session 過期、ID 可被他人重新宣告 | TC-012 | L4 | `cargo test --test runtime_store_sqlite expiry` |
| AC-013 | explicit clear 單交易刪除並釋放 ownership | TC-013 | L4 | `cargo test --test runtime_store_sqlite clear_release` |
| AC-014 | 無效 bearer 被 418/401 拒且不耗 bucket;後續認證請求獲准 | TC-014 | L4 | `cargo test --test rate_limit_ingress auth_before_limiter` |
| AC-015 | 429 body 依 route family 釘死 + 整數 Retry-After + no-store | TC-015 | L4 | `cargo test --test rate_limit_ingress envelope_pinned` |
| ERR-001 | store 不可用 → typed error、不 fallback、不外洩 | TC-ERR01 | L4 | `cargo test --test runtime_store_sqlite unavailable` |
| ERR-002 | ownership conflict 錯誤路徑(同 AC-002 負向) | TC-ERR02 | L4 | `cargo test --test runtime_store_sqlite ownership` |
| ERR-003 | 超出月額度 → BudgetExceeded + next reset、無 reservation | TC-ERR03 | L4 | `cargo test --test runtime_store_sqlite budget_exceeded` |
| ERR-004 | 未知成本 → pending + ReconciliationRequired、不自動退款 | TC-ERR04 | L4 | `cargo test --test runtime_store_sqlite unknown_cost` |
| ERR-005 | burst 超限 → 429 完整回應、零昂貴操作 | TC-ERR05 | L4 | `cargo test --test rate_limit_ingress reject_before_work` |
| ERR-006 | 已知超額 → overdrawn 凍結(同 AC-011 負向延伸) | TC-ERR06 | L4 | `cargo test --test runtime_store_sqlite over_settlement` |

---

## 2. Test Cases

Slice 1 全部注入 `now: DateTime<Utc>`(spec D-004/D-005);Slice 2 用 `burst_size=1` + 長 refill period 確保確定性,以 `tower::ServiceExt::oneshot` 直打 `build_router` 產物,不啟動真實 server、不呼叫 OpenRouter/MCP(PRD out of scope)。

### TC-001: 關檔重開保留全部已提交狀態

**Source**: AC-001 / spec: `SqliteRuntimeStore`
**Level**: L4 · **Precondition**: temp dir 中新 SQLite 檔

```text
Given 一個 SQLite 檔已提交 opaque user、owned session、一筆淨化 summary、一筆 reservation
When 關閉 repository 後用同一路徑重開
Then 同一 owner、summary 內容與 ledger snapshot 完整讀回(未過期)
```

**Implementation notes**: `tests/runtime_store_sqlite.rs`;用 `tempfile::tempdir`?— repo 無此 dep,改用 `std::env::temp_dir()` + uuid 子目錄,測試結束刪除。

### TC-002 / TC-ERR02: session 跨 actor 隔離

**Source**: AC-002, ERR-002 · **Level**: L4

```text
Given actor A 擁有未過期 session s1 且存有一筆 summary
When actor B 對 s1 load 與 append
Then 回 StoreError::OwnershipConflict,錯誤值不含 A 的 owner key、summary 或存在細節
```

### TC-003: 淨化管線與五筆上限

**Source**: AC-003 / spec: `sanitize.rs` + `append_turn` · **Level**: L4

```text
Given 六筆 summary fixtures 含 email、IP、cookie、bearer token、連續空白、超過 500 字元文字
When 依序 append 至同一 owned session
Then 直接查 SQLite 只有最新五筆;無任何原始敏感 fixture 與超限全文(redact→normalize→truncate 之後的形狀)
```

**Implementation notes**: 斷言用 raw `SELECT`(繞過 repository)證明落庫內容,非僅 API 回傳。

### TC-004: 併發 reserve 原子性

**Source**: AC-004 · **Level**: L4

```text
Given actor A 本月剩 1,000,000 micro-USD
When 兩個 tokio task 同時對同一 store reserve 750,000
Then 恰一個 Reserved、另一個 BudgetExceeded,且 spent+reserved ≤ 20,000,000
```

### TC-005: settle 冪等

**Source**: AC-005 · **Level**: L4

```text
Given reservation g1 已成功
When 以同一權威金額 settle g1 兩次
Then 第一次 Settled、spent 只增加一次;第二次 AlreadySettled
```

### TC-006: 台北月界(L2 unit)

**Source**: AC-006 / spec: `month.rs` · **Level**: L2

```text
Given 2026-08-31T15:59:59Z 與 2026-08-31T16:00:00Z 兩個 instant
When 推導 month key 與 next reset
Then 分別得 2026-08 與 2026-09,後者 reset = 2026-09-30T16:00:00Z
```

### TC-007: Runbook 文件驗收(L1)

**Source**: AC-007 / FR-004 · **Level**: L1(人工 checklist)

```text
Given 開發者閱讀 runbook.md 與 config 範例
When 核對部署需求
Then 文件明列:單 replica、persistent volume、停機備份、opt-in burst 與「actor/月度 enforcement 未接線」「非多 replica 保護」的區別
```

### TC-008 / TC-ERR05: 拒絕先於昂貴工作

**Source**: AC-008, ERR-005 · **Level**: L4

```text
Given burst_size=1 的 router,第一個認證請求已耗盡 bucket
When 第二個認證、大小合法的請求打 /agent/stream
Then 回 429 + Retry-After;handler spy、JSON extractor 皆未被呼叫(spy handler panic 保證)
```

**Implementation notes**: 以測試 router 將真 handler 換成 `panic!` spy;業務未被呼叫 = 測試不 panic。LLM/MCP/SQLite 不在 router 內即為零呼叫證據。

### TC-009: probe/greeting 旁路

**Source**: AC-009 · **Level**: L4

```text
Given expensive-route bucket 已耗盡
When 依既有 bearer contract 呼叫 /health、/ready、/greeting
Then 三者回應 shape 與 status 不變(對照未啟用 limiter 的 baseline 回應)
```

### TC-010: audit 有界且最小化

**Source**: AC-010 · **Level**: L4

```text
Given 帶 raw IP header、session、bearer、prompt fixtures 的請求在 bucket 空時被拒
When limiter 發出拒絕決策
Then CapturingSink 恰收到一筆 RateLimitRejected;serde 序列化後不含任何 fixture 字串;SQLite 檔零寫入(POC limiter 根本不持 store handle,以型別隔離佐證)
```

**Implementation notes**: 复用 `src/runtime/audit.rs` 測試中既有 `CapturingSink` 模式。

### TC-011 / TC-ERR06: 已知超額結算凍結當月

**Source**: AC-011, ERR-006 · **Level**: L4

```text
Given actor A 本月 spent=16,000,000 且 g2 保留 4,000,000
When settle g2 權威金額 9,000,000,隨後 A 再 reserve 任意正額
Then snapshot spent=25,000,000、SettledWithOverage;新 reserve 回 BudgetExceeded 直到注入時間跨入下個台北月
```

### TC-012: 過期釋放 session ID

**Source**: AC-012 · **Level**: L4

```text
Given actor A 的 s2 最後 append 在注入時間 30 天又 1 秒之前
When A load s2,且 B 用同 ID 建 session
Then A 讀不到任何 summary;B 無衝突成為新空 session 的 owner
```

### TC-013: clear 釋放 ownership

**Source**: AC-013 · **Level**: L4

```text
Given actor A 的 s3 有兩筆 summary
When A clear s3 後 B 用 s3 建 session
Then session row 與 turns 已刪(raw SELECT 為空);B 無衝突取得新 session
```

### TC-014: auth 先於 limiter

**Source**: AC-014 · **Level**: L4

```text
Given burst_size=1 且 bucket 只剩一格
When 一個無效 bearer 請求打 /agent/stream,接著一個有效 bearer 請求
Then 第一個回 418(OpenAI 路由則 401)且 bucket 未扣;第二個被 admit(未回 429)
```

### TC-015: 429 envelope 釘死

**Source**: AC-015 · **Level**: L4

```text
Given bucket 已耗盡
When 認證請求分別打 /agent/stream 與 /v1/chat/completions
Then 前者 body 恰為 {"error": "<string>"};後者恰為 {"error":{"message":"<string>","type":"rate_limit_error"}};兩者 Retry-After 可 parse 為整數秒且 Cache-Control: no-store
```

### TC-ERR01: store 不可用

**Source**: ERR-001 · **Level**: L4

```text
Given 指向不存在父目錄的 db path(以及一個寫入非 SQLite bytes 的損壞檔)
When 開啟 repository
Then 回 StoreError::Unavailable;無任何 in-memory fallback 可用;錯誤字串不含 stored value
```

### TC-ERR03: 月額度超限

**Source**: ERR-003 · **Level**: L4

```text
Given actor A 本月 spent+reserved = 19,999,999
When reserve 2 micro-USD
Then BudgetExceeded 附 snapshot 與 next reset;raw SELECT 確認無新 reservation row
```

### TC-ERR04: 未知成本保留

**Source**: ERR-004 · **Level**: L4

```text
Given g3 為 pending reservation
When 以 unknown cost settle
Then ReconciliationRequired;g3 維持 pending 且原額持續計入本月
```

---

## 3. Boundary Tests

| Test ID | Boundary | Expected behavior | Source |
|---------|----------|-------------------|--------|
| TC-B01 | 空字串 / 超長(>128 chars)actor key 與 session ID | `StoreError::InvalidIdentifier`;summary 內容永不觸發 reject | FR-002 boundary |
| TC-B02 | settle 金額 < 保留額(under-settlement) | 實際額入 spent,差額同交易釋放回 remaining | FR-003 boundary |
| TC-B03 | 重複 reservation ID:同 actor/月/金額 vs 金額不同 | 前者冪等回原結果;後者 `ReservationMismatch` typed conflict | FR-003 boundary |
| TC-B04 | 月初 00:00:00 Asia/Taipei 首筆 reserve | 建新月 ledger,前月 ledger 原樣保留(raw SELECT 兩列) | FR-003 boundary |
| TC-B05 | 恰好 500 字元 summary(邊界不截斷)與 501 字元(截斷) | 500 原樣、501 截為 500 | FR-002 input |
| TC-B06 | reserve 金額 0 或負數 | typed error 拒絕(正整數約束) | FR-003 input |
| TC-B07 | `max_turns` 設 >5 | 建構時拒絕或 clamp 至 5(POC 上限) | FR-001 input |
| TC-B08 | limiter config `enabled=false`(預設) | 兩 route family 完全無 429 行為,回應與 baseline 相同 | FR-004 boundary / NFR Compatibility |

---

## 4. Capability-specific Tests

| Capability | Required tests | Evidence |
|------------|----------------|----------|
| `has_ui` | N/A (has_ui=false) | N/A |
| `has_api` | L4 router contract tests(TC-008~010、014~015、B08)以 `tower::ServiceExt::oneshot` 直打 router,零外部服務 | `cargo test --test rate_limit_ingress` |
| `typed_contracts` | 契約以 `cargo check` 驗證;store 契約另有 L4 行為測試 | `cargo check` |
| `has_e2e` | N/A (has_e2e=false) | N/A |

---

## 5. Test Matrix

| Level | Scope | Count | Command / Evidence |
|-------|-------|------:|--------------------|
| L1 靜態 + 文件 | `cargo check` + clippy + TC-007 runbook 驗收 | 1 | `cargo clippy -- -D warnings` |
| L2 Unit | `month.rs`(TC-006)、`sanitize.rs`(TC-003 的 unit 前哨)、config default | 3+ | `cargo test`(模組內 `#[cfg(test)]`) |
| L3 Component | N/A(has_ui=false) | 0 | N/A |
| L4 Integration | TC-001~005、008~015、ERR01/03/04、B01~B08 | 21 | `cargo test --test runtime_store_sqlite` + `cargo test --test rate_limit_ingress` |
| L5 E2E | N/A(has_e2e=false) | 0 | N/A |

---

## 6. Mock / Fixture Plan

| Dependency | Strategy | Contract source |
|------------|----------|-----------------|
| SQLite | 真實引擎 + temp 檔(非 mock;持久性是受測物) | spec schema v1 |
| 時鐘 | 注入 `now: DateTime<Utc>` 參數;無 wall-clock 依賴 | spec D-004 |
| AuditSink | 測試內 `CapturingSink`(既有模式) | `src/runtime/audit.rs` |
| Handler / LLM / MCP | panic-spy handler 取代真 handler;LLM/MCP 完全不進 router | spec Flow;PRD out of scope(POC 測試不呼叫 OpenRouter/MCP) |
| bearer token | 測試自訂 `GLOBAL_TOKEN` 環境(既有 router 測試慣例) | `src/server/route.rs` tests |
| 敏感 fixtures | email/IP/cookie/bearer/長文常數,集中於測試模組頂部 | PRD AC-003/AC-010 |

---

## Machine traceability gate

- [x] Every PRD AC maps to at least one test.(AC-001~015 → TC-001~015)
- [x] Every PRD ERR maps to at least one error or recovery test.(ERR-001~006 → TC-ERR01~06)
- [x] Every test has a source ID and level.
- [x] Capability-disabled sections are marked N/A rather than filled with fake assets.(L3/L5 N/A)
- [x] Mock or fixture data follows the Spec/API contract.(§6 全部指向 spec/既有程式碼)
- [x] Representative test evidence is executable or has a concrete command.(`cargo test --test <file>`)

---

## Related Documents

| Document | Link |
|----------|------|
| PRD | `docs/work/runtime-user-session-rate-limit/prd.md` |
| Spec | `docs/work/runtime-user-session-rate-limit/spec.md` |
