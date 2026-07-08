# P6 — 模型 / skill Eval（第二條驗證軸）

**分期**: P6 ・ 依賴: P1–P5 ・ 估時: ~5h ・ 上層: `spec-overview.md`

> 確定性測試驗機制；eval 分兩軸：pipeline eval 離線驗 intent/slots/action，response eval 驗 LLM/skill **輸出品質**。eval 與其他模組同一套可拔插機制（經 Registry），fixtures 隨能力包，可重跑當 CI gate。對齊 falcon-client I12。

## 變更檔案
| 路徑 | 操作 | 說明 |
|------|------|------|
| `src/runtime/eval/mod.rs` | NEW | eval 子系統匯出 |
| `src/runtime/eval/evaluator.rs` | NEW | `Evaluator` trait + `PipelineDeterministicEvaluator` + `ResponseBaselineEvaluator` + `LlmJudgeEvaluator` |
| `src/runtime/eval/fixtures.rs` | NEW | 載入能力包 eval fixtures（golden set）|
| `src/runtime/eval/baseline.rs` | NEW | response baseline 比對 + 指標彙整 |
| `src/runtime/eval/report.rs` | NEW | eval 報告（pass/fail + latency/token/refuse/fallback）|
| `src/bin/eval.rs` | NEW | 可重跑 eval CLI（CI gate；`--pipeline-only` 離線必跑；response live/replay 選跑）|
| `config/runtime/evals/inputs.json` | NEW | 能力包 eval fixtures（每 intent 數筆中英測資）|
| `config/runtime/evals/response-baseline.json` | NEW | Rust-side 新建 baseline：由 source-recorded responses 或 live smoke 產生，不假設 falcon-client 已有同名檔 |

## 型別（`evaluator.rs`）
```rust
/// 一筆 eval 測資（隨能力包走，config/runtime/evals/inputs.json）。
#[derive(Debug, Clone, Deserialize)]
pub struct EvalCase {
    pub id: String,
    pub input: String,
    pub option_id: Option<String>,
    pub category: EvalCategory,                 // root-option | free-form | ambiguous | injection | no-data
    pub expect: EvalExpectation,
}

/// 期望：pipeline 部分（機制）+ response/rubric（品質）。
#[derive(Debug, Clone, Deserialize)]
pub struct EvalExpectation {
    pub intent: Option<String>,
    pub slots: BTreeMap<String, serde_json::Value>,
    pub output_template: Option<String>,
    pub action: Option<String>,                 // answer | answer-with-disclaimer | refuse:off_scope | refuse:prompt_injection
    pub must_include: Vec<String>,              // response eval：答案應含的關鍵詞
    pub must_not_include: Vec<String>,          // 禁止幻覺/離題用語
    pub rubric: Option<String>,                 // LLM-judge 評分準則
}

pub enum EvalMode { PipelineOnly, ResponseReplay, ResponseLive }

pub struct EvalOutcome {
    pub case_id: String, pub passed: bool,
    pub scores: BTreeMap<String, f32>,          // grounding/insight/relevancy…（LLM-judge）
    pub latency_ms: u64, pub tokens: Option<u32>,
    pub failures: Vec<String>,
}

/// 可拔插：pipeline evaluator 離線；response evaluator 需 live LLM 或 recorded replay。
#[async_trait]
pub trait Evaluator: Send + Sync {
    fn id(&self) -> &str;
    fn mode(&self) -> EvalMode;
    async fn evaluate(&self, case: &EvalCase, observed: &ObservedTurn) -> EvalOutcome;
}
// PipelineDeterministicEvaluator：比對 intent/slots/template/action（離線可跑）。
// ResponseBaselineEvaluator：比對 must(_not)_include、refuse/fallback/latency/token budget（live 或 replay）。
// LlmJudgeEvaluator：用 rubric 對答案評分 grounding/insight/relevancy（live 或 replay）。
```

```rust
// baseline.rs：把 EvalOutcome 聚合，與 response-baseline.json 的 budget 比對
pub struct EvalReport {
    pub total: usize, pub passed: usize,
    pub p50_latency_ms: u64, pub p95_latency_ms: u64,
    pub refuse_rate: f32, pub fallback_rate: f32,
    pub regressions: Vec<String>,               // 對 baseline 退步者
}
impl EvalReport { pub fn is_green(&self, baseline: &Baseline) -> bool; }
```

## config
```toml
# config/config.toml
[runtime.eval]
pipeline_evaluators = ["pipeline-deterministic"]
response_evaluators = ["response-baseline","llm-judge"]
fixtures = "runtime/evals/inputs.json"
baseline = "runtime/evals/response-baseline.json"
```
> `Evaluator` 由 `[runtime.eval] pipeline_evaluators/response_evaluators` 選用、經 `Registry::build_evaluators` 建出（與其他模組同機制）。`cargo run --bin eval -- --pipeline-only` 在 CI 必跑；response baseline / LLM-judge 需 live LLM 或 replay artifact，CI 選跑。
> `response-baseline.json` 是本 migration 新產物：若 source repo 尚未有 baseline，Phase 0/6 需先用 recorded TS responses 或 approved live sample 產生初版。

## 測試（pipeline 部分為 CI 必跑、不需 live LLM）
```rust
#[test] fn fixtures_load_from_pack() { /* inputs.json → Vec<EvalCase> */ }
#[tokio::test] async fn pipeline_eval_checks_intent_slots_and_action() {
    // 只跑 input pipeline + answer policy，比對 expect.intent/slots/action；不需 live LLM
}
#[tokio::test] async fn eval_report_flags_regression_vs_baseline() {
    // 注入超過 baseline budget 的 latency/refuse_rate → EvalReport.is_green == false
}
#[test] fn response_evaluators_marked_live_or_replay() { /* response-baseline/llm-judge 不在 --pipeline-only 路徑執行 */ }
#[test] fn evaluator_registered_in_registry() { /* pipeline/response evaluators 能由 Registry build_evaluators 建出 */ }
```
> 驗收：`cargo run --bin eval -- --pipeline-only` 對第一個能力包種子 fixtures 全綠；response eval 在 live/replay 模式下 `EvalReport.is_green(baseline)` == true。

## 範圍
- 本輪：框架（trait + runner + pipeline evaluator + response evaluator seam + 種子 fixtures/baseline）+ pipeline CI gate。
- 後續：完整 golden-set 規模化、LLM-judge 線上常態跑、eval dashboard。
