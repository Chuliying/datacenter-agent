# 模組：`runtime::eval` — partial

> ← [Modules](./index.md)  
> **Source**：[`src/runtime/eval/`](../../../src/runtime/eval/mod.rs)、[`src/bin/eval.rs`](../../../src/bin/eval.rs)、[`.github/workflows/runtime.yml`](../../../.github/workflows/runtime.yml)

## 現有 modes

| Mode | Current implementation |
|---|---|
| pipeline-only | 讀 default fixtures，直接跑 `InputPipeline::run_with_config`，目前 3 cases，只比較 intent/slots |
| response replay | 讀 recorded artifact，執行 deterministic response checks |
| response live | provider-backed behavior；需外部設定，不在一般測試執行 |

`Evaluator` trait 與 `NoopEvaluator` 存在；registry 對每個 evaluator ID 都建立 noop，runner 沒有執行一套 config-selected grounding/insight/hallucination/LLM-judge pipeline。

## CLI exit contract

`run(mode)` 回 Err 或 `EvalReport.failed > 0` 時 CLI exit 1。integration test 以 synthetic failing replay 固定 process nonzero，workflow 的 eval steps 可阻擋 reported regression。

目前可宣稱 fixtures/replay mechanics 存在；不可宣稱已實作 LLM judge、grounding 或 hallucination evaluation。Target 見 [PRD FR-010](../prd.md)；工作見 [plan I06](../../../.agent/artifacts/plan/2026-06-29-runtime-correctness/implementation.md)。
