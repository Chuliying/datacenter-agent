# Implement Report — S-RUNTIME-SEC-01

Skill: implement
Executed At: 2026-08-20 18:20 +08:00
Execution Mode: team-feature
Spec: `spec.md` v1.0.0 (PRD v0.5.0, qa-plan machine gate 15/15)
Branch: `codex/runtime-user-session-rate-limit`

## Files

| Path | Op | Spec step |
|---|---|---|
| `Cargo.toml` | MODIFY | S1(`tokio-rusqlite 0.7`(bundled)、`governor 0.10`;dev:`tempfile`、`tokio-rusqlite`) |
| `src/runtime/mod.rs` | MODIFY | S2 |
| `src/runtime/store/mod.rs` | NEW | S2 契約 |
| `src/runtime/store/month.rs` | NEW | S3 |
| `src/runtime/store/sanitize.rs` | NEW | S4 |
| `src/runtime/store/sqlite.rs` | NEW | S5/S6 |
| `tests/runtime_store_sqlite.rs` | NEW | S7(23 tests) |
| `src/runtime/audit.rs` | MODIFY | S8(`RateLimitRejected`) |
| `src/config.rs` | MODIFY | S9(`[server.rate_limit]`,deny_unknown_fields) |
| `config/config.toml` | MODIFY | S9(註解範例區塊) |
| `src/server/rate_limit.rs` | NEW | S10 + S12 tests(8 tests,in-crate) |
| `src/server/mod.rs` | MODIFY | S10 |
| `src/server/route.rs` | MODIFY | S11(limiter 在 bearer 之內;probe/greeting 不掛) |
| `src/server/openai.rs` | MODIFY | S10(`ERR_RATE_LIMIT`) |
| `src/appstate.rs`、`src/test_support.rs` | MODIFY | S11 接線(AppState 增 `rate_limit` 欄位) |
| `docs/work/runtime-user-session-rate-limit/runbook.md` | NEW | S13 |
| `docs/reference/endpoints/agent-stream.md`、`chat-completions.md` | MODIFY | documentation impact(opt-in 429 契約) |

## RED → GREEN cycles

| Cycle | RED evidence | GREEN evidence |
|---|---|---|
| S3/S4 month+sanitize | `cargo test --lib store::` → 12 failed(全部 `not yet implemented` panic) | 同命令 15 passed |
| S5/S6 sqlite store | `cargo test --test runtime_store_sqlite` → 16 failed(todo! stubs) | 同命令 16 passed |
| S10–S12 burst limiter | `cargo test --lib rate_limit` → 4 failed(預期 429,得 400/200;pass-through stub) | 同命令 6 passed |
| TC-B07 max_turns 上限 | `cargo test --test runtime_store_sqlite max_turns` → 1 failed(開檔未拒絕) | 同命令 1 passed |

qa-plan 補測(對既有已測行為的加深覆蓋,非新行為):TC-ERR03、TC-B05、TC-B06、
ERR-001 corrupt-file、AC-014 OpenAI-401 → 加入後全綠。

## 對 spec 的偏差(4 筆,均記錄理由)

1. **Slice 2 router 測試位置**:spec 列 `tests/rate_limit_ingress.rs`;實際放
   `src/server/rate_limit.rs` 的 `#[cfg(test)]`。原因:`test_support::app_state()`
   (唯一能組出 `AppState` 的 fixture)是 `#[cfg(test)]` in-crate,integration
   test 組不出 `AppState`(`McpHandle` 需完成 MCP handshake)——與
   `tests/route_contract.rs` 檔頭記載的既有限制一致,route.rs 的 router 測試同樣在 in-crate。
2. **`StoreError::BudgetExceeded` variant 未實作**:spec 契約表列了它,但 PRD
   FR-003 把「budget exceeded」定義為 reserve 的**結果**(`ReserveOutcome::BudgetExceeded`
   帶 snapshot + next reset),不是錯誤;保留未使用 variant 會被
   `clippy -D warnings`(dead_code)擋下。
3. **audit 寫入 await inline,非 `tokio::spawn`**:spec flow 圖寫 spawn;D-001 的
   rationale 允許「await/spawn」。await inline 讓 AC-010 測試無 race、且 sink
   本身輕量;拒絕路徑非熱路徑。
4. **AppState 增 `rate_limit` 欄位**:spec Files 表未列 `appstate.rs`,但
   `build_router(state)` 需要 config 來源;這是最小接線(test_support 同步補預設)。

qa-plan TC-001 實作註記寫「repo 無 tempfile dep」——已過時,本次加了 dev-dep
`tempfile`(較 `std::env::temp_dir()` 手工方案安全,自動清理)。

