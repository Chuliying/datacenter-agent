# 模組：`runtime::llm_normalizer`

> ← 回 [模組總覽](./index.md) ｜ [專案主體](../index.md)
>
> **Source**：[`src/runtime/llm_normalizer.rs`](../../../src/runtime/llm_normalizer.rs)（“Optional LLM-backed input normalization seam”）

## 職責
可選的 LLM-backed 輸入正規化 seam：在決定性 [input pipeline](./runtime-input.md) **之後**、[answer policy](./runtime-guardrails.md) **之前**，補強低信心／灰區的分類。**不取代** deterministic pipeline。

## 關鍵項
- `trait LlmInputNormalizer` —— 介面 seam。
- 由 `[runtime.llm_normalizer] enabled / backend` config gate，**預設關閉**（`disabled`）。
- 開啟時由 [orchestrator](./runtime-orchestrator.md) 在 rule pipeline 後呼叫。

## 設計意圖
保留「之後可加 LLM 補強」的接線，但預設不動成本／延遲；屬本輪 out-of-scope 的即時 fallback，只先立介面 + gate。

## 相關
- 上游 → [input](./runtime-input.md)
- 下游決策 → [guardrails · answer_policy](./runtime-guardrails.md)
- 組裝 → [registry](./runtime-registry.md)
