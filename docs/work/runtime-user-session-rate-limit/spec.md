# Runtime SQLite Persistence and Layered Rate-Limit POC 技術規格

**Story ID**: S-RUNTIME-SEC-01
**Spec 版本**: v1.0.0
**對應 PRD**: `docs/work/runtime-user-session-rate-limit/prd.md` @ v0.5.0
**Stage**: awaiting-approval

## Capability Snapshot

| Capability | Value | Evidence |
|---|---|---|
| `has_ui` | false | manifest `has_ui: false`; runtime storage + middleware only |
| `has_api` | true | `docs/reference/index.md`; this story adds opt-in `429` behavior on existing routes only |
| `typed_contracts` | true | `src/model.rs` + `cargo check`; new contracts live in `src/runtime/store/mod.rs` |
| `has_e2e` | false | manifest `has_e2e: false` |

## Version History / 版本歷史

| Version | Updated at | Change | Impact | PRD version | Author |
|---|---|---|---|---|---|
| v1.0.0 | 2026-08-13 15:55 | 初版:兩個 POC slices(SQLite repositories;ingress admission + audit) | 新模組 + opt-in middleware,預設行為不變 | PRD v0.5.0 | Claude (Fable 5) |

## Files

| Path | Operation | Purpose |
|---|---|---|
| `Cargo.toml` | MODIFY | 新增 `tokio-rusqlite`(bundled SQLite)與 `governor`(token-bucket 狀態引擎)。 |
| `src/runtime/mod.rs` | MODIFY | 宣告 `pub mod store`。 |
| `src/runtime/store/mod.rs` | NEW | Repository contracts、typed errors、`StoreConfig`、`ReserveOutcome`/`SettleOutcome`/`LedgerSnapshot`。 |
| `src/runtime/store/sqlite.rs` | NEW | `tokio-rusqlite` 實作:schema migration、PRAGMA、immediate transactions、全部 repository 方法。 |
| `src/runtime/store/sanitize.rs` | NEW | 固定順序淨化管線:redact 設定 pattern → normalize whitespace → truncate 至 per-field 上限。 |
| `src/runtime/store/month.rs` | NEW | Asia/Taipei(固定 +08:00)month key 與 next-reset 推導,輸入為注入的 UTC instant。 |
| `src/runtime/audit.rs` | MODIFY | `AuditEvent` 新增 `RateLimitRejected` variant(request_id、route、decision、retry_after_secs、policy_version)。 |
| `src/server/rate_limit.rs` | NEW | Opt-in 全域 burst middleware:`governor` RateLimiter + per-family `429` body + audit 事件。 |
| `src/server/mod.rs` | MODIFY | 宣告 `pub mod rate_limit`。 |
| `src/server/route.rs` | MODIFY | 兩個 sub-router 各自在 bearer gate 之內掛 opt-in limiter layer(auth 維持最外層)。 |
| `src/config.rs` | MODIFY | 新增 `[server.rate_limit]` 設定(`enabled`/`burst_size`/`refill_period_ms`),預設 disabled。 |
| `config/config.toml` | MODIFY | 加入註解過的 `[server.rate_limit]` 範例區塊(不動 `version = 1`)。 |
| `tests/runtime_store_sqlite.rs` | NEW | Slice 1 整合測試:AC-001~006、AC-011~013 + ERR-001~004、ERR-006。 |
| `tests/rate_limit_ingress.rs` | NEW | Slice 2 router 測試:AC-008~010、AC-014~015 + ERR-005。 |
| `docs/work/runtime-user-session-rate-limit/runbook.md` | NEW | FR-004 POC runbook:DB path/persistent volume/備份/單 replica/遷移限制/burst policy。 |

## Contracts

### Data Contracts (`typed_contracts` conditional)

新契約定義在 `src/runtime/store/mod.rs`(runtime 內部契約,不進 `src/model.rs` 的 HTTP DTO 層);驗證方式 `cargo check`。

