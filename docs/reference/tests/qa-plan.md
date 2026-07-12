# datacenter-agent 現況測試與 Coverage

**QA 版本**：v1.4.0
**對應 Target PRD**：[`../prd.md`](../prd.md) v1.4.0
**對應 Spec**：[`../spec/spec.md`](../spec/spec.md) v1.4.0
**狀態**：Current test inventory；不是未實作測試的完成聲明  
**Source**：[`src/**` module tests](../../../src/lib.rs)、[`tests/runtime_contract.rs`](../../../tests/runtime_contract.rs)、[`tests/llm_connector.rs`](../../../tests/llm_connector.rs)、[`.github/workflows/runtime.yml`](../../../.github/workflows/runtime.yml)

> 本頁區分「test fn 存在」、「test 被一般 CI 執行」與「test 真正證明某個 production contract」。未覆蓋項目明確列為 gap，不以讀碼或 middleware 名稱冒充測試。

## 1. 2026-06-30 可重現快照

| Command | Result |
|---|---|
| `cargo fmt --all -- --check` | exit 0 |
| `cargo check` | exit 0 |
| `cargo clippy -- -D warnings` | exit 0 |
| `cargo test` | 92 passed、0 failed、2 ignored |
| `cargo run --bin eval -- --pipeline-only` | reported passed=3、failed=0；exit 0 |
| response replay smoke | reported passed=2、failed=0；exit 0 |
| synthetic failing replay | `tests/eval_cli.rs` 驗證 reported failed=1 時 process exit nonzero |
| `docker build -t datacenter-agent:blocker-fix .` + config presence check | exit 0；final image 含 top-level、prompt、runtime config |

兩個 ignored 項目：外部 LLM/MCP live test，以及一個 doc test。一般 `cargo test` 不執行 live test。

### v1.4.0 快照狀態（2026-07-11）

本次 v1.4.0 文件更新的環境**沒有 Rust toolchain**（`cargo`/`rustc` 不可用），因此**未重跑**測試套件；上表 92 passed 仍是 2026-06-30 最後一次 CI-verified run。v1.4.0 的變更以**原始碼檢視**確認，尚待在 CI 重跑取得新的聚合數字：

- 新增 2 個 member intent pipeline 測試（見 §4.1 TC-U17／TC-U18），存在於 `src/runtime/input/pipeline.rs`。
- `/report`、`/report/stream` 端點與 `REPORT_MAX_TOKENS`、`falcon-report` 輸出契約**目前沒有任何自動化測試**（見 §6、§8）。

在 CI 或具備 toolchain 的環境重跑 `cargo test` 後，應以新結果取代 2026-06-30 快照並更新計數。

## 2. 測試層級定義

| Level | 定義 | 現有例子 |
|---|---|---|
| L2 module unit | `src/**` 內 `#[cfg(test)]`，測單一 module/function | normalizer、policy、config、memory、audit |
| L3 component/handler | `src/**` 內以 fake dependency 測多元件或 wire mapping | orchestrator fake `AgentPort`、handler mapping |
| L4 crate integration/contract | `tests/**`，從 crate 公開 API 測契約 | `tests/runtime_contract.rs` |
| L5 external/manual | ignored live test、staging script、live eval | `tests/llm_connector.rs`、`staging-smoke.sh` |

原本把所有 orchestrator fake tests 都標成 L4、把 pipeline-only eval 標成 L5，會混淆 Cargo test location 與外部整合程度；v1.1.0 起使用上表。

## 3. AC → evidence 對照

