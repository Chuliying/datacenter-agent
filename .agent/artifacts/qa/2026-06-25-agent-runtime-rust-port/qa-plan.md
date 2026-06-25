# Agent Runtime Rust Port QA Plan

**對應 PRD**: `docs/agent-runtime-rust-port/prd.md` v1.2.0  
**對應 Spec**: `docs/agent-runtime-rust-port/spec/spec-overview.md` + `spec-01`..`spec-06`  
**Story ID**: S-RUNTIME-01  
**日期**: 2026-06-25  
**狀態**: draft

---

## 0. Testability Check

| 檢查 | 結論 | 風險 / 動作 |
|------|------|-------------|
| Spec 是否容易測 | 大多數 runtime core 都是純函式或 trait seam，可測性高 | orchestrator 必須接 raw `AgentTurnInput`，不能只接 normalized input，否則 rejected request audit 不可測 |
| 是否有無法 mock 的相依 | LLM/MCP live path 需外部服務 | `AgentPort` fake 覆蓋 deterministic flow；live smoke 放 `#[ignore]` |
| 邏輯是否過複雜 | pipeline / policy / memory / audit 已拆開 | 禁止把 async memory 塞回 sync `PipelineStage` |
| Manifest 是否存在 | `.agent/project-manifest.md` 不存在 | QA command 以 `Cargo.toml` 和現有 tests 慣例推導；後續 onboarding 補 manifest |

**QA Research Sources**