| Contract | Source | Shape / fields | Evidence |
|---|---|---|---|
| `StoreConfig` | `src/runtime/store/mod.rs` | `db_path: PathBuf`, `max_turns: usize (=5)`, `ttl_days: u32 (=30)`, `busy_timeout_ms: u64 (=5000)`, `summary_char_limit: usize (=500)`, `redact_patterns: Vec<Regex>` | `cargo check` |
| `StoreError` | 同上 | `Unavailable(String)` / `OwnershipConflict` / `InvalidIdentifier(&'static str)` / `BudgetExceeded { snapshot }` / `ReservationMismatch` | `cargo check`;錯誤不含任何 stored value(NFR Security) |
| `SessionRecord` | 同上 | `session_id`, `actor_key`, `created_at_ms`, `last_append_ms` | `cargo check` |
| `TurnSummaryInput` | 同上 | 對齊既有 `SessionMemoryTurn` 欄位(`turn_id`, `user_summary`, `answer_summary`, `intent`, `metric`, `asset`, `time_range_label`, `option_id`);時間一律由注入 instant 決定 | 既有型別:`src/runtime/memory/store.rs:21` |
| `ReserveOutcome` | 同上 | `Reserved { snapshot }` / `BudgetExceeded { snapshot, next_reset_utc }` | `cargo check` |
| `SettleOutcome` | 同上 | `Settled` / `SettledWithOverage { overdrawn_by_micro }` / `AlreadySettled` / `ReconciliationRequired` | 對齊 PRD FR-003 settle result 四態 |
| `LedgerSnapshot` | 同上 | `month_key: String`, `spent_micro: i64`, `reserved_micro: i64`, `remaining_micro: i64 (clamp ≥0)`, `next_reset_utc` | `cargo check` |
| `RateLimitConfig` | `src/config.rs` | `enabled: bool (=false)`, `burst_size: NonZeroU32`, `refill_period_ms: NonZeroU64` | `cargo check`;serde default disabled |

**SQLite schema**(`src/runtime/store/sqlite.rs`,單一 migration v1;`schema_meta(version)` 記版本):

```sql
users(actor_key TEXT PRIMARY KEY, first_seen_ms INTEGER NOT NULL, last_seen_ms INTEGER NOT NULL);
sessions(session_id TEXT PRIMARY KEY, actor_key TEXT NOT NULL REFERENCES users(actor_key),
         created_at_ms INTEGER NOT NULL, last_append_ms INTEGER NOT NULL);
session_turns(session_id TEXT NOT NULL REFERENCES sessions(session_id) ON DELETE CASCADE,
              seq INTEGER NOT NULL, turn_id TEXT NOT NULL, user_summary TEXT NOT NULL,
              answer_summary TEXT NOT NULL, intent TEXT, metric TEXT, asset TEXT,
              time_range_label TEXT, option_id TEXT, created_at_ms INTEGER NOT NULL,
              PRIMARY KEY(session_id, seq));
monthly_budgets(actor_key TEXT NOT NULL, month_key TEXT NOT NULL,
                spent_micro INTEGER NOT NULL DEFAULT 0, reserved_micro INTEGER NOT NULL DEFAULT 0,
                PRIMARY KEY(actor_key, month_key));
budget_reservations(reservation_id TEXT PRIMARY KEY, actor_key TEXT NOT NULL,
                    month_key TEXT NOT NULL, reserved_micro INTEGER NOT NULL,
                    state TEXT NOT NULL CHECK(state IN ('pending','settled','reconcile')),
                    settled_micro INTEGER, created_at_ms INTEGER NOT NULL, settled_at_ms INTEGER);
```

PRAGMA(開檔即設):`foreign_keys=ON`、`journal_mode=WAL`、`synchronous=FULL`、`busy_timeout=<config>`。開檔或 migration 失敗 → `StoreError::Unavailable`,絕不 fallback 至記憶體(ERR-001)。

### API Contract (`has_api` conditional)

本 story 不新增 endpoint;唯一 API 變更是 opt-in `429`(PRD AC-015 釘死)。現有成功/失敗 shape 不變(`docs/reference/endpoints/agent-stream.md`)。

| Operation | Request | Success response | Error response | Auth / permission |
|---|---|---|---|---|
| 既有 expensive routes(`/agent/stream`, `/insight*`, `/report*`) | 不變 | 不變 | 新增:`429` + `Retry-After: <整數秒>` + `Cache-Control: no-store` + body `{"error": "rate limited: retry after <n>s"}`(沿用 `src/server/error.rs` 的 `ErrorBody`) | bearer 先於 limiter;401/418 不耗 bucket |
| `POST /v1/chat/completions` | 不變 | 不變 | 新增:`429` + 同上 headers + body `{"error": {"message": "rate limited: retry after <n>s", "type": "rate_limit_error"}}`(沿用 `src/server/openai.rs` 的 `OpenAiErrorBody`,新增常數 `ERR_RATE_LIMIT = "rate_limit_error"`) | OpenAI bearer gate(401)先於 limiter |
| `/health`, `/ready`, `/greeting` | 不變 | 不變 | 不套用 limiter(AC-009;guardrails 禁改 probe shape) | 既有 contract 不變 |

