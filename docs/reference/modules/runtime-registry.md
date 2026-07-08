# 模組：`runtime::registry` — partial

> ← [Modules](./index.md)  
> **Source**：[`src/runtime/registry.rs`](../../../src/runtime/registry.rs)、[`src/appstate.rs`](../../../src/appstate.rs) `build_runtime`

## 現況職責

`BuiltinRegistry` 保存已知 module ID，提供 validation 與部分 component builders。未知 ID 會讓 runtime config validation 失敗。它是 target pluggability 的 seam，但目前不是所有 config ID 的 production dispatcher。

## Production wiring

| Builder / ID set | Return | AppState/request path uses it? |
|---|---|---|
| `build_answer_policy` | `Arc<dyn AnswerPolicy>` | yes |
| `build_llm_normalizer` | optional `Arc<dyn LlmInputNormalizer>` | yes；disabled backend 是 no-op seam |
| `build_memory` | optional `Arc<dyn SessionMemoryStore>` | yes |
| `build_audit` | `Arc<dyn AuditSink>` | yes |
| `build_input_pipeline` | `Vec<String>` stage IDs | **no production caller**；不是 stage objects |
| `build_evaluators` | `Vec<Arc<dyn Evaluator>>` | 只在 module test 使用；每個 ID 都建 `NoopEvaluator` |
| extractor/guardrail sets | known-ID validation | 沒有 runtime dispatch builder |

`AppState::build_runtime` 最後直接使用 `InputPipeline::default()`，沒有使用 `build_input_pipeline`。eval runner 也沒有從 registry 執行 evaluator list。

## 能力邊界

目前可以只換 config 的部分：intent/lexicon/threshold data、已註冊 answer policy/memory/audit/normalizer backend選擇。  
目前不能只換 config 完成：任意 stage order/implementation、extractor/guardrail/evaluator dispatch、新領域機制。

Target state 見 [PRD FR-005](../prd.md)；實作工作見 [plan I04](../../../.agent/artifacts/plan/2026-06-29-runtime-correctness/implementation.md)。