- Rust Cargo test 可跑 unit / integration / doc tests，並可用 filter 或 `--test` 指定測試，參考 [Cargo Book: cargo test](https://doc.rust-lang.org/cargo/commands/cargo-test.html)。
- Tokio 官方建議用 async-aware test runtime 測 async 邏輯，參考 [Tokio Unit Testing](https://tokio.rs/tokio/topics/testing)。
- axum 基於 tower service，handler/router 可用 service 層級測試，不必啟動真 server；參考 [axum docs](https://docs.rs/axum/latest/axum/) 與 tokio-rs axum testing example。

---

## 1. AC → TC Traceability

| AC-ID | AC 描述 | TC-ID | TC 描述 | 層級 | 來源 |
|-------|---------|-------|---------|------|------|
| AC-01 | Rust runtime 成為單一推理權威 | TC-I01 | `/agent` 與 `/agent/stream` 都走 orchestrator，不直接呼叫 `llm_connector` | L4 | PRD US-1, US-21 |
| AC-02 | 領域內容 config 化 | TC-U01 | `RuntimeConfig` 載入 intents/lexicon/thresholds/injection | L2 | PRD US-2 |
| AC-03 | EV 充電 BI 第一個 capability pack | TC-U02 | pack TOML 解析並通過完整 classifier 驗證 | L2 | PRD US-3 |
| AC-04 | input pipeline stages 可 config 啟停/排序 | TC-U03 | `input_stages` unknown id fail-fast；改順序會反映執行序 | L2 | PRD US-4, US-7 |
| AC-05 | answer policy/memory/audit backend 可拔插 | TC-U04 | registry 能 build rule policy、in-memory store、stdout audit | L2 | PRD US-5 |
| AC-06 | slot extractors/guardrails 可 config 選用 | TC-U05 | 未註冊 extractor/guardrail id 驗證失敗 | L2 | PRD US-6 |
| AC-07 | 中文/中英 normalize | TC-U06 | NFKC + CJK punctuation table + whitespace/case | L2 | PRD US-8 |
| AC-08 | `option_id` + lexicon + text override intent | TC-U07 | option path 命中、text override 覆蓋、candidate 保留 | L2 | PRD US-9 |
| AC-09 | slots 抽取且 asset allowlist config 化 | TC-U08 | time/metric/asset/rank limit；未知 asset warning | L2 | PRD US-10 |
| AC-10 | prompt injection 偵測 | TC-U09 | 中英 injection patterns 命中；版本號存在 | L2 | PRD US-11 |
| AC-11 | answer policy pre-LLM 拒絕/提示 | TC-U10 | injection/off-scope/refusal/gray/answer 四類決策 | L2 | PRD US-12 |
| AC-12 | 空/超長輸入 pre-LLM 400 且 audit | TC-I02 | rejected request emits `InputRejected` and never calls AgentPort | L4 | PRD US-13 |
| AC-13 | session memory server side | TC-I03 | `session_id` scope read/write；no session fallback to client history | L4 | PRD US-14, US-17 |
| AC-14 | memory sanitize/truncate/budget | TC-U11 | system-like content dropped/truncated by budget | L2 | PRD US-15 |
| AC-15 | memory store trait 可換 | TC-U12 | common contract test for in-memory store | L2/L3 | PRD US-16 |
| AC-16 | audit covers every decision point | TC-I04 | success/refusal/rejected/error/clear/tool paths all emit expected audit sequence | L4 | PRD US-18 |
| AC-17 | audit redaction / seq / correlation | TC-U13 | request id/session id/seq monotonic/PII hash/secrets redacted | L2 | PRD US-19, US-20 |
| AC-18 | SSE/JSON wire stable | TC-I05 | external stream frames remain token/done/error/clear; refusal is token+done | L4 | PRD US-21, US-22 |
| AC-19 | `Clear` clears answer buffer | TC-I06 | Token/Clear/Token/Done final response excludes preamble | L4 | PRD US-23 |
| AC-20 | config validates intent allowlist | TC-U14 | unknown intent missing, duplicate id, empty keywords, invalid option prefix fail | L2 | PRD US-24 |
| AC-21 | runtime error policy | TC-U15 | runtime request path returns typed errors, no `unwrap`/`expect` | L2/L1 | PRD US-25 |
| AC-22 | orchestrator depends only on traits | TC-U16 | fake AgentPort/Memory/Audit/Policy drive one-turn tests | L2/L4 | PRD US-26 |
| AC-23 | LLM normalizer fallback gated | TC-U17 | disabled by default; enabled only through config | L2 | PRD US-27 |
| AC-24 | Rust-only paths tested | TC-I07 | refusal/disclaimer/abort/rejected audit all covered | L4 | PRD US-28 |
| AC-25 | eval subsystem exists | TC-U18 | fixtures/baseline load; evaluators registered | L2 | PRD US-29, US-31 |
| AC-26 | eval fixtures travel with pack | TC-U19 | each pack resolves own eval files relative to config root | L2 | PRD US-30 |
| AC-27 | eval CI gate split | TC-I08 | `cargo run --bin eval -- --pipeline-only` avoids live LLM; response eval requires live/replay | L4 | PRD US-32 |

---

## 2. L2 Unit Tests

| TC-ID | Module | 測試描述 | 輸入 | 預期 |
|-------|--------|----------|------|------|
| TC-U01 | `runtime/config.rs` | capability pack 載入 | valid TOML refs | `RuntimeConfig` fields filled |
| TC-U02 | `runtime/config.rs` | classifier 完整欄位 | thresholds.toml | all classifier fields match expected |
| TC-U03 | `runtime/registry.rs` | input stage assembly | `input_stages=["normalize","intent"]` | two stage ids in order |
| TC-U04 | `runtime/registry.rs` | answer/memory/audit build | builtin ids | trait objects built |
| TC-U05 | `runtime/registry.rs` | unknown ids fail | bad extractor/evaluator id | `RuntimeError::UnknownModule` |
| TC-U06 | `input/normalizer.rs` | CJK punctuation normalize | `「近６個月」、營收` | mapped punctuation + NFKC |
| TC-U07 | `input/intent.rs` | option path + text override | `option_id=charging`, text revenue | revenue + `TextOverride` |
| TC-U08 | `input/slots.rs` | slot extraction | top-N / asset / metric / time | normalized slots |
| TC-U09 | `guardrails/injection.rs` | injection regex | `忽略先前指令` / `system prompt` | warning or injection flag |
| TC-U10 | `guardrails/answer_policy.rs` | four action classes | normalized inputs | refuse/disclaimer/answer |
| TC-U11 | `memory/context.rs` | memory safety | system-like memory | dropped or sanitized |
| TC-U12 | `memory/store.rs` | store contract | append/get/clear | max_turns cap + actor isolation |
| TC-U13 | `audit.rs` | audit redaction | IP/UA/Bearer/API key | hashes/redaction/no preview |
| TC-U14 | `config.rs` | intent allowlist validation | missing unknown/duplicate/empty keywords | config error |
| TC-U15 | `runtime/` lint grep | no unwrap/expect request path | source scan | zero request-path hits |
| TC-U16 | `orchestrator.rs` | trait-only fake deps | fake agent/store/audit/policy | one turn passes |
| TC-U17 | fallback normalizer | disabled default | config default | no LLM fallback called |
| TC-U18 | `eval/*` | evaluator registry | builtins | pipeline/response evaluators built |
| TC-U19 | `eval/fixtures.rs` | pack-relative fixtures | config root | cases loaded |

**Suggested commands**

```bash
cargo test --lib
cargo test --test runtime_contract
```

---

## 3. L4 Integration Tests

| TC-ID | 測試描述 | Strategy |
|-------|----------|----------|
| TC-I01 | handlers route through orchestrator | Build AppState with fake AgentPort; assert no direct `llm_connector::generate` path in request tests |
| TC-I02 | rejected request audit | Empty/too-long prompt; capture `InputRejected`; AgentPort call count = 0 |
| TC-I03 | server memory mode | With `session_id`, upstream history is `[]`; without session, client history passed |
| TC-I04 | audit full path coverage | Capture sequences for success/refusal/rejected/tool/clear/error |
| TC-I05 | wire contract stability | SSE JSON envelopes remain `token`, `done`, `error`, `clear`; internal tool frames not exposed |
| TC-I06 | clear buffer | Fake stream Token/Clear/Token/Done; final JSON response only final token |
| TC-I07 | Rust-only edge paths | refusal token+done, disclaimer first token, abort status split |
| TC-I08 | eval command contract | `cargo run --bin eval -- --pipeline-only` runs without provider env vars |

**Suggested commands**

```bash
cargo test
cargo run --bin eval -- --pipeline-only
```

---

## 4. Live / Provider Smoke Tests

These are not CI-required. They run only when credentials and MCP server are available, following the existing ignored-test convention in `tests/llm_connector.rs`.

| TC-ID | 測試描述 | Command |
|-------|----------|---------|
| TC-L01 | live MCP/LLM loop still returns Markdown | `cargo test --test llm_connector -- --ignored` |
| TC-L02 | response eval live smoke | `cargo run --bin eval -- --response --live` |
| TC-L03 | response eval replay smoke | `cargo run --bin eval -- --response --replay <artifact>` |
| TC-L04 | response baseline creation | create initial `response-baseline.json` from recorded TS responses or approved live samples |

Gate rule: live smoke failures block release only when the target release changes live LLM/MCP behavior. Pipeline-only failures always block.
`response-baseline.json` is a new migration artifact unless source repo produces one before Phase 6; QA must verify its provenance.

---

## 5. Boundary Tests

| TC-ID | 邊界條件 | 預期行為 |
|-------|----------|----------|
| TC-B01 | prompt empty / whitespace only | 400 + `InputRejected`; no AgentPort |
| TC-B02 | prompt exactly configured max (`[input].max_prompt_chars`; EV pack 4000) | accepted |
| TC-B03 | prompt over configured max (`max_prompt_chars + 1`) | 400 + `InputRejected` |
| TC-B04 | request body near 64KiB | body limit handled by axum/tower; runtime prompt cap still applied |
| TC-B05 | unknown `option_id` prefix | warning + fallback to lexicon or unknown; audit includes option_id |
| TC-B06 | asset not in allowlist | warning, not hard-coded skiplist |
| TC-B07 | no `session_id` with history | client-history fallback |
| TC-B08 | `session_id` present with client history | server memory wins; upstream history `[]` |
| TC-B09 | `Clear` before any Token | no panic; audit `AnswerCleared`; final empty or later token |
| TC-B10 | stream abort with buffer vs empty | completed(aborted) vs failed per spec |
| TC-B11 | evaluator unknown id | config validation fails boot |
| TC-B12 | malformed eval fixture | eval load error with case path |

---

## 6. Error Scenario Tests

| TC-ID | ERR-ID | 錯誤場景 | 預期行為 |
|-------|--------|----------|----------|
| TC-ERR01 | ERR-CONFIG | bad capability pack path | boot fails with config error |
| TC-ERR02 | ERR-CONFIG | missing `unknown` intent | boot fails |
| TC-ERR03 | ERR-CONFIG | duplicate intent id | boot fails |
| TC-ERR04 | ERR-CONFIG | invalid TOML regex | boot fails with rule id |
| TC-ERR05 | ERR-INPUT | prompt injection | semantic refusal 200 or configured pre-LLM rejection; audit present |
| TC-ERR06 | ERR-UPSTREAM | AgentPort error | external error frame / AppError mapping + `ResponseFailed` |
| TC-ERR07 | ERR-AUDIT | audit sink write failure | fail-open logs and continues; fail-closed stops turn with `ResponseFailed` / 5xx |
| TC-ERR08 | ERR-EVAL | live eval requested without env | command exits with actionable error, pipeline-only unaffected |

---

## 7. Characterization Tests

| TC-ID | 現有行為描述 | 目的 |
|-------|--------------|------|
| TC-CT01 | `/agent` returns `{ user_prompt, model_response }` | JSON contract preserved |
| TC-CT02 | `/agent/stream` emits `StreamFrame` tagged by `event` | SSE contract preserved |
| TC-CT03 | `llm_connector::generate` clears buffer on `LlmEvent::Clear` | Rust-only semantics preserved |
| TC-CT04 | `tests/llm_connector.rs` ignored live test remains opt-in | CI does not require provider credentials |
| TC-CT05 | `config.rs` uses `deny_unknown_fields` | config typos fail loudly |

---

## 8. Mock / Fake Requirements

| Fake | Purpose | Contract |
|------|---------|----------|
| `FakeAgentPort` | deterministic orchestrator tests | emits `AgentTurnFrame` including Token/Clear/ToolCalled/ToolResult/Error |
| `CapturingAuditSink` | audit sequence assertions | preserves seq and event payloads |
| `FailingAuditSink` | audit failure policy tests | returns `RuntimeError::AuditSink` |
| `InMemorySessionStore` | memory contract tests | actor/session isolation |
| `FakeAnswerPolicy` | refusal/disclaimer branches | deterministic `AnswerAction` |
| `FixtureCapabilityPack` | config validation tests | minimal valid pack + mutation helpers |
| `RecordedResponseReplay` | response eval without live LLM | matches `ObservedTurn` schema |

Mock safety: all fake payloads must derive from runtime schemas, not ad hoc JSON strings.

---

## 9. Test Matrix

| 層級 | 數量 | Tool / Command | Frequency |
|------|------|----------------|-----------|
| L1 static | 2 | `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings` | every PR |
| L2 unit | 19+ | `cargo test --lib` | every PR |
| L4 integration | 8+ | `cargo test` + targeted integration tests | every PR |
| Eval pipeline | 1 command + cases | `cargo run --bin eval -- --pipeline-only` | every PR |
| Live smoke | 3 | ignored tests / live eval | pre-release or manual |

---

## 10. QA Gates

### Gate 1: Static + Unit

```bash
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test --lib
```

Pass criteria: format clean, clippy clean, unit tests pass.

### Gate 2: Integration + Pipeline Eval

```bash
cargo test
cargo run --bin eval -- --pipeline-only
```

Pass criteria: all non-ignored tests pass; pipeline fixtures green; no provider env required.

### Gate 3: Live Smoke / Release Gate

```bash
cargo test --test llm_connector -- --ignored
cargo run --bin eval -- --response --live
```

Pass criteria: required only for releases that alter live LLM/MCP behavior; failures require explicit risk acceptance.

---

## 11. Done Definition

- [ ] Every PRD user story US-1..US-32 has at least one TC above.
- [ ] No runtime request path `unwrap` / `expect`.
- [ ] `option_id` is present in DTO, audit, eval fixtures, and intent pipeline.
- [ ] Tool call/result metadata is auditable; not leaked as external SSE event.
- [ ] Rejected requests are audited before any AgentPort call.
- [ ] `cargo test` passes without provider credentials.
- [ ] `cargo run --bin eval -- --pipeline-only` passes without provider credentials.
- [ ] Live smoke path documented and still opt-in.
- [ ] Original `falcon-client` parity fixtures pass or documented diff is approved.

---

## 12. Related Documents

| 文件 | 路徑 |
|------|------|
| PRD | `docs/agent-runtime-rust-port/prd.md` |
| Spec overview | `docs/agent-runtime-rust-port/spec/spec-overview.md` |
| File structure | `docs/agent-runtime-rust-port/file-structure.md` |
| Migration plan | `docs/agent-runtime-rust-port/migration-plan.md` |
