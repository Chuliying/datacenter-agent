# datacenter-agent 現況測試與 Coverage

**QA 版本**：v1.4.0（對應 crate 0.4.0，2026-08-20 同步）
**對應 Target PRD**：[`../prd.md`](../prd.md) v1.4.0
**對應 Spec**：[`../spec/spec.md`](../spec/spec.md) v1.4.0
**狀態**：Current test inventory；不是未實作測試的完成聲明  
**Source**：[`src/**` module tests](../../../src/lib.rs)、[`tests/runtime_contract.rs`](../../../tests/runtime_contract.rs)、[`tests/llm_connector.rs`](../../../tests/llm_connector.rs)、[`src/test_support.rs`](../../../src/test_support.rs)、[`.github/workflows/runtime.yml`](../../../.github/workflows/runtime.yml)

> 本頁區分「test fn 存在」、「test 被一般 CI 執行」與「test 真正證明某個 production contract」。未覆蓋項目明確列為 gap，不以讀碼或 middleware 名稱冒充測試。

## 1. 2026-08-20 可重現快照

| Command | Result |
|---|---|
| `cargo fmt --all -- --check` | exit 0 |
| `cargo clippy --all-targets --all-features -- -D warnings` | exit 0 |
| `cargo test` | **220 passed、0 failed、6 ignored** |
| `cargo run --bin eval -- --pipeline-only` | reported passed=3、failed=0；exit 0 |
| `cargo run --bin eval -- --response --replay config/runtime/evals/replay-smoke.json` | reported passed=2、failed=0；exit 0 |
| synthetic failing replay | `tests/eval_cli.rs` 驗證 reported failed=1 時 process exit nonzero（隨 `cargo test` 執行） |

六個 ignored 項目：5 個外部 LLM/MCP live test 與 1 個 doc test。一般 `cargo test` 不執行 live test。docker build 證據停在 v1.3.x（0.3.x image），未隨本次重驗。

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
| AC-001 | 單一 cap 4000（runtime prelude） | runtime input_guard 4000/4001 tests | **partial**：cap 的 Router-level status test 仍缺（0.4.0 起 validation error 已是 pre-stream HTTP status，見 spec §3.2）。~~legacy cap 2000~~ 已隨 `/insight`/`/report` 端點退役移除 |
| AC-002 | runtime intent.resolved → token → done | `turn::streams_intent_resolved_then_tokens_then_done` | **partial**：fake AgentPort；真 provider transport test仍缺 |
| AC-003 | runtime 預設 on；false/0 rollback 且壞 config 不阻擋 legacy | `appstate::runtime_enabled_env_defaults_on_with_explicit_rollback`、`explicit_rollback_skips_invalid_runtime_config` | **covered at component level** |
| AC-004 | config 可調部分領域資料/元件，但不是任意 stage dispatch | config/registry tests | **partial**：builder existence 不等於 production request wiring |
| AC-005 | config 真正 dispatch stages/guardrails/extractors/evaluators | builder/config unit tests | **missing/partial**：stage order ignored、evaluators noop |
| AC-006 | injection request path + config policy thresholds | pipeline producer、orchestrator refusal/no-upstream/no-memory、policy config tests | **partial**：Router-level REST/SSE與numeric validation仍缺 |
| AC-007 | trusted actor memory scope與正確summary/budget contract | memory store/context unit tests | **missing/partial**：production actor None、full text、無tenant E2E |
| AC-008 | central audit redaction與所有terminal audit | audit helper/failure-policy tests | **missing/partial**：redaction無production caller、cancel/aborted terminal缺 |
| AC-009 | eval failure使process/CI nonzero | `tests/eval_cli.rs::reported_regression_exits_nonzero` | **covered** |
| AC-010 | decided auth/CORS/probe contract | `route::unmatched_paths_do_not_leak_token_validity`、`retired_paths_return_404_regardless_of_authorization`、`per_group_timeout_layers_survive_a_merge`、`openai_timeout_returns_openai_error_envelope` | **partial + decision gap**：路由層 404 一致性與 per-group timeout/envelope 已有 router-level tests；418/401 auth **envelope 本身**仍只有讀碼，418 migration、CORS very-permissive 與 deployment profile 仍待決策 |
| AC-011 | runtime disabled隔離invalid runtime config | `appstate::explicit_rollback_skips_invalid_runtime_config` | **covered** |
| AC-012 | 每個完成claim有contract test與truthful docs | test inventory/doc link review | **partial**：沒有CI-enforcedclaim/status gate |
| AC-013 | Final LLM 無 MCP/DB/RAG access，只消費 validated Evidence Pack | none | **missing**：current LLM直接持有tools + McpHandle；相關types/modules不存在 |