### UI Design (`has_ui` conditional)

N/A (has_ui=false)。

## Flow / Data Flow

```text
Slice 2(request path,opt-in):
  request → bearer gate(418/401;不耗 bucket)
          → rate_limit middleware(governor check;全域 key)
              ├─ 拒絕 → 429 + Retry-After + no-store + per-family body
              │         → tokio::spawn(AuditSink.write(RateLimitRejected{...}))(僅一筆;零 SQLite 寫入)
              └─ 允許 → JSON extractor → handler(既有路徑)

Slice 1(repository-only,不接 request path):
  caller(tests / 未來 integration slice)
    → SqliteRuntimeStore(tokio-rusqlite dedicated thread)
    → upsert_user(actor, now) → ensure_session(actor, session_id, now)[BEGIN IMMEDIATE]
    → append_turn:sanitize(redact→normalize→truncate 500)→ insert,cap 5、TTL 30d(以 last_append)
    → month::taipei_month_key(now_utc) → reserve(actor, rid, amount, now)[BEGIN IMMEDIATE;
        spent+reserved+amount ≤ 20_000_000 else BudgetExceeded]
    → settle(rid, actual, now)[BEGIN IMMEDIATE;=:Settled/<:釋放差額/>:SettledWithOverage、
        overdrawn 期間 reserve 一律拒絕/unknown:ReconciliationRequired]
```

**API-to-UI transformer**: N/A(has_ui=false)。

## Errors / Boundaries

| Case | Trigger | Expected behavior | Recovery |
|---|---|---|---|
| ERR-001 store unavailable | 路徑無法開啟、migration 失敗、busy timeout 超時 | `StoreError::Unavailable`;不 fallback、不外洩 stored value | 操作者修 path/volume/權限後重試 |
| ERR-002 ownership conflict | actor B 用 actor A 的未過期 session | `StoreError::OwnershipConflict`(零資料欄位) | 換新 session ID |
| ERR-003 budget exceeded | `spent+reserved+requested > 20_000_000` | `ReserveOutcome::BudgetExceeded` + snapshot + next reset;不建 reservation | 等下個台北月 |
| ERR-004 unknown final cost | settle 無權威金額 | reservation 維持 pending 原額計入;`ReconciliationRequired` | 未來 adapter/操作者補權威資料重結 |
| ERR-005 burst exceeded | governor bucket 空 | `429` + 整數 `Retry-After` + `no-store` + per-family body;一筆 audit;不進 handler/LLM/MCP/SQLite | 依 Retry-After 重試;調 config 政策 |
| ERR-006 known overage | settle 權威金額 > 保留額 | 同交易記實際 spent、關 reservation、回報 overage;overdrawn 期間 reserve 全拒 | 下個月自動恢復或人工對帳 |
| Boundary: 月界 | `2026-08-31T16:00:00Z` | month_key `2026-09`;reset `2026-09-30T16:00:00Z`(固定 +08:00) | N/A |
| Boundary: 結構性無效 ID | 空/超長 actor key 或 session ID | `StoreError::InvalidIdentifier`;summary 內容永不觸發 | caller 修正輸入 |
| Boundary: 過期/清除 | last_append + 30d < now;或 explicit clear | 對所有 actor 不可見;ID 可被任何 actor 重新宣告(單交易刪 session+turns) | N/A |

## Decisions

