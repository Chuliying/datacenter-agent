# Eval Evaluator 機制修正與 Rubric v1 PRD

**Story ID**: S-EVAL-01
**版本**: v1.4.0
**狀態**: Ready
**Sprint**: Sprint 1
**Tickets**: N/A

---

## 版本歷史

| 版本 | 修改時間 | 修改內容（摘要） | 影響範圍 | 對應 Spec 版本 | 作者 |
|------|---------|----------------|---------|--------------|------|
| v1.0.0 | 2026-07-02 14:30 | 初版 | - | — | Claude / dev-kickoff |
| v1.1.0 | 2026-07-02 15:30 | 確認 FU-001（judge_model 採 Option A）、FU-002（threshold 採 Option B，capability-level 可覆寫）；新增 AC-006/AC-007 對應驗證 | FR-003、AC | — | Claude / dev-kickoff |
| v1.2.0 | 2026-07-02 16:15 | Opus review 後修正：(1) FU-002 改回 Option A（全域 threshold，撤銷 capability-level 覆寫，因無分層 config 骨架且僅一個 capability pack）；AC-006 改寫為純單元測試；(2) 修正 §7 錯誤 Assumption——`config.toml` 實際已宣稱 `response-baseline`/`llm-judge`，FR-005 交付前必須先移除，否則 `RuntimeConfig::load` 的 `validate()` 會讓 `--pipeline-only` fail-fast；(3) FR-002 拆分 `build_evaluators` 為 pipeline/response 兩條驗證路徑，避免 pipeline-only 被未實作的 response evaluator id 拖累；(4) FR-001 Data 來源狀態改為誠實揭露 `EvalCase`/`EvalOutcome`/`ObservedTurn`/`EvalCategory` 目前只存在於 spec 文件、需新建；(5) S-EVIDENCE-01 對齊決定 AC-002 用 `compile_fail` doctest（不新增 `trybuild` 依賴） | FU-002、FR-001、FR-002、FR-005、§7、AC-006 | — | Claude / dev-kickoff |
| v1.3.0 | 2026-07-03 15:30 | Codex review 後修正：新增 payload-free `EvaluatorKind`，避免 evaluator metadata 與 CLI `EvalMode` 的路徑 payload 耦合；AC-002 改為 spy evaluator 的 registry-to-runner contract test，直接驗證 dispatch 接線；文件狀態與 metadata 對齊為 Ready | FR-001、FR-002、AC-002、狀態 | — | Codex / review integration |
| v1.3.1 | 2026-07-03 16:01 | 文件整理：移除已完成使命的 brainstorm artifact 參照，保留正式 PRD、plan、spec 與模組文件作為追溯來源 | §9 相關文件 | — | Codex / docs cleanup |
| v1.4.0 | 2026-07-03 16:06 | Review 修正：明定 rubric score/threshold 為 `[0.0, 1.0]`、三維預設門檻 `0.70` 與非法值 fail-fast；eval CLI 新增向後相容的 `--config` 以隔離 intentional-negative fixture | FR-003、FR-004、ERR-004、AC-003、AC-006、AC-008、NFR、Dependencies | — | Codex / review fix |

---

## 0. 防禦性思考（Failure Constraints）

### 三大失敗風險

| 風險 | 說明 | 緩解策略 |
|------|------|---------|
| 🔴 **Registry 換了實作但 runner.rs 沒有真的呼叫到** | `PipelineDeterministicEvaluator` 建出來後，若 `runner.rs::run_pipeline_only()` 沒有改成呼叫 registry 建出的 evaluator，而是繼續用原本手寫比對邏輯，會出現「registry 換了實作、CI 結果卻完全沒變」的假象，跟現在的 noop 問題本質相同 | AC 必須注入可觀察呼叫次數與固定結果的 spy evaluator，從 registry builder 一路執行到 runner/report，不能只測 evaluator 本身 |
| 🟡 **Rubric v1 的弱 grounding heuristic 被誤解為真正的 grounding 驗證** | `must_include`/`must_not_include` 覆蓋率只是替代指標，不是真正對照資料來源；若文件/log 沒有明確標注，未來人員可能誤以為 grounding 已經是可信指標 | 程式註解與 `docs/reference/modules/runtime-eval.md` 必須明確寫「v1 grounding 是 heuristic，非 citation-level 驗證，待 Evidence Pack 完成後升級」 |
| 🟠 **CI negative self-test 本身寫錯，變成恆真測試** | Negative self-test 若寫成「呼叫本來就會失敗的東西」而非「注入真實 regression fixture」，會變成永遠通過的假保護 | Negative self-test 必須先在沒有這個修正前手動驗證會失敗（true negative），再驗證修正後會通過 |