## 4. Rust test source inventory

qa source 驗證曾展開 79 個 Rust test function references；79/79 都有 test attribute 且出現在 `cargo test -- --list`。下表保留原 TC ID，並標記它真正能證明的範圍。

### 4.1 Validation / input

| TC | Source | Evidence boundary |
|---|---|---|
| ~~TC-U01~~ | ~~`handler::prompt_validation_rejects_empty_prompt`~~ | **已刪除**：handler 版 `validate_prompt` 隨端點退役移除；空 prompt 現由 prelude 的 `input_guard` 擋 |
| ~~TC-U02-L~~ | ~~`handler::prompt_validation_preserves_existing_2000_char_cap`~~ | **已刪除**：2000-char cap 不再存在 |
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

### 4.2 Guardrails

| TC | Source | Evidence boundary |
|---|---|---|
| TC-U20 | `injection::versioned_detector_matches_zh_and_en_injection` | detector 單元 |
| TC-U21 | `pipeline::detects_prompt_injection_and_warns` + `turn::prompt_injection_is_refused_without_calling_upstream` | production producer→policy→zero-upstream |
| TC-U21b | `turn::prompt_injection_refusal_is_not_persisted_to_memory` | rejected attack 不寫 memory |
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
| ~~TC-U03~~ | ~~`handler::stream_mapping_preserves_external_sse_events`~~ | **已刪除**：legacy event mapping 隨 legacy path 移除；現行 `AgentEvent`→`StreamFrame` 映射由 `handler::insight_frames_stream_stages_tokens_and_a_clean_terminal` 覆蓋 |
| ~~TC-U04~~ | ~~`handler::runtime_route_selection_requires_built_enabled_runtime`~~ | **已刪除**：0.4.0 起 runtime 必要、無 legacy 分支；rollback 503 行為由 handler 直接回傳，尚無專屬測試（gap） |
| TC-U04b | `appstate::runtime_enabled_env_defaults_on_with_explicit_rollback` + `explicit_rollback_skips_invalid_runtime_config` | cutover + startup rollback |
| ~~TC-U05~~ | ~~3 個 `agent_response_*` tests~~ | **已刪除**：`AgentResponse` DTO 隨非串流端點退役移除 |
| TC-C01 | `turn::maps_llm_events_to_runtime_frames` + `tests/runtime_contract.rs` wire serialization | `LlmEvent`→`AgentTurnFrame` 與 `StreamFrame` wire 兩段映射 |
| TC-I01 | `turn::streams_intent_resolved_then_tokens_then_done` | fake AgentPort ordering |
| TC-I02 | `turn::rest_consumes_same_turn_with_noop_emit` | shared core turn（`run_agent_turn` 本身 dormant，此為 module test） |
| TC-I03 | 2 個 `tests/runtime_contract.rs` stream serialization tests | public wire serialization |
| TC-I03b | 2 個 `tests/runtime_contract.rs` request serde tests | history default、metadata fields |
| TC-I04 | `turn::refusal_does_not_call_upstream` | off-scope refusal；不證明 injection E2E |
| TC-I05 | 2 個 memory orchestration tests | fake/in-memory path |
| TC-I06 | 2 個 audit orchestration tests | fake sink event calls |
| TC-I07 | `turn::clear_frame_clears_buffer` | core buffer |
| TC-I08 | `turn::disclaimer_is_prepended_before_agent_tokens` | core ordering |
| TC-I09 | `turn::upstream_error_always_fails_truncation_aborts` | fake AgentPort frames；不測 live adapter EOF |
| TC-I10 | 2 個 LLM normalizer orchestration tests | fake normalizer |

### 4.7 Route-level（0.4.0 新增）

依 [`src/test_support.rs`](../../../src/test_support.rs) 的 stub MCP fixture（`tokio::io::duplex`）組出真 `AppState` 並對 `build_router` 做 oneshot——第一批打到組裝後 router 的測試。

| TC | Source | Evidence boundary |
|---|---|---|
| TC-R01 | `route::retired_paths_return_404_regardless_of_authorization` | 4 條退役路徑 × 3 種 Authorization 狀態都回 404 |
| TC-R02 | `route::surviving_paths_are_still_routed` | 5 條存活路徑不落入 fallback（只 pin routing，不 pin handler 行為） |
| TC-R03 | `route::unmatched_paths_do_not_leak_token_validity` | 任意未匹配路徑對 3 種 Authorization 狀態回應一致（token-validity oracle 迴歸） |
| TC-R04 | `route::per_group_timeout_layers_survive_a_merge` | merge 後 per-group timeout 各自存活（結構等價縮時版，不打真 router） |
| TC-R05 | `route::openai_timeout_returns_openai_error_envelope` | OpenAI 群逾時回 504 + `{"error":{"type":"server_error",...}}`（結構等價版） |