| ID | Decision | Rationale | Rejected | Reversibility |
|---|---|---|---|---|
| D-001 | 用 `governor` crate(token-bucket 引擎)寫薄 axum middleware,而非 `tower_governor` wrapper | middleware fn 是 async:可 await/spawn `AuditSink` 寫入、可依 route family 組 429 body;`tower_governor` 的 error handler 是同步閉包且與 axum 0.8 相容版本未驗證。`governor` 正是 tower-governor 底層的狀態引擎,PRD 的 process-local/restart-reset 語義完全一致 | `tower_governor`:sync error handler 難發 audit、雙 body 需兩份 config、版本相容風險 | 高:middleware 介面不變,可換回 wrapper |
| D-002 | Asia/Taipei 用 `FixedOffset::east_opt(8*3600)`,不加 `chrono-tz` | 台灣自 1980 後無 DST,固定 +08:00 與 PRD AC-006 驗算一致;省一個依賴 | `chrono-tz`:對本案無增益 | 高:`month.rs` 單點替換 |
| D-003 | `tokio-rusqlite`(bundled)做 async adapter | PRD 外部證據:cloneable handle 把 SQLite 閉包排到專用背景執行緒,不阻塞 Tokio worker(NFR Performance) | 直接 `rusqlite` + `spawn_blocking`:每 call 佔 blocking pool、無序列化保證 | 中:repository trait 後可換 backend(FU-002) |
| D-004 | 金額用 `i64` micro-USD + `BEGIN IMMEDIATE` 交易 | PRD 禁浮點;immediate 交易在單 DB 下序列化 read-check-write,防併發超訂(AC-004) | `REAL` 金額、`DEFERRED` 交易 | N/A(PRD 硬性要求) |
| D-005 | Slice 1 不接 `AppState`/startup;由測試與未來 slice 呼叫 | PRD FR-001 boundary:probe shape 不變因 POC 不接 startup;guardrails 禁改 probe | 直接接 `/agent/stream`:PRD 明列 out of scope | 高:整合屬下一 slice |
| D-006 | limiter layer 掛在各 sub-router 的 bearer layer 之「內」(axum builder 中先 `.layer(rate_limit)` 再 `.layer(auth)`,auth 維持最外) | PRD AC-014:未認證流量不得耗 bucket;axum 後掛的 layer 在外層先執行 | 全 Router 外層統一掛:會先於 auth 執行且波及 probe | 高:一行 layer 順序 |
| D-007 | `RateLimitRejected` audit 欄位僅 `request_id`, `route`, `decision`, `retry_after_secs`, `policy_version` | PRD AC-010/FR-005:不含 IP/session/actor/token/prompt/response;`request_id` 由 middleware 生成(uuid),與既有 `AuditCtx.request_id` 同語義 | 帶 headers/body 摘要:違反 PRD | N/A |

## Steps

| Step | Files | Action | Depends on | Estimate | Verification |
|---|---|---|---|---|---|
| S1 | `Cargo.toml` [MODIFY] | 加 `tokio-rusqlite`(features bundled)、`governor`;鎖與 axum 0.8/tokio 1 相容版本 | none | 15min | `cargo check` |
| S2 | `src/runtime/store/mod.rs` [NEW], `src/runtime/mod.rs` [MODIFY] | 契約:`StoreConfig`/`StoreError`/outcome 型別 + `SqliteRuntimeStore` 建構簽名(全方法帶注入 `now: DateTime<Utc>`) | S1 | 30min | `cargo check` |
| S3 | `src/runtime/store/month.rs` [NEW] | +08:00 month key/next reset;unit tests 含 AC-006 兩個時刻與 reset | S2 | 20min | `cargo test month` |
| S4 | `src/runtime/store/sanitize.rs` [NEW] | redact(設定 regex:email/IP/cookie/bearer)→ normalize whitespace → truncate 500;unit tests 用 AC-003 fixtures | S2 | 30min | `cargo test sanitize` |
| S5 | `src/runtime/store/sqlite.rs` [NEW] | 開檔 + PRAGMA + migration v1 + users/sessions/turns 方法(ownership、TTL、cap 5、clear、prune) | S2,S3,S4 | 90min | `cargo test` 子集(AC-001/002/003/012/013) |
| S6 | `src/runtime/store/sqlite.rs` [MODIFY] | ledger 方法:reserve/settle/snapshot(immediate tx、idempotency、overage、under-settle 釋放、unknown pending) | S5 | 90min | `cargo test` 子集(AC-004/005/011) |
| S7 | `tests/runtime_store_sqlite.rs` [NEW] | Slice 1 整合測試:含 close/reopen 同檔、兩 task 併發 reserve、statement-count 佐證 | S5,S6 | 60min | `cargo test --test runtime_store_sqlite` |
| S8 | `src/runtime/audit.rs` [MODIFY] | `RateLimitRejected` variant(D-007 欄位) | none | 15min | `cargo check` + 既有 audit tests |
| S9 | `src/config.rs` [MODIFY], `config/config.toml` [MODIFY] | `RateLimitConfig`(serde default disabled)+ 註解範例區塊 | none | 20min | `cargo test --test deployment_contract`(config 不變性) |
| S10 | `src/server/rate_limit.rs` [NEW], `src/server/mod.rs` [MODIFY] | governor middleware:全域 key、429 組裝(兩 family body + Retry-After + no-store)、audit spawn | S1,S8,S9 | 60min | `cargo check` |
| S11 | `src/server/route.rs` [MODIFY] | 兩 sub-router 依 D-006 順序掛 opt-in layer;probe/greeting 不掛 | S10 | 30min | `cargo test --test rate_limit_ingress` |
| S12 | `tests/rate_limit_ingress.rs` [NEW] | burst=1 確定性測試:AC-008/009/010/014/015 + handler-spy 證明拒絕不進 handler | S11 | 60min | `cargo test --test rate_limit_ingress` |
| S13 | `docs/work/runtime-user-session-rate-limit/runbook.md` [NEW] | FR-004/AC-007:volume、備份(停機 copy)、單 replica、遷移前提、burst 調參 | S7,S12 | 30min | 人工 read-through 對 AC-007 |
| S14 | 全部 | `cargo fmt` + `cargo clippy -- -D warnings` + `cargo test` 全綠 | S1–S13 | 20min | 三命令輸出 |

