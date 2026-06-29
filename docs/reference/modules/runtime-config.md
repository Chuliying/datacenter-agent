# 模組：`runtime::config`

> ← 回 [模組總覽](./index.md) ｜ [專案主體](../index.md)
>
> **Source**：[`src/runtime/config.rs`](../../../src/runtime/config.rs)（“Runtime capability pack configuration”）

## 職責
載入能力包 TOML（領域資料 + assembly metadata）成 typed struct，並 `validate()`。它支援部分領域/config 變更，但目前不能單獨完成任意垂直應用切換。

## 載入內容
- 領域資料：`config/runtime/intents.toml`、`lexicon.toml`、`thresholds.toml`、`injection.toml`。
- 模組組裝段（決定 [registry](./runtime-registry.md) 組什麼）：
  - `[runtime.pipeline] input_stages = ["normalize","input_guard","injection","intent","slots"]` 注意：目前為**宣告性**：registry 會驗證這組 id，但 `InputPipeline` 尚未據其分派，實際只執行 `normalize→intent→slots`（見 [orchestrator](./runtime-orchestrator.md)）
  - `[runtime.answer_policy] backend = "rule"`
  - `[runtime.llm_normalizer] enabled, backend`
  - `[runtime.memory] enabled, backend`
  - `[runtime.audit] sink, failure_policy`
  - `[runtime.guardrails] enabled = [...]`
  - `[runtime.slots] extractors = [...]`

## 關鍵項
- `RuntimeConfig::load(refs, &registry)` — 入口。
- `assembly`（含 `audit_failure_policy`）— 模組組裝結果。
- `validate()` — 每個 `option_prefixes`／`[[intent]].id` 都在 allowlist、`unknown` 必須存在、keywords 非空、id 不重複；違反即中止開機。

## Validation gaps

目前沒有完整驗證 confidence 都在 `0..=1`、gray/normal ordering、`max_prompt_chars`/`max_turns` 為正數、margin tiers ordering/重複等 numeric invariants。另有多組 assembly ID 只被驗證，不代表 request path 會 dispatch。

`RuntimeConfig::load` 在 `RUNTIME_ENABLED` 判斷前由 AppState 呼叫，因此 flag false 仍可能因能力包無效而 startup fail。

## 與頂層 config 的關係
頂層 [`src/config.rs`](../../../src/config.rs) 的 `Manifest` 透過 `[runtime]` 段引用本能力包檔（見 [專案主體](../index.md#4-啟動與組裝top-level-接線)）。

## 相關
- 消費者 → [registry](./runtime-registry.md)
- 驗證錯誤型別 → [error](./runtime-error.md)
