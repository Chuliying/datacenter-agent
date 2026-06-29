# 模組：`runtime::input`

> ← 回 [模組總覽](./index.md) ｜ [專案主體](../index.md)
>
> **Source**：[`src/runtime/input/mod.rs`](../../../src/runtime/input/mod.rs)（“Deterministic input pipeline”）

## 職責
決定性（非 LLM）的輸入工程 pipeline：把原始使用者輸入正規化、分類 intent、抽取 slots，輸出帶信心分數的 `NormalizedInput`。領域內容（lexicon、allowlist、門檻）來自 config。

## 子檔案

| 檔案 | 職責 |
|---|---|
| [`normalizer.rs`](../../../src/runtime/input/normalizer.rs) | 文字正規化：NFKC + **手工全形標點對照表**、空白、大小寫 |
| [`intent.rs`](../../../src/runtime/input/intent.rs) | intent 分類：`option_id` option-path + rule-lexicon 計分 + text-override |
| [`slots.rs`](../../../src/runtime/input/slots.rs) | slot 抽取：time range / metric / asset / rank limit；asset 未知判定走 config allowlist |
| [`pipeline.rs`](../../../src/runtime/input/pipeline.rs) | `InputPipeline::run_with_config` 串接，實際執行 `normalize → intent → slots` 注意：config `input_stages` 為宣告性、目前未據其分派（無 injection、input_guard 為 orchestrator 前置步） |

## 關鍵點
- **NFKC 不足**：`、「」` 等全形標點需手工對照表補。
- **asset 不得硬編**：未知資產走 config allowlist，未知標 warning（移植時修掉 TS 的硬編 skiplist）。
- intent 為對 allowlist 驗證過的 `String`，非編譯期 enum。

## 相關
- 輸出型別 `NormalizedInput` / `NormalizedSlots` → [schema](./runtime-schema.md)
- 低信心補強 → [llm_normalizer](./runtime-llm-normalizer.md)
- pipeline stage 由誰組裝 → [registry](./runtime-registry.md) · [config](./runtime-config.md)
- 結構防護（空／超長）→ [guardrails](./runtime-guardrails.md)
