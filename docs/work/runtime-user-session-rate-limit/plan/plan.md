# Plan: runtime-user-session-rate-limit

## Plan

- Format: canonical-v2
- Concurrency: 1
- Intent: 依 spec v1.0.0 的 S1~S14 完成兩個獨立 POC slice(SQLite repositories;
  全域 burst limiter),測試先行,預設行為不變。
- Scope: `src/runtime/store/*`、`src/server/rate_limit.rs`、config/route/audit 接線、
  Slice 1 整合測試、Slice 2 router 測試、runbook、reference 文件 429 契約。
- Non-goals: 接線 `/agent/stream` 生產路徑、actor 傳遞、OpenRouter cost adapter
  (FU-003)、summary 產生邊界(FU-006)、多 replica。

## Sources

- user: goal-directive-implement-on-branch-2026-08-20
- local: spec.md#steps
- local: qa-plan.md#machine-traceability-gate

## Tasks

### T01 | slice-1-sqlite-repositories

- Status: done
- Depends On: []

#### Intent

S1~S7:依賴、store 契約、month/sanitize、SqliteRuntimeStore(schema v1、PRAGMA、
BEGIN IMMEDIATE、ownership/TTL/cap5、micro-USD ledger)、整合測試。

#### Expected Result

`cargo test --test runtime_store_sqlite` 全綠;AC-001~006、011~013 + ERR-001/003/004/006
+ TC-B01~B07 全覆蓋。

#### Definition of Done

- RED(todo! stubs)先於實作被觀察;GREEN 全綠。
- 關檔重開、跨連線併發、raw SELECT 資料最小化佐證皆入測。

#### Verification

- `cargo test --lib store::` → 19 passed(month 5 + sanitize 12 + config boundary)。
- `cargo test --test runtime_store_sqlite` → 23 passed / 0 failed(含兩輪 review 加測)。

### T02 | slice-2-burst-limiter

- Status: done
- Depends On: [T01]

#### Intent

S8~S12:`RateLimitRejected` audit variant、`[server.rate_limit]` config、
governor middleware、route 接線(auth 先於 limiter;probe/greeting 不掛)、router 測試。

#### Expected Result

AC-008~010、014~015 + ERR-005 + TC-B08 全綠;429 envelope 依 family 釘死。

#### Definition of Done

- RED(pass-through stub → 預期 429 失敗)先被觀察;GREEN 全綠。
- 測試位置偏差(in-crate 取代 tests/rate_limit_ingress.rs)記入 implement-report。

#### Verification

- `cargo test --lib rate_limit` → 8 passed / 0 failed。

### T03 | runbook-and-docs

- Status: done
- Depends On: [T01, T02]

#### Intent

S13 runbook(FR-004/AC-007)+ reference endpoints 文件補 opt-in 429 契約
(documentation impact)。

#### Expected Result

runbook 列出單 replica、persistent volume、備份、遷移邊界;
`agent-stream.md` 與 `chat-completions.md` 記載 429 行為。

#### Definition of Done

- TC-007 人工 read-through PASS。

#### Verification

- implement-report.md「QA Validation」TC-007 列 PASS。

### T04 | full-verification

- Status: done
- Depends On: [T01, T02, T03]

#### Intent

S14:fmt + clippy -D warnings + 全測試 + CI eval gates。

#### Expected Result

全部命令零失敗。

#### Definition of Done

- fresh evidence 記入 implement-report Gate 區塊。

#### Verification

- `cargo clippy --all-targets --all-features -- -D warnings` → clean。
- `cargo test` → 267 passed / 0 failed。
- `eval --pipeline-only` 3/3、`--response --replay` 2/2。

## Change Log

- 2026-08-20T18:00+08:00: Created alongside implementation(單 session 完成,
  plan 與執行同步落檔;RED/GREEN 證據見 implement-report.md)。
- 2026-08-20T18:25+08:00: T01~T04 全部 done。
- 2026-08-20T20:20+08:00: 兩輪 fable review 修正併入(counts refresh:267/0)。