## 5. Non-test sources

| ID | Source type | Current status |
|---|---|---|
| TC-E01 | `tests/llm_connector.rs::live_generates_markdown_via_mcp` | test fn 存在、`#[ignore]`；一般 CI 不執行 |
| TC-E02 | `scripts/staging-smoke.sh` | script；只檢查基本 response keys/event allowlist，不覆蓋全部 AC |
| TC-E03 | eval CLI command | reported failure exit nonzero；evaluator quality scope仍有限 |
| TC-B05 | Router middleware reference | 不是 test；body >64 KiB 最終 status 未固定 |
| TC-CT01 | auth 418/401 讀碼 | envelope/body 仍沒有 HTTP characterization test；未匹配路徑對 Authorization 的**一致性**已由 TC-R03 覆蓋 |
| ~~TC-CT02~~ | ~~legacy intent unknown 讀碼~~ | **已失效**：legacy serving path 與 `AgentResponse` DTO 隨 0.4.0 移除 |

## 6. Boundary matrix

| Boundary | Current expected behavior | Automated evidence | Status |
|---|---|---|---|
| ~~legacy 2000/2001~~ | — | — | **已移除**：cap 收斂為 runtime 單一來源 4000 |
| runtime 4000 | accepted | input_guard test | no handler test |
| runtime 4001 | pre-stream HTTP 400（0.4.0 起，非 200+frame） | input_guard test | no route-level status test |
| runtime 2001 | accepted | explicit parity-diff test | covered |
| body >64 KiB | Router rejects before handler; exact final mapping not pinned | none | gap |
| unmatched path | 404、空 body、與 Authorization 無關 | TC-R01/R03 | covered |
| history omitted | `[]` | crate integration test | covered |
| memory max turns | oldest removed | store test | covered |
| provider partial EOF | missing/incompatible finish reason emits Error | finish-state unit contract；真 transport test缺 | partial |
| slow/disconnected SSE client | no bounded backpressure/cancel guarantee | none | **reliability gap** |

## 7. Error matrix

| Error | Existing evidence | Missing evidence |
|---|---|---|
| empty/overlong prompt | helper/input_guard unit tests | runtime SSE external status/frame |
| invalid auth | TC-R03（未匹配路徑一致性）；envelope 仍 read code only | Router oneshot 418/401 envelope body/header |
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

1. Router oneshot suite：**部分完成**（TC-R01~R05 覆蓋 routing、404 一致性、per-group timeout/envelope）；仍缺 auth 418/401 envelope body、JSON rejection、64 KiB、prompt cap 的 route-level status。
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

實作與先後依賴見 [程式修改計劃](../../../.agent/artifacts/plan/2026-06-29-runtime-correctness/implementation.md)。

## 9. QA gate

目前可誠實宣稱：

- clippy/fmt/test/pipeline-eval/replay-smoke 通過（2026-08-20 fresh run，220 passed/0 failed）。
- 退役路徑回 404、未匹配路徑不洩漏 token 有效性、per-group timeout 與 OpenAI timeout envelope——都有 router-level 迴歸測試。
- 所有被 qa-plan 引用的 Rust test fn 都存在。
- deterministic pipeline/replay smoke 目前無 reported failure。
- eval reported regression 會使 process nonzero。
- injection refusal 不呼叫 upstream、不寫 memory，memory sanitizer 使用相同 detector + normalization。
- 明確 false/0 rollback 可略過損壞的 runtime capability config。

目前不可宣稱：

- 完整 config-selected evaluator quality gate 已落地。
- audit redaction、config-only pluggability 已 E2E 生效。
- 所有 route status/limits 已有 contract test（auth envelope、JSON rejection、64 KiB、prompt cap status 仍缺）。
- live LLM/MCP 與 deployment probes 已驗收。
- Evidence Pack、Capability Gateway、Prompt Builder、Final LLM isolation或Output Validator已實作。

## 10. Related documents

- [Reference root](../index.md)
- [Reverse PRD](../prd.md)
- [Technical spec](../spec/spec.md)
- [Code change plan](../../../.agent/artifacts/plan/2026-06-29-runtime-correctness/implementation.md)