總估時:約 8 小時(兩個 slice 可獨立驗證:S2–S7 為 Slice 1,S8–S12 為 Slice 2)。

## Test Strategy

| Level | Scope | Command / evidence | Applicability |
|---|---|---|---|
| Unit | `month.rs`(AC-006)、`sanitize.rs`(AC-003 fixtures)、`RateLimitConfig` default | `cargo test`(模組 `#[cfg(test)]`) | required |
| Integration | Slice 1 repository:AC-001/002/004/005/011/012/013 + ERR-001~004/006;Slice 2 router:AC-008/009/010/014/015 + ERR-005 | `cargo test --test runtime_store_sqlite`、`cargo test --test rate_limit_ingress` | required(`has_api: true`;無外部服務——PRD 禁 POC 測試呼叫 OpenRouter/MCP) |
| Component | N/A | N/A | N/A(has_ui=false) |
| E2E | N/A | N/A | N/A(has_e2e=false) |

確定性:所有 repository 測試注入 `now`;limiter 測試用 `burst_size=1`、長 refill period,不依賴 wall-clock 補充。

## References

| Document | Path |
|---|---|
| PRD | `docs/work/runtime-user-session-rate-limit/prd.md` @ v0.5.0 |
| Architecture map | `.agent/knowledge/system-context.md` |
| API reference | `docs/reference/index.md`、`docs/reference/endpoints/agent-stream.md` |
| Design tokens | N/A (has_ui=false) |
| 既有 seam | `src/runtime/memory/store.rs`(SessionMemoryTurn)、`src/runtime/audit.rs`(AuditSink)、`src/server/error.rs` + `src/server/openai.rs`(429 envelopes)、`src/server/route.rs`(layer 順序) |
| External | tokio-rusqlite docs(dedicated background thread)、governor crate docs(token bucket)、RFC 6585 §4(429 不可 cache) |

## Gate 2 自檢

- [x] Capability Snapshot 與 manifest 一致(has_ui=false、has_api=true、typed_contracts=true、has_e2e=false)。
- [x] Files 覆蓋每個 FR:FR-001/002/003 → store 模組 + tests;FR-004 → runbook;FR-005 → rate_limit + route + audit + tests。
- [x] Contracts/API/UI 條件式章節已填寫或標示 N/A。
- [x] Flow / Data Flow、Errors(含全部 6 個 ERR)、Decisions(7 筆)、Steps(14 步,含估時/依賴/驗證)完整。
- [x] Test Strategy trace PRD AC-001~015 / ERR-001~006;capability false 項目為 N/A。
- [x] Gate 2 evidence 可被 QA / implement 下游讀取(schema、body 範例、layer 順序皆具體)。

Gate 2: PASS
Failed checks: none
Evidence: `docs/work/runtime-user-session-rate-limit/spec.md` v1.0.0
Reviewed at: 2026-08-13 15:55 +08:00