| AC | Current contract | Automated evidence | Coverage verdict |
|---|---|---|---|
| AC-001 | legacy cap 2000；runtime cap 4000；runtime SSE error frame | legacy helper test + runtime input_guard 4000/4001/2001 tests | **partial**：沒有 Router-level REST/SSE status test |
| AC-002 | runtime intent.resolved → token → done | `orchestrator::streams_intent_resolved_then_tokens_then_done` | **partial**：fake AgentPort；真 provider transport test仍缺 |
| AC-003 | runtime 預設 on；false/0 rollback 且壞 config 不阻擋 legacy | `appstate::runtime_enabled_env_defaults_on_with_explicit_rollback`、`explicit_rollback_skips_invalid_runtime_config` | **covered at component level** |
| AC-004 | config 可調部分領域資料/元件，但不是任意 stage dispatch | config/registry tests | **partial**：builder existence 不等於 production request wiring |
| AC-005 | config 真正 dispatch stages/guardrails/extractors/evaluators | builder/config unit tests | **missing/partial**：stage order ignored、evaluators noop |
| AC-006 | injection request path + config policy thresholds | pipeline producer、orchestrator refusal/no-upstream/no-memory、policy config tests | **partial**：Router-level REST/SSE與numeric validation仍缺 |
| AC-007 | trusted actor memory scope與正確summary/budget contract | memory store/context unit tests | **missing/partial**：production actor None、full text、無tenant E2E |
| AC-008 | central audit redaction與所有terminal audit | audit helper/failure-policy tests | **missing/partial**：redaction無production caller、cancel/aborted terminal缺 |
| AC-009 | eval failure使process/CI nonzero | `tests/eval_cli.rs::reported_regression_exits_nonzero` | **covered** |
| AC-010 | decided auth/CORS/probe contract | code inspection only | **decision/test gap**：418、very-permissive、無deployment profile |
| AC-011 | runtime disabled隔離invalid runtime config | `appstate::explicit_rollback_skips_invalid_runtime_config` | **covered** |
| AC-012 | 每個完成claim有contract test與truthful docs | test inventory/doc link review | **partial**：沒有CI-enforcedclaim/status gate |
| AC-013 | Final LLM 無 MCP/DB/RAG access，只消費 validated Evidence Pack | none | **missing**：current LLM直接持有tools + McpHandle；相關types/modules不存在 |
| AC-014 | `/report` 產生合法 `falcon-report` HTML、數字源自 tool、共用 validation/error contract | member intent pipeline tests（TC-U17/U18） | **partial**：intent 已測；報表端點、`REPORT_MAX_TOKENS`、HTML/圖表輸出、繞過 runtime 皆無 test |
| AC-015 | Privacy Proxy 原文不出境、可逆還原、config 可開關且停用時 inert | none | **missing**：尚未實作；驗收見 privacy-proxy 功能 qa |

## 4. Rust test source inventory

qa source 驗證曾展開 79 個 Rust test function references；79/79 都有 test attribute 且出現在 `cargo test -- --list`。下表保留原 TC ID，並標記它真正能證明的範圍。

### 4.1 Validation / input

| TC | Source | Evidence boundary |
|---|---|---|
| TC-U01 | `handler::prompt_validation_rejects_empty_prompt` | legacy validation helper |
| TC-U02-L | `handler::prompt_validation_preserves_existing_2000_char_cap` | legacy 2000/2001 |
| TC-U02-R1 | `input_guard::accepts_prompt_at_runtime_limit` | runtime config limit 4000 |
| TC-U02-R2 | `input_guard::rejects_prompt_over_runtime_limit` | runtime 4001 rejects |
| TC-U02-R3 | `input_guard::accepts_approved_2001_char_parity_diff` | runtime 明確接受 2001 |
| TC-U10 | `normalizer::maps_fullwidth_and_cjk_punctuation` | fullwidth/CJK normalize |
| TC-U11 | `normalizer::collapses_whitespace_and_lowercases_ascii` | whitespace/ASCII normalize |
| TC-U12 | `pipeline::option_id_maps_to_option_path_intent` | option mapping |
| TC-U13 | `pipeline::text_override_beats_option_path_when_confident` | text override |
| TC-U14 | `pipeline::extracts_time_metric_asset_and_rank_slots` | current hard-coded pipeline functions |
| TC-U15 | `pipeline::unknown_option_prefix_warns_and_falls_back_to_text` | fallback warning |
| TC-U16 | `pipeline::unknown_asset_warns_without_hardcoded_allowance` | config asset behavior |
| TC-U17 | `pipeline::member_growth_prompt_classifies_as_member_and_is_answerable` | member intent 分類且清過 answer_normal gate |
| TC-U18 | `pipeline::revenue_growth_prompt_stays_on_revenue_not_member` | `營收成長` 歸 revenue，不誤入 member |