## QA Validation(qa-plan traceability → 實測)

| TC | 測試 | 結果 |
|---|---|---|
| TC-001 | `ac001_committed_state_survives_reopen` | PASS |
| TC-002/ERR02 | `ac002_ownership_conflict_across_actors` | PASS |
| TC-003 | `ac003_persisted_memory_is_minimized`(raw SELECT 佐證)+ sanitize units | PASS |
| TC-004 | `ac004_concurrent_reserve_never_oversubscribes` + 跨連線加測 `ac004_cross_connection_concurrent_reserve_never_oversubscribes` | PASS |
| TC-005 | `ac005_idempotent_settle_charges_once` | PASS |
| TC-006 | `month::tests`(5 units,含 AC-006 兩時刻與年界) | PASS |
| TC-007 | runbook.md 人工 read-through:單 replica/persistent volume/VACUUM INTO 備份/opt-in/未接線邊界 全列 | PASS |
| TC-008/ERR05 | `ac008_burst_rejection_happens_before_the_handler` + `ac010`(counter-spy 證 handler 未被叫) | PASS |
| TC-009 | `ac009_probe_and_greeting_routes_are_not_limited` | PASS |
| TC-010 | `ac010_rejection_audits_once_with_bounded_fields_and_skips_handler`(CapturingSink;serde 序列化無 fixture;limiter 無 store handle = 型別級零 SQLite) | PASS |
| TC-011/ERR06 | `ac011_over_settlement_overdraws_and_freezes_month` | PASS |
| TC-012 | `ac012_expiry_is_deterministic_and_releases_the_id` + `reads_never_refresh_expiry` | PASS |
| TC-013 | `ac013_clear_releases_ownership` | PASS |
| TC-014 | `ac014_unauthenticated_traffic_cannot_starve_the_bucket` + `ac014_openai_unauthenticated_rejects_401_without_consuming` | PASS |
| TC-015 | `ac015_429_body_is_pinned_per_route_family` | PASS |
| TC-ERR01 | `err001_unopenable_path_is_unavailable` + `err001_corrupt_file_is_unavailable` | PASS |
| TC-ERR03 | `err003_budget_exceeded_creates_no_reservation`(raw SELECT 零列佐證) | PASS |
| TC-ERR04 | `err004_unknown_cost_requires_reconciliation` | PASS |
| TC-B01 | `invalid_identifiers_are_rejected` | PASS |
| TC-B02 | `under_settlement_releases_unused_reservation` | PASS |
| TC-B03 | `duplicate_reservation_is_idempotent_only_on_full_match` | PASS |
| TC-B04 | `month_rollover_starts_fresh_ledger_and_keeps_history` | PASS |
| TC-B05 | `sanitize::tests::char_limit_boundary_is_exact` | PASS |
| TC-B06 | `reserve_rejects_zero_and_negative_amounts` | PASS |
| TC-B07 | `max_turns_above_poc_limit_is_refused_at_open` | PASS |
| TC-B08 | `disabled_config_attaches_no_limiter` | PASS |

## Gate

```text
type-check: PASS  (cargo check,含 clippy 全 targets)
lint:       PASS  (cargo clippy --all-targets --all-features -- -D warnings:0 warnings)
test:       PASS  (cargo test:267 passed / 0 failed;含 23 條 sqlite 整合、8 條 limiter router;兩輪 review 修正後 fresh)
eval:       PASS  (cargo run --bin eval -- --pipeline-only:3/3;--response --replay:2/2)
UI mockup:  N/A   (has_ui=false)
```

## Execution Checklist

```text
Skill: implement
Executed At: 2026-08-20 18:20
Files: 見上表
Report: docs/work/runtime-user-session-rate-limit/implement-report.md

Steps:
Step 1: 環境與邊界 - PASS
  Evidence: mode=team-feature;PRD v0.5.0 / spec v1.0.0 / qa-plan 皆已讀;
  manifest test_cmd=cargo test、lint=clippy -D warnings、typecheck=cargo check
Step 2: RED-GREEN-REFACTOR - PASS
  Evidence: 4 個 RED→GREEN cycle,RED 皆為 observed failing test(non-compile);
  REFACTOR=cargo fmt 後全綠
Step 3: 專案模式與完整驗證 - PASS
  Evidence: 267/267 tests、clippy 0 warnings、CI eval gates 同位;
  documentation impact 已落 docs/reference/endpoints/*
Gate:
  type-check: PASS
  lint: PASS
  test: PASS (267/267)
  UI mockup: N/A
```