---

## 0.5 最佳實踐搜尋結果

### 業界標準摘要

| 來源 | 關鍵發現 |
|------|---------|
| [EleutherAI/lm-evaluation-harness \| DeepWiki](https://deepwiki.com/EleutherAI/lm-evaluation-harness) | Registry pattern（`Registry[T]` 映射 string alias 到物件或 lazy placeholder）是業界標準拔插機制——與本專案 `BuiltinRegistry::build_evaluators()` 的設計方向相同 |
| [LLM-as-a-Judge Rubric Design \| Appen](https://www.appen.com/llm-as-a-judge-rubric-design) | 模糊準則會產生不一致分數；建議 categorical threshold 判定比純數值加權平均更穩定，對應本 story 「各維度獨立 threshold」的決定 |

### 採用策略

- ✅ 沿用既有 `BuiltinRegistry` 的 registry pattern，把 `PipelineDeterministicEvaluator` 接進去，不新增第二套拔插機制
- ✅ Rubric 分數維度採用 categorical/threshold 判定（各維度獨立 threshold），而非加權平均，對齊業界對「清楚 score-level 定義比連續數值更穩定」的建議
- ✅ 明確標注 v1 grounding 是 heuristic，待有 citation 依據（Evidence Pack）才算真正 grounding 判定，避免過early 宣稱已達成業界說的「evidence-grounded scoring」
- ❌ 不採用 float 型連續分數作為 pass/fail 唯一依據（業界建議 categorical 更穩定）；本 sprint 不引入完整 G-Eval chain-of-thought 機制，因為 LlmJudgeEvaluator 本身排除在本 sprint 範圍外

---

## 1. 概述

### 商業目標

把 eval CI gate 從「換 evaluator id 不影響結果」的假象，修正為「config 真正 dispatch」，讓 PRD FR-010／FR-005 對 eval 子系統成立。

### Personas + 痛點

| Persona | 描述 | 目前痛點 |
|---------|------|---------|
| Runtime 維護工程師 | 修改 `config/config.toml` 的 `[runtime.eval]` 區塊來調整 evaluator 組合 | 改了 config，CI 結果完全不變，因為 registry 建出的都是 `NoopEvaluator`，無從察覺自己的變更沒有生效 |
| CI/Release 守門人 | 依賴 `cargo run --bin eval -- --pipeline-only` 的 exit code 決定能否合併 | 目前只有 3 筆 fixtures 且比對邏輯寫死在 `runner.rs`，無法信任「evaluator 已涵蓋 config 宣稱的檢查項」 |

### 效能期望

`cargo run --bin eval -- --pipeline-only` 應在數秒內完成（現有 3 筆 fixture 規模），本 sprint 不擴充 fixture 規模，不引入額外效能負擔。

### 成功指標

| 指標 | 目標值 | 衡量方式 |
|------|--------|---------|
| Evaluator dispatch 真實性 | 100% | registry-to-runner contract test 注入 spy evaluator，證明 evaluator 被呼叫且其固定結果會改變 `EvalReport` |
| CI negative gate 有效性 | 1 個 intentional-failing fixture 使 process exit 1 | Integration test 斷言 |
| Registry fail-fast 覆蓋率 | 100% 未實作 evaluator id 在 startup 被拒絕 | Config validation 單元測試 |
| Rubric threshold 設定安全性 | 100% 非有限值與超出 `[0.0, 1.0]` 的門檻被拒絕 | Config validation 單元測試 |

---

## 2. 功能需求（FR）

### FR-001：Evaluator trait 改為 async 版本並分離執行種類

**描述**：把 `src/runtime/eval/evaluator.rs` 現有的同步 `Evaluator` trait（`id()` + `run() -> RuntimeResult<()>`）換成 async 版本：`id()`、`kind() -> EvaluatorKind`、`async fn evaluate(&self, case: &EvalCase, observed: &ObservedTurn) -> EvalOutcome`。新增 payload-free 的 `EvaluatorKind::{Pipeline, Response}`，只描述 evaluator 適用種類；不得沿用 runner 的 CLI `EvalMode`，因為現有 `EvalMode::ResponseReplay`/`ResponseLive` 分別攜帶 artifact/baseline 路徑，屬於單次執行參數，不是 evaluator metadata。

**輸入**：

| 參數 | 型別 | 必要 | 說明 |
|------|------|------|------|
| case | `&EvalCase` | ✅ | 一筆 eval 測資（id/input/option_id/category/expect） |
| observed | `&ObservedTurn` | ✅ | 實際觀察到的 pipeline/response 執行結果 |

`EvaluatorKind` 契約：

| Variant | 說明 |
|---------|------|
| `Pipeline` | 離線 pipeline evaluator；不需要 response artifact、baseline 路徑或 provider credential |
| `Response` | response evaluator；實際採 replay 或 live、以及對應路徑，仍由 runner 的 `EvalMode` 決定 |

**輸出**：

| 項目 | 型別 | 說明 |
|------|------|------|
| outcome | `EvalOutcome` | 含 `case_id`、`passed`、`scores`（維度分數）、`latency_ms`、`tokens`、`failures` |

**邊界條件**：

- 舊的 `NoopEvaluator`/同步 `run()` 介面必須整個移除，不保留 dual-interface 相容層（本專案內部型別，非對外 API，無相容性負擔）
- `EvaluatorKind` 不得包含 artifact/baseline 路徑或其他單次執行資料；CLI `EvalMode` 的既有 variants 與參數保持不變
- trait 改動必須是 breaking change，所有既有呼叫點（`registry.rs`）需同步更新，不可留下編譯警告
- `EvalCase`/`EvalOutcome`/`ObservedTurn`/`EvalCategory` 目前只存在於 spec 文件（`src/` 內完全查無這些型別），本 FR 需要**新建**這些型別，不是搬遷既有程式碼；`PipelineFixture`（`fixtures.rs`）需要一個轉換成 `EvalCase` 的 adapter

**Data 來源狀態**：
- [x] ✅ 已有現成資料源（型別**設計**依據 `docs/agent-runtime-rust-port/spec/spec-06-eval.md` 既有 spec，但依目前 payload-bearing CLI `EvalMode` 修正為獨立 `EvaluatorKind`；程式碼本身是新建，非搬遷既有 Rust 型別）

**權限/可見性**：內部 Rust module，無使用者可見性議題。

### FR-002：`PipelineDeterministicEvaluator` 實作與 registry 真接線

**描述**：新增 `PipelineDeterministicEvaluator`，把 `runner.rs::run_pipeline_only()` 中手寫的 intent/slots 比對邏輯搬進去；`registry.rs::build_evaluators()` 對 id `"pipeline-deterministic"` 回傳這個真實 impl；`runner.rs` 改為呼叫 registry 建出的 evaluator 執行判定，不再自行重複比對邏輯。**`build_evaluators()` 需拆成 `build_pipeline_evaluators()`/`build_response_evaluators()` 兩條獨立路徑**（現行版本把 `pipeline_evaluators`/`response_evaluators` 兩份 config 串成同一個 `Vec` 再逐一驗證），否則 `--pipeline-only` 會被 config 裡尚未實作的 response evaluator id 一併卡住（見 FR-005）。

**輸入**：

| 參數 | 型別 | 必要 | 說明 |
|------|------|------|------|
| fixtures | `Vec<PipelineFixture>` | ✅ | 沿用既有 `config/runtime/evals/inputs.json` |
| runtime_config | `&RuntimeConfig` | ✅ | 沿用既有 `InputPipeline::run_with_config` 呼叫路徑 |

**輸出**：

| 項目 | 型別 | 說明 |
|------|------|------|
| report | `EvalReport` | 沿用既有 `passed`/`failed` 欄位語意，不改變外部 CLI 輸出格式 |

**邊界條件**：

- `runner.rs` 呼叫 registry 建出的 evaluator 後，若比對邏輯與原本 `run_pipeline_only()` 手寫邏輯不一致，須以 spec 定義（intent/slots/action 比對）為準，不得為了相容舊行為而妥協正確性
- 既有 3 筆 fixture 的 pass/fail 結果在這次重構後必須維持不變（重構不改變可觀察行為，除非該行為本身就是要修正的 bug）
- `run_pipeline_only()` 目前是同步函式，新 `Evaluator::evaluate()` 是 async；需要一個明確的橋接方式（如 `tokio::runtime::Runtime::block_on`，沿用 `runner.rs` 現有對 `ResponseLive` 模式的做法），本 FR 需在實作時定案，不得留下 ad hoc 寫法
- `registry.rs:212-218` 現有的 evaluator count 單元測試（斷言 `build_evaluators` 回傳長度）需同步更新，避免因拆成兩個函式而變成過期斷言
- runner 需提供不依賴檔案或全域 config 的內部測試接縫，接收 registry builder 產出的 `Vec<Arc<dyn Evaluator>>`；contract test 以 spy evaluator 記錄呼叫次數並回傳固定 fail outcome，驗證 runner 確實消費該 evaluator 並反映到 `EvalReport`

**Data 來源狀態**：
- [x] ✅ 已有現成資料源（`config/runtime/evals/inputs.json`，本 FR 不新增/修改 fixture 內容，只搬遷比對邏輯的執行位置）

**權限/可見性**：內部 CLI/CI 使用，無終端使用者權限議題。

### FR-003：Rubric v1 型別與 threshold 判定

**描述**：本 FR 涵蓋四件事：

1. Pipeline fixture 與新增的 response fixture 型別加上 `rubric: Option<String>` 欄位（per-fixture judging instruction，非全域一份）
2. `EvalOutcome.scores` 支援 `grounding`/`insight`/`relevancy` 三維度，`EvalReport` 新增各維度獨立 threshold 判定（任一維度低於門檻即該筆 fail，不使用加權平均）
3. `grounding` 維度在 Evidence Pack 完成前只能用 fixture 的 `must_include`/`must_not_include` 覆蓋率作為 heuristic 替代分數，型別/文件需明確標注非真正 citation-level grounding
4. 新增 `judge_model` config 欄位（允許為空）；threshold 維持 runtime 全域固定值，不做 capability-level 分層

**輸入**：

| 參數 | 型別 | 必要 | 說明 |
|------|------|------|------|
| rubric | `Option<String>` | 選填 | per-fixture judging instruction，本 sprint 只定義型別與載入，不要求 live judge 消費它 |
| thresholds | 三個 `f32`（grounding/insight/relevancy） | 選填 | 分數與門檻皆使用 `[0.0, 1.0]` 尺度；整個 table 或個別維度省略時使用 `0.70`，runtime 全域固定，本 sprint 不做 capability-level 覆寫 |
| judge_model | `Option<String>`，位於 `[runtime.eval]` | 選填 | 與 production Final LLM 分開設定避免自評偏差；本 sprint 允許為空，Sprint 2 `LlmJudgeEvaluator` 才要求非空 |

設定契約：

```toml
[runtime.eval.thresholds]
grounding = 0.70
insight = 0.70
relevancy = 0.70
```

**輸出**：

| 項目 | 型別 | 說明 |
|------|------|------|
| dimension_pass | `BTreeMap<String, bool>` | 每個維度是否達門檻，供 `EvalOutcome.passed` 彙整（任一 false → 整體 false）；門檻為 runtime 全域固定值，本 sprint 不涉及 capability config 解析 |

**邊界條件**：

- `judge_model` config 欄位本 sprint 允許為空值，不因為空值導致 startup fail-fast——真正要求非空是 Sprint 2 `LlmJudgeEvaluator` 的範圍
- `[runtime.eval.thresholds]` 與其中個別欄位皆可省略；缺少的維度以 `0.70` 補齊，確保既有 config 不因新增 schema 而失效
- rubric 欄位為空時，evaluator 不得因此 panic 或視為自動 pass，需明確定義「無 rubric = 略過該維度評分」的行為
- 三個 threshold 必須通過 `is_finite()` 且落在閉區間 `[0.0, 1.0]`；NaN、正負無限值、負值或大於 `1.0` 一律在 config load 階段回傳 `RuntimeError::Config`
- fixture 有 rubric、但 evaluator 缺少任一必要維度分數時，該 case 必須 fail 並指出缺少的維度；不得把缺分數視為通過
- 本 sprint 沒有任何 evaluator 會產出 response 文字（`pipeline-deterministic` 只比對 intent/slots），因此三維度 threshold 判定函式本 sprint **只做單元測試層級的隔離驗證**（輸入分數 → 輸出 pass/fail），不要求端到端跑出真實分數（見 AC-006 修正）

**Data 來源狀態**：
- [x] ✅ 已有現成資料源（config schema 擴充，非新 API）

**權限/可見性**：內部 config/CLI，無終端使用者權限議題。

### FR-004：CI negative self-test

**描述**：eval CLI 新增全域 `--config <PATH>`，預設仍為 `config/config.toml`；新增一個指向 intentional-failing fixture 的 test-only config，並以 process integration test 斷言 `cargo run --bin eval -- --config tests/fixtures/eval-failing/config.toml --pipeline-only` 的 exit code 為 1。同時保留未傳 `--config` 的既有 positive smoke test，確保預設路徑未觸發 regression 時 exit code 為 0。

**輸入**：

| 參數 | 型別 | 必要 | 說明 |
|------|------|------|------|
| config | `PathBuf` | 選填 | CLI `--config <PATH>`；未提供時預設 `config/config.toml`，接受相對或絕對路徑 |
| intentional-failing fixture | JSON 檔案或程式內建 synthetic case | ✅ | 刻意設計成一定會 fail 的比對條件 |

**輸出**：

| 項目 | 型別 | 說明 |
|------|------|------|
| process exit code | `i32` | regression 存在時為 1，否則為 0 |

**邊界條件**：

- Negative self-test 必須先驗證「移除本次修正前」會失敗（true negative），避免自我實現的假保護（對應 §0 風險 3）
- intentional-negative fixture 不可混入正式的 `config/runtime/evals/inputs.json`；`tests/fixtures/eval-failing/config.toml` 必須用相對於該 config 檔案的路徑指向 test-only fixture
- `--config` 不改變既有 mode flags；未指定時必須維持目前 `config/config.toml` 行為，指定不存在、不可讀或格式錯誤的路徑時回傳 config error 並 exit 1

**Data 來源狀態**：
- [x] ✅ 已有現成資料源（沿用既有 `run(EvalMode)` 的 process exit 慣例，`AC-009` 已證明 nonzero exit 機制存在，本 FR 只是補齊 evaluator dispatch 真的接線後的 negative test）

**權限/可見性**：CI-only，無終端使用者權限議題。

### FR-005：Registry 對未支援 evaluator id fail-fast

**描述**：`registry.rs::build_evaluators()` 對 config 宣稱但沒有真實 impl 的 evaluator id 在 startup 階段回傳明確錯誤並 fail-fast，不再靜默塞 `NoopEvaluator` 假裝支援。**`config/config.toml:77` 目前已宣稱 `response_evaluators = ["response-baseline", "llm-judge"]`，兩者本 sprint 都沒有真實 impl**——本 FR 必須同時把這兩個 id 從 `config.toml` 移除（並在 config 註解標注「Sprint 2 實作對應 evaluator 後才重新宣稱」），否則 `RuntimeConfig::load` 的 `validate()`（`config.rs:317-322`）會在 `--pipeline-only` 呼叫的 config load 階段就 fail-fast，直接牴觸 AC-001。

**輸入**：

| 參數 | 型別 | 必要 | 說明 |
|------|------|------|------|
| evaluator id | `String` | ✅ | 來自 `config.toml` 的 `pipeline_evaluators`/`response_evaluators` |

**輸出**：

| 項目 | 型別 | 說明 |
|------|------|------|
| `RuntimeResult<Vec<Arc<dyn Evaluator>>>` | Result | 未知/未實作 id 回傳 `RuntimeError::Config`，訊息需明確指出哪個 id 不受支援 |

**邊界條件**：

- 交付順序：`config.toml` 移除 `"response-baseline"`/`"llm-judge"` 宣稱，必須與本 FR 的 fail-fast 邏輯**同一個 PR 交付**，不可分兩步（分兩步會有一個中間狀態讓 CI 紅燈）
- 已有真實 impl 的 id（本 sprint 交付的 `"pipeline-deterministic"`）不受影響
- 本 FR 的 fail-fast 只驗證「id 是否有真實 impl」，不含 FR-002 提到的 pipeline/response 路徑拆分本身——兩者是同一次交付的兩個必要條件，缺一 `--pipeline-only` 都無法通過 AC-001

**Data 來源狀態**：
- [x] ✅ 已有現成資料源（`registry.rs` 既有 `require_evaluator` 驗證機制，本 FR 是把驗證結果從「允許 noop 通過」改成「fail-fast」）

**權限/可見性**：Startup-time 內部驗證，無終端使用者權限議題。

---

## 3. 非功能需求（NFR）

| 類別 | 要求 |
|------|------|
| **Performance** | `cargo run --bin eval -- --pipeline-only` 在既有 3 筆 fixture 規模下，執行時間不因本次重構明顯增加（維持數秒內完成） |
| **Security / Compliance** | 不適用，因為本 story 不涉及 credential、PII 或對外資料存取；`judge_model` config 欄位本 sprint 允許為空，不涉及 secret 儲存 |
| **Accessibility** | 不適用，因為本 story 為 Rust 後端 CLI/library 變更，無使用者介面 |
| **Compatibility** | 既有 `--pipeline-only`/`--response --replay`/`--response --live` 行為不變；`--config <PATH>` 是 additive option，未提供時仍讀 `config/config.toml` |

---

## 4. 錯誤場景（ERR）

### ERR-001：Config 宣稱未實作的 evaluator id

**觸發條件**：`config.toml` 的 `pipeline_evaluators`/`response_evaluators` 列出一個沒有對應真實 impl 的 id（例如移除前的 `"llm-judge"`/`"response-baseline"`——本 sprint 已在 FR-005 交付時一併從 config 移除，此 ERR 描述的是未來若有人誤加回去的情況）

**預期行為**：`registry.rs::build_evaluators()` 在 startup 階段回傳 `RuntimeError::Config`，訊息明確指出哪個 id 不受支援，應用程式無法啟動

**恢復策略**：維護工程師需從 config 移除該 id，或等該 evaluator 真實 impl 完成後再宣稱

### ERR-002：Rubric 欄位存在但沒有 judge 消費它

**觸發條件**：fixture 的 `rubric` 欄位有內容，但本 sprint 沒有 `LlmJudgeEvaluator` 去讀取它

**預期行為**：載入 fixture 時不因 `rubric` 有值而觸發任何評分行為（因為本 sprint 沒有 consumer），也不得因為欄位存在但未使用而報錯——欄位單純被保留供 Sprint 2 使用

**恢復策略**：不需要恢復，屬預期行為；文件需註明此欄位目前僅供型別預留

### ERR-003：Negative self-test 的 intentional-failing fixture 意外被真正 pass

**觸發條件**：`PipelineDeterministicEvaluator` 實作有 bug，導致本應設計成一定失敗的 fixture 被判定為 pass

**預期行為**：CI negative self-test 本身要能偵測到這個狀況——test 斷言的是「process exit code 應為 1」，若 evaluator bug 導致 exit code 變成 0，negative self-test 應該 fail，提示開發者 evaluator 邏輯有誤而非測試基礎設施出錯

**恢復策略**：檢查 `PipelineDeterministicEvaluator::evaluate()` 的比對邏輯，確認 intentional-failing fixture 的欄位是否真的觸發不一致條件

### ERR-004：Rubric threshold 不合法

**觸發條件**：`grounding`、`insight` 或 `relevancy` threshold 是 NaN、正負無限值、負值或大於 `1.0`

**預期行為**：`RuntimeConfig::load` 回傳 `RuntimeError::Config`，錯誤訊息包含維度名稱、非法值與合法範圍 `[0.0, 1.0]`，應用程式與 eval CLI 均不得完成啟動

**恢復策略**：將該維度改為有限且位於 `[0.0, 1.0]` 的數值；未客製時使用預設值 `0.70`

---

## 5. 驗收標準（AC）

### AC-001：Evaluator trait 改動後既有 fixture 行為不變

```gherkin
Given `config/runtime/evals/inputs.json` 現有 3 筆 pipeline fixture 未變更
When 執行 `cargo run --bin eval -- --pipeline-only`（改用新 async Evaluator trait + PipelineDeterministicEvaluator 之後）
Then 三筆 fixture 的 pass/fail 結果與重構前一致，process exit code 為 0
```

**備註**：這是重構安全網，證明 trait 改動本身不改變可觀察行為。

### AC-002：Registry 建出的 evaluator 會被 runner 實際呼叫

```gherkin
Given 測試 registry 為 pipeline evaluator id 建出一個記錄呼叫次數且固定回傳 fail outcome 的 spy evaluator
When 透過 runner 的內部測試接縫執行一筆原本會通過手寫 intent/slots 比對的 fixture
Then spy evaluator 的呼叫次數為 1，`EvalReport.failed` 增加 1，且 runner 不再執行自己的 intent/slots 判定
```

**備註**：此 contract test 必須跨越 registry builder → runner → report 三層；只直接呼叫 `PipelineDeterministicEvaluator::evaluate()` 的單元測試不能取代本 AC。

### AC-003：CI negative self-test 能偵測 regression

```gherkin
Given 已新增一筆 intentional-failing fixture（設計成一定會比對失敗）
When 執行 `cargo run --bin eval -- --config tests/fixtures/eval-failing/config.toml --pipeline-only`
Then process exit code 為 1，且輸出包含該 fixture 的 regression 訊息
```

### AC-004：未實作 evaluator id 導致 startup fail-fast

```gherkin
Given `config.toml` 的 `[runtime.eval] pipeline_evaluators` 或 `response_evaluators` 宣稱一個沒有真實 impl 的 id
When 應用程式或 eval CLI 嘗試啟動並呼叫 `build_evaluators()`
Then 回傳 `RuntimeError::Config` 且訊息包含該 evaluator id，應用程式無法完成啟動
```

### AC-005：Rubric 欄位可被載入但不影響本 sprint 判定結果

```gherkin
Given 一筆 fixture 的 JSON 中包含 `rubric` 欄位且有文字內容
When 執行 `cargo run --bin eval -- --pipeline-only`
Then fixture 正常載入不報錯，且 `rubric` 欄位不影響該筆 fixture 的 pass/fail 判定（因為本 sprint 沒有消費它的 evaluator）
```

### AC-006：三維度 threshold 判定函式的單元測試

```gherkin
Given 三維 threshold 均為 `0.70`，且一組固定分數中有一維為 `0.69`
When 呼叫 threshold 判定函式
Then 回傳的 `dimension_pass` 該維度為 false，且整體 `passed` 為 false（任一維度不達標即整體 fail）
```

**備註**：本 sprint 沒有任何 evaluator 產出 response 文字（`pipeline-deterministic` 只比對 intent/slots），threshold 判定函式沒有端到端 consumer，因此本 AC 只做函式層級單元測試，不宣稱有真實分數跑過完整流程（對齊 FR-003 邊界條件第 3 點）。

### AC-007：`judge_model` 允許為空且不阻塞本 sprint 交付

```gherkin
Given `config/config.toml` 的 `[runtime.eval]` 沒有設定 `judge_model`（欄位為空）
When 應用程式或 eval CLI 啟動
Then 啟動成功，不因 `judge_model` 為空而 fail-fast（本 sprint 不強制要求 live judge）
```

### AC-008：非法 threshold 在 config load 階段被拒絕

```gherkin
Given 表格驅動測試依序省略整個 threshold table、或把任一 threshold 設為 `NaN`、正負無限值、`-0.01` 或 `1.01`
When 呼叫 `RuntimeConfig::load`
Then 省略 table 時三維均載入為 `0.70`；非法值每筆都回傳 `RuntimeError::Config` 且訊息包含維度名稱與合法範圍；邊界值 `0.0`、`1.0` 均載入成功
```

---

## 6. UI/UX 概念

不適用，因為本 story 是 Rust 後端 CLI/library 內部機制修正，無使用者介面、無設計稿、無 UI States/Microcopy/響應式行為需求。

---

## 7. Dependencies & Constraints

- **上游依賴**：無（本 story 是既有 `src/runtime/eval/` 骨架的修正，不依賴 Evidence Pack story）
- **下游影響**：Sprint 2 的 `ResponseBaselineEvaluator` 搬遷、fixtures 規模化、`LlmJudgeEvaluator` 實作都建立在本 story 的 `Evaluator` trait 之上；`docs/reference/modules/runtime-eval.md`、PRD FR-010 現況描述需在本 story 完成後同步更新
- **Breaking Change**：✅ 是（影響：`src/runtime/eval/evaluator.rs` 的 `Evaluator` trait 簽名整個改變，`NoopEvaluator` 移除；`registry.rs::build_evaluators()` 拆成 pipeline/response 兩個函式；移除尚未實作的 response evaluator 宣稱；thresholds 與 CLI `--config` 均為 additive 且有向後相容預設）
- **Assumptions**：無未驗證假設——已核實 `config.toml` 現況並將對應動作（移除未實作 evaluator id）納入 FR-005 交付範圍（見 v1.2.0 修正）

---

## 8. 範圍外

- ❌ `ResponseBaselineEvaluator` 完整搬遷（`runner.rs::run_response_replay()` 的邏輯本 sprint 不動，排入 Sprint 2）
- ❌ Fixtures 規模化到涵蓋每個 intent × `EvalCategory`（root-option/free-form/ambiguous/injection/no-data），本 sprint 維持現有 3 筆
- ❌ `LlmJudgeEvaluator` 真正 live 評分實作（本 sprint 只定義 rubric 型別與 threshold 判定框架，不接 live LLM 呼叫）
- ❌ Evidence Pack、SkillPackage、FinalLlmPort 相關工作（見獨立 PRD：`docs/sprints/2026-07-02-sprint-1/evidence-pack-skillpackage-finalllmport/prd.md`）
- ❌ Report maker 相關工作（依既有排程決策，排在 Evidence Pack 之後）

---

## 9. 相關文件

| 文件 | 連結 |
|------|------|
| 對應 PRD 條目 | `docs/reference/prd.md` FR-010 / AC-009 |
| 對應 Plan section | `.agent/artifacts/plan/2026-06-29-runtime-correctness/implementation.md` I06 |
| 對應 Spec | `docs/agent-runtime-rust-port/spec/spec-06-eval.md` |
| 現況模組文件 | `docs/reference/modules/runtime-eval.md` |
| 同 Sprint 另一個 Story | `docs/sprints/2026-07-02-sprint-1/evidence-pack-skillpackage-finalllmport/prd.md` |

---

## Gate 1 自檢清單

**基本檢核**：
- [x] **§0 防禦性思考** 有 3 個失敗風險
- [x] **§0.5 最佳實踐** 有 search_web 紀錄
- [x] 每個 FR 有輸入/輸出/Data 來源狀態
- [x] 所有 AC 的 Given 含可執行 precondition
- [x] 至少 3 個錯誤場景（ERR）
- [x] 「範圍外」有內容
- [x] 沒有「可能」「大概」「應該」等模糊詞
- [x] 版本歷史表已填第一筆

**新增檢核**：
- [x] 所有 FU 決策已收斂：原 FU-001（judge_model → Option A）、FU-002（threshold → Option A，v1.2.0 修正）已全數確認並回填進 FR-003/AC，Follow-ups 區塊移除
- [x] NFR 四項都已填（或標「不適用 + 原因」）
- [x] UI States 五項齊全（或標「不適用 + 原因」）— 本 story 整個 §6 標不適用並附原因
- [x] Microcopy 主要按鈕 + 主要錯誤訊息已列 — 不適用（同 §6 原因）
- [x] Dependencies 的 Breaking Change 已標記
- [x] 流程圖依條件規則產出（或明確說明為何省略）— 本 story 無頁面導航、分支邏輯集中在 ERR/AC 已足夠表達，省略 §1.5
- [x] （manifest 無 knowledge_boundary 欄位）不適用

**角色視角**：
- [x] PM：Sprint（Sprint 1）/ Dependencies / 商業目標 + 成功指標清楚
- [x] RD：NFR 可實作 / Breaking Change 已標
- [x] User：Persona（維護工程師、CI 守門人）+ Pain Point 已列 / 內部工具無權限分級議題
- [x] QA：每個 AC 的 Given 含可執行 precondition
- [x] UI：不適用（後端 CLI/library），已於 §6 說明原因