### 4.2 Guardrails

| TC | Source | Evidence boundary |
|---|---|---|
| TC-U20 | `injection::versioned_detector_matches_zh_and_en_injection` | detector 單元 |
| TC-U21 | `pipeline::detects_prompt_injection_and_warns` + `orchestrator::prompt_injection_is_refused_without_calling_upstream` | production producer→policy→zero-upstream |
| TC-U21b | `orchestrator::prompt_injection_refusal_is_not_persisted_to_memory` | rejected attack 不寫 memory |
| TC-U22 | `answer_policy::refuses_unknown_or_low_confidence_off_scope` | config-backed policy threshold |
| TC-U23 | `answer_policy::adds_disclaimer_for_gray_confidence` | config-backed policy threshold |
| TC-U24 | `answer_policy::answers_when_confidence_is_clear` | config-backed policy threshold |

### 4.3 Config / registry

| TC | Source | Evidence boundary |
|---|---|---|
| TC-U06 | `config::rejects_unknown_assembly_module_ids` | unknown ID validation |
| TC-U06b | `config::rejects_missing_unknown_intent` | required intent |
| TC-U06c | `config::rejects_invalid_injection_regex` | regex compile validation |
| TC-U30 | `config::loads_ev_capability_pack_from_default_config` | default files load |
| TC-U31 | `registry::builds_builtin_runtime_components` | builders exist；不證明 AppState 使用每個 builder |

### 4.4 Audit / memory

| TC | Source | Evidence boundary |
|---|---|---|
| TC-U07 | `audit::audit_writer_assigns_monotonic_seq_and_redacts_actor` | actor 有值時會 hash；production actor 仍 None |
| TC-U07b | `audit::redact_secrets_masks_known_tokens` | helper 單元；production sink 無 caller |
| TC-U40 | `memory::store::append_caps_at_max_turns` | turn retention cap |
| TC-U41 | `memory::store::clear_then_get_is_none` | clear semantics |
| TC-U42 | `memory::store::key_isolates_by_actor` | store 支援 actor；production actor_id 仍 None |
| TC-U43 | `memory::context::memory_sanitizes_system_like_content` | detector-based whole-field filtering 基本案例 |
| TC-U44 | `memory::context::memory_budget_exhausted_drops` | 超限整段 drop，不是 truncate |
| TC-U45 | `memory::context::memory_injected_on_followup` | context formatting |
| TC-U46 | `memory::context::memory_sanitizes_every_configured_injection_variant` | memory sanitizer 與 detector 規則一致 |

### 4.5 Eval / connector utilities

| TC | Source | Evidence boundary |
|---|---|---|
| TC-U50 | `eval::runner::pipeline_only_runs_default_pack_fixtures` | 3 fixtures、intent/slots only |
| TC-U51 | `eval::runner::replay_mode_reads_artifact_without_network` | replay offline |
| TC-U52 | `eval::runner::replay_mode_reports_response_regressions` | report 計數；不證明 process exit |
| TC-U53 | 3 個 `eval::baseline` validation tests | baseline schema |
| TC-U54 | 4 個 `bin/eval` parse tests | CLI argument parse |
| TC-I11 | `tests/eval_cli.rs::reported_regression_exits_nonzero` | failed report 的 process exit contract |
| TC-I12 | `tests/deployment_contract.rs::dockerfile_runtime_stage_copies_default_config_tree` | final runtime stage 的 COPY/CMD 與 source config tree 靜態契約；另有 local image build evidence |
| TC-U55 | 3 個 `llm_connector::agent` assemble/parse/hash tests | utility functions |
| TC-U56 | `llm_connector::agent` finish/tool completeness tests | finish reason、truncated JSON、blank identity、partial multi-call classification；helpers 接 production loop |

### 4.6 Handler / orchestrator / public contract

