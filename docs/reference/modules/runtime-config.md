# 模組：`runtime::config`

> ← 回 [模組總覽](./index.md) ｜ [專案主體](../index.md)
>
> **Source**：[`src/runtime/config.rs`](../../../src/runtime/config.rs)（“Runtime capability pack configuration”）

## 職責
載入能力包 TOML（領域資料 + assembly metadata）成 typed struct，並 `validate()`。它支援部分領域/config 變更，但目前不能單獨完成任意垂直應用切換。

## 載入內容
- 領域資料：`config/runtime/intents.toml`、`lexicon.toml`、`thresholds.toml`、`injection.toml`。
- 模組組裝段（決定 [registry](./runtime-registry.md) 組什麼）：
  - `[runtime.pipeline] input_stages = ["normalize","input_guard","injection","intent","slots"]` 注意：目前為**宣告性**：registry 會驗證這組 id，但 `InputPipeline` 尚未據其分派，實際固定執行 `normalize→injection→intent→slots`（見 [orchestrator](./runtime-orchestrator.md)）
  - `[runtime.answer_policy] backend = "rule"`
  - `[runtime.llm_normalizer] enabled, backend`
  - `[runtime.memory] enabled, backend`
  - `[runtime.audit] sink, failure_policy`
  - `[runtime.guardrails] enabled = [...]`
  - `[runtime.slots] extractors = [...]`

## 關鍵項
- `RuntimeConfig::load(refs, &registry)` — 入口。
- `assembly`（`RuntimeRefs`，含 `input_stages` / `answer_policy_backend` / `llm_normalizer_enabled`+`backend` / `memory_enabled`+`backend` / `audit_sink` / `audit_failure_policy`（`String`，經 `parse_audit_failure_policy` 只接受 `"fail-open"`/`"fail-closed"`）/ `guardrails` / `slot_extractors` / `pipeline_evaluators` / `response_evaluators` / `eval_fixtures` / `response_baseline`）— 模組組裝結果，均為一般 Rust struct 欄位，非僅 TOML 概念。
- `Thresholds` / `ConfidenceThresholds` / `ClassifierTuning` / `MemoryLimits` / `InputConfig` — 領域數值 struct（如 `answer_normal`、`answer_gray`、`option_path`、`llm_override_floor`、margin tiers、`max_prompt_chars`、`max_turns`、`max_memory_context_chars`），對應 `thresholds.toml` 等來源檔。
- `validate()` — 每個 `option_prefixes`／`[[intent]].id` 都在 allowlist、`unknown` 必須存在、keywords 非空、id 不重複；違反即中止開機。

## Validation gaps

目前沒有完整驗證 confidence 都在 `0..=1`、gray/normal ordering、`max_prompt_chars`/`max_turns` 為正數、margin tiers ordering/重複等 numeric invariants。另有多組 assembly ID 只被驗證，不代表 request path 會 dispatch。

AppState 在 `RuntimeConfig::load` 前解析 `RUNTIME_ENABLED`；明確 `false/0` 會跳過 runtime config/build，作為可用的 legacy rollback。

## 與頂層 config 的關係
頂層 [`src/config.rs`](../../../src/config.rs) 的 `Manifest` 透過 `[runtime]` 段引用本能力包檔（見 [專案主體](../index.md#4-啟動與組裝top-level-接線)）。

## 相關
- 消費者 → [registry](./runtime-registry.md)
- 驗證錯誤型別 → [error](./runtime-error.md)
