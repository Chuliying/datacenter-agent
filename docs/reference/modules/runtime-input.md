# 模組：`runtime::input`

> ← 回 [模組總覽](./index.md) ｜ [專案主體](../index.md)
>
> **Source**：[`src/runtime/input/mod.rs`](../../../src/runtime/input/mod.rs)（“Deterministic input pipeline”）

## 職責
決定性（非 LLM）的輸入工程 pipeline：把原始使用者輸入正規化、分類 intent、抽取 slots，輸出帶信心分數的 `NormalizedInput`。領域內容（lexicon、allowlist、門檻）來自 config。

## 結構

子檔案分工見各檔 `//!`（[`mod.rs`](../../../src/runtime/input/mod.rs)、`normalizer.rs`、`intent.rs`、`slots.rs`、`pipeline.rs`）。本頁只記 doc comment 沒說的部分：

- intent 判定是三層合成：`option_id` option-path ＋ rule-lexicon 計分 ＋ text-override。
- **`pipeline.rs` 的執行順序是寫死的** `normalize → injection → intent → slots`；config
  `input_stages` 仍為宣告性、未據其分派（input_guard 是 orchestrator 前置步，不在本 pipeline 內）。

## 關鍵點
- **NFKC 不足**：`、「」` 等全形標點需手工對照表補。
- **asset 不得硬編**：未知資產走 config allowlist，未知標 warning（移植時修掉 TS 的硬編 skiplist）。
- intent 為對 allowlist 驗證過的 `String`，非編譯期 enum。
- **例外**：`slots.rs` 的時間範圍詞（近六個月／近三個月／這個月／本月／去年／今年）目前是寫死的 Rust 陣列，未走 config allowlist，與 asset 的處理方式不一致；`rank_limit` 正則只認英文 `top N`，無中文（如「前5名」）對應。

## 相關
- 輸出型別 `NormalizedInput` / `NormalizedSlots` → [schema](./runtime-schema.md)
- 低信心補強 → [llm_normalizer](./runtime-llm-normalizer.md)
- pipeline stage 由誰組裝 → [registry](./runtime-registry.md) · [config](./runtime-config.md)
- 結構防護（空／超長）→ [guardrails](./runtime-guardrails.md)