| TC | Source | Evidence boundary |
|---|---|---|
| TC-U03 | `handler::stream_mapping_preserves_external_sse_events` | legacy event mapping/filter |
| TC-U04 | `handler::runtime_route_selection_requires_built_enabled_runtime` | handler branch selection |
| TC-U04b | `appstate::runtime_enabled_env_defaults_on_with_explicit_rollback` + `explicit_rollback_skips_invalid_runtime_config` | cutover + startup rollback |
| TC-U05 | 3 個 `agent_response_*` tests | outcome→REST response |
| TC-C01 | `handler::turn_event_maps_to_external_stream_frame` | runtime event mapping |
| TC-I01 | `orchestrator::streams_intent_resolved_then_tokens_then_done` | fake AgentPort ordering |
| TC-I02 | `orchestrator::rest_consumes_same_orchestration_with_noop_emit` | shared core orchestration |
| TC-I03 | 2 個 `tests/runtime_contract.rs` stream serialization tests | public wire serialization |
| TC-I03b | 2 個 `tests/runtime_contract.rs` request serde tests | history default、metadata fields |
| TC-I04 | `orchestrator::refusal_does_not_call_upstream` | off-scope refusal；不證明 injection E2E |
| TC-I05 | 2 個 memory orchestration tests | fake/in-memory path |
| TC-I06 | 2 個 audit orchestration tests | fake sink event calls |
| TC-I07 | `orchestrator::clear_frame_clears_buffer` | core buffer |
| TC-I08 | `orchestrator::disclaimer_is_prepended_before_agent_tokens` | core ordering |
| TC-I09 | `orchestrator::upstream_error_always_fails_truncation_aborts` | fake AgentPort frames；不測 live adapter EOF |
| TC-I10 | 2 個 LLM normalizer orchestration tests | fake normalizer |

## 5. Non-test sources

| ID | Source type | Current status |
|---|---|---|
| TC-E01 | `tests/llm_connector.rs::live_generates_markdown_via_mcp` | test fn 存在、`#[ignore]`；一般 CI 不執行 |
| TC-E02 | `scripts/staging-smoke.sh` | script；只檢查基本 response keys/event allowlist，不覆蓋全部 AC |
| TC-E03 | eval CLI command | reported failure exit nonzero；evaluator quality scope仍有限 |
| TC-B05 | Router middleware reference | 不是 test；body >64 KiB 最終 status 未固定 |
| TC-CT01 | auth 418 讀碼 | 沒有 HTTP characterization test |
| TC-CT02 | legacy intent unknown 讀碼/handler mapping | 沒有 Router-level characterization test |

## 6. Boundary matrix

| Boundary | Current expected behavior | Automated evidence | Status |
|---|---|---|---|
| legacy 2000 | accepted | legacy helper test | covered at unit level |
| legacy 2001 | HTTP helper error | legacy helper test | no Router test |
| runtime 4000 | accepted | input_guard test | no handler test |
| runtime 4001 | runtime error | input_guard test | no REST/SSE status test |
| runtime 2001 | accepted | explicit parity-diff test | covered |
| `/report` prompt 2000 | legacy helper cap（非 runtime 4000） | none | gap（端點無 test） |
| body >64 KiB | Router rejects before handler; exact final mapping not pinned | none | gap |
| history omitted | `[]` | crate integration test | covered |
| memory max turns | oldest removed | store test | covered |
| provider partial EOF | missing/incompatible finish reason emits Error | finish-state unit contract；真 transport test缺 | partial |
| slow/disconnected SSE client | no bounded backpressure/cancel guarantee | none | **reliability gap** |

## 7. Error matrix

| Error | Existing evidence | Missing evidence |
|---|---|---|
| empty/overlong prompt | helper/input_guard unit tests | runtime SSE external status/frame |
| invalid auth | read code only | Router oneshot 418/body/header |
| upstream error | fake orchestrator test | real LlmAgentPort EOF/transport combinations |
| off-scope refusal | orchestrator fake | route-level REST/SSE contract |
| injection refusal | producer→consumer→zero-upstream/no-memory component tests | Router-level REST/SSE |
| config invalid | config unit tests + flag false invalid-ref startup regression | staging rollback smoke |
| audit sink fail | fail-open/fail-closed unit tests | handler external mapping |
| MCP semantic error | none across adapter boundary | `is_error=true` → model/audit outcome |
| eval regression | runner counts failure + process nonzero integration | richer evaluator semantics |
| Evidence Pack invalid/stale/tampered | none | schema、digest、freshness、classification、citation validation |
| capability/tool denied | none | gateway allowlist/scope/argument/cost policy與zero-execution assertion |
| indirect injection in evidence | none | untrusted-data boundary、Prompt Builder escaping/delimiters、Final LLM no-tool isolation |
| output cites missing evidence | none | Output Validator citation existence/coverage與bounded repair |

