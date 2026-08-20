---
output: docs/work/runtime-user-session-rate-limit/qa-report.md
stage: qa-report
slug: runtime-user-session-rate-limit
---

# Runtime SQLite Persistence + Layered Rate-Limit POC 驗收報告

**Spec**: `spec.md` v1.0.0 · **PRD**: `prd.md` v0.5.0 · **QA plan**: `qa-plan.md`
**Capabilities**: `has_ui=false` · `has_api=true` · `typed_contracts=true` · `has_e2e=false`
**驗收日期**: 2026-08-20

## 1. Pre-flight Checklist

- [x] Type/compile check:`cargo check`(經 clippy 全 targets)→ **0 errors**
- [x] Lint:`cargo clippy --all-targets --all-features -- -D warnings` → **clean**
- [x] Coding rules:store 走 runtime 模組慣例(typed errors、注入時鐘);
  limiter 沿用既有 middleware/envelope 慣例(`ErrorBody`/`OpenAiErrorBody`)

## 2. 測試執行

### Unit / Integration(L2/L4)

- 命令:`cargo test`(fresh,非採信舊 report)
- 結果:**260 passed / 0 failed**(lib 233:含 month 5、sanitize 8、rate_limit router 7;
  `runtime_store_sqlite` 整合 20;其餘既有 suites 全綠回歸)
- CI 同位:`cargo run --bin eval -- --pipeline-only` → 3/3;
  `--response --replay config/runtime/evals/replay-smoke.json` → 2/2
- 確定性:Slice 1 全部注入 `now`;Slice 2 用 `burst_size=1` + 1h refill,零 wall-clock 依賴

### E2E(L5)

- **N/A**(`has_e2e=false`;PRD 禁 POC 測試呼叫 OpenRouter/MCP)

## 3. AC 追溯(15/15 PASS)

| AC | 覆蓋 | 狀態 |
|---|---|---|
| AC-001 重開保留 | `ac001_committed_state_survives_reopen` | ✅ |
| AC-002 跨 actor 隔離 | `ac002_ownership_conflict_across_actors` | ✅ |
| AC-003 資料最小化 | `ac003_persisted_memory_is_minimized`(raw SELECT) | ✅ |
| AC-004 併發原子性 | `ac004_concurrent_reserve_never_oversubscribes` + 跨連線變體 | ✅ |
| AC-005 settle 冪等 | `ac005_idempotent_settle_charges_once` | ✅ |
| AC-006 台北月界 | `month::tests`(兩時刻 + reset + 年界) | ✅ |
| AC-007 POC 邊界文件 | runbook.md read-through(單 replica/volume/備份/opt-in/未接線) | ✅ |
| AC-008 拒絕先於昂貴工作 | `ac008_...` + `ac010_...`(counter-spy:handler 未被叫) | ✅ |
| AC-009 probe 旁路 | `ac009_probe_and_greeting_routes_are_not_limited` | ✅ |
| AC-010 audit 有界 | `ac010_...`(恰一筆;serde 無敏感 fixture;limiter 無 store handle) | ✅ |
| AC-011 超額凍結 | `ac011_over_settlement_overdraws_and_freezes_month` | ✅ |
| AC-012 過期釋放 ID | `ac012_...` + `reads_never_refresh_expiry` | ✅ |
| AC-013 clear 釋放 | `ac013_clear_releases_ownership` | ✅ |
| AC-014 auth 先於 limiter | `ac014_...`(418 標準路由)+ OpenAI 401 變體 | ✅ |
| AC-015 429 envelope 釘死 | `ac015_429_body_is_pinned_per_route_family` | ✅ |

### ERR 追溯(6/6 PASS)

| ERR | 覆蓋 | 狀態 |
|---|---|---|
| ERR-001 store 不可用 | `err001_unopenable_path` + `err001_corrupt_file` | ✅ |
| ERR-002 ownership conflict | AC-002 同測 | ✅ |
| ERR-003 月額度超限 | `err003_budget_exceeded_creates_no_reservation`(raw SELECT 零列) | ✅ |
| ERR-004 未知成本 | `err004_unknown_cost_requires_reconciliation` | ✅ |
| ERR-005 burst 超限 | AC-008/AC-015 同測(429 + headers + body) | ✅ |
| ERR-006 超額凍結 | AC-011 同測 | ✅ |

### Boundary(TC-B01~B08,8/8 PASS)

`invalid_identifiers_are_rejected`、`under_settlement_releases_unused_reservation`、
`duplicate_reservation_is_idempotent_only_on_full_match`、
`month_rollover_starts_fresh_ledger_and_keeps_history`、
`sanitize::char_limit_boundary_is_exact`、`reserve_rejects_zero_and_negative_amounts`、
`max_turns_above_poc_limit_is_refused_at_open`、`disabled_config_attaches_no_limiter`。

## 4. 已知限制(非驗收失敗;spec/PRD 已記錄)

1. Slice 1 為 repository-only:未接 `/agent/stream`,per-actor 月額度尚無 request 強制
   (FU-003 + actor 傳遞屬下一 slice)。
2. limiter 狀態 process-local:重啟歸零、多 replica 各自獨立(FU-002 前不得水平擴展)。
3. qa-plan 所列 `tests/rate_limit_ingress.rs` 改為 in-crate router 測試
   (`src/server/rate_limit.rs`),原因見 implement-report 偏差 #1。

## 驗收結論

**PASS** — 15/15 AC、6/6 ERR、8/8 boundary 全數通過;lint/typecheck/eval gates 乾淨;
可進入 release gate(PR #10 review)。