## 8. Required next tests

這些是 gap，不是已存在的 TC：

1. Router oneshot suite：auth scope、418 envelope、JSON rejection、64 KiB、REST/SSE prompt caps。
2. Runtime SSE lifecycle：bounded backpressure、disconnect cancellation、JoinError、terminal frame。
3. LLM adapter transport integration：EOF without finish reason、explicit finish、transport error、tool-call truncation（finish-state unit contract已有）。
4. MCP semantic result：`is_error` 保留到 `ToolResult.ok=false` 與 audit。
5. Runtime startup staging smoke：flag false + invalid runtime config。
6. Injection/answer policy Router-level REST/SSE contract + confidence numeric validation。
7. Audit redaction、actor extraction、memory tenant isolation。
8. Eval evaluator semantics：讓 config IDs 對應真實 evaluator，不以 noop 冒充。
9. Evidence Pack unit contract：required fields、version、digest、size/token budget、freshness/expiry、classification、partial/conflict states。
10. Capability Gateway component tests：allowed/denied tool、scope、argument schema、credential non-disclosure、timeout/cost limit、audit。
11. Prompt Builder golden tests：Skill Package + Evidence Pack + schema + memory deterministic composition，external content明確標untrusted。
12. Final LLM isolation test：`FinalLlmPort` API/compile dependency不接受tools、MCP/DB/RAG handles或credentials。
13. Output Validator tests：schema failure、unknown/missing citation、repair budget、insufficient-evidence refusal。
14. End-to-end controlled flow：fake Evidence Hub/Gateway產pack，Final LLM只收compiled prompt，published claims可回指evidence id。
15. `/report` 端點契約：Router-level auth/2000 cap/JSON rejection、`REPORT_MAX_TOKENS` 生效、`falcon-report` fenced-block 與 self-contained HTML 輸出、繞過 runtime 的行為斷言（不經 injection/answer policy/memory）。
16. Privacy Proxy（FR-015）：PII 偵測/checksum、tag 穩定與 streaming 還原容錯、殘留掃描 fail-closed、對照表加解密與 TTL、`[runtime.privacy]` 停用時 inert；細節見 privacy-proxy 功能 qa。

實作與先後依賴見 [程式修改計劃](../../../.agent/artifacts/plan/2026-06-29-runtime-correctness/implementation.md)。

## 9. QA gate

目前可誠實宣稱：

- check/clippy/fmt/test 通過（2026-06-30 fresh run）。
- 所有被 qa-plan 引用的 Rust test fn 都存在。
- deterministic pipeline/replay smoke 目前無 reported failure。
- eval reported regression 會使 process nonzero。
- injection refusal 不呼叫 upstream、不寫 memory，memory sanitizer 使用相同 detector + normalization。
- 明確 false/0 rollback 可略過損壞的 runtime capability config。

目前不可宣稱：

- 完整 config-selected evaluator quality gate 已落地。
- audit redaction、config-only pluggability 已 E2E 生效。
- 所有 route status/limits/timeouts 已有 contract test。
- live LLM/MCP 與 deployment probes 已驗收。
- Evidence Pack、Capability Gateway、Prompt Builder、Final LLM isolation或Output Validator已實作。
- `/report`、`/report/stream` 的端點行為（`falcon-report` HTML、圖表、`REPORT_MAX_TOKENS`、繞過 runtime）已有任何自動化 test。
- Privacy Proxy（FR-015）任何部分已實作或驗收。

## 10. Related documents

- [Reference root](../index.md)
- [Reverse PRD](../prd.md)
- [Technical spec](../spec/spec.md)
- [Privacy Proxy 功能文件](../features/privacy-proxy/prd.md)（FR-015，規劃中）
- [Code change plan](../../../.agent/artifacts/plan/2026-06-29-runtime-correctness/implementation.md)
