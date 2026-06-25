# P1 — config + registry + schema + error

**分期**: P1 ・ 依賴: 無 ・ 估時: ~5h ・ 上層: `spec-overview.md`

> 本層是「config 驅動 + 模組拔插」的地基：載入能力包、註冊可插拔模組、定義核心型別與錯誤。

## 變更檔案
| 路徑 | 操作 | 說明 |
|------|------|------|
| `src/runtime/mod.rs` | NEW | runtime 根；`RuntimeConfig` 聚合 + re-export |
| `src/runtime/config.rs` | NEW | 載入能力包 TOML（含模組組裝段）成 typed struct + `validate()` |
| `src/runtime/registry.rs` | NEW | config 字串 → 註冊的 trait object（input stage/answer policy/memory/audit/guardrail/extractor/evaluator）|
| `src/runtime/error.rs` | NEW | `RuntimeError`（`thiserror`）；config/per-request 分流 |
| `src/runtime/schema.rs` | NEW | `NormalizedInput`/`NormalizedSlots`/`TimeRangeSlot`；intent 為 validated `String`；derive Ser+De |
| `src/config.rs` | MODIFY | `Manifest` 擴 `[runtime]` + 模組組裝段 |
| `src/lib.rs` | MODIFY | `pub mod runtime;` |
| `Cargo.toml` | MODIFY | 加 `uuid`/`sha2`/`unicode-normalization`/`regex`/`async-trait`/`thiserror` |
| `config/config.toml` | MODIFY | 新增 `[runtime]` + 模組組裝段 |
| `config/runtime/{intents,lexicon,thresholds,injection}.toml` | NEW | 第一個能力包 |

## 型別

### `src/runtime/schema.rs`
```rust
/// intent 為 config 驅動：validated String，非 enum（PRD US-24）。
pub type Intent = String;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TimeRangeKind { Week, Month, Quarter, Year, Custom }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TimeRangeSlot {
    pub label: String, pub kind: TimeRangeKind,
    pub start: Option<String>, pub end: Option<String>, pub months: Option<u32>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct NormalizedSlots {
    pub time_range: Option<TimeRangeSlot>,
    pub asset: Option<String>, pub metric: Option<String>, pub rank_limit: Option<u32>,
    pub extensions: BTreeMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NormalizedInput {
    pub raw_input: String, pub clean_input: String,
    pub intent: Intent, pub confidence: f32,
    pub candidate_intents: Vec<Intent>,
    pub intent_source: Option<IntentSource>,
    pub slots: NormalizedSlots, pub warnings: Vec<String>,
    pub output_template: Option<OutputTemplate>,
    pub registry_versions: RegistryVersions,
}
// IntentSource(kebab): option-path|rule-lexicon|text-override|llm-normalizer
// OutputTemplate(kebab): bi-briefing|data-lookup|fallback
```

### `src/runtime/error.rs`
```rust
use thiserror::Error;

#[derive(Debug, Error)]
pub enum RuntimeError {
    // ── config / boot（中止開機）──
    #[error("config: {0}")] Config(String),
    #[error("unknown module id `{0}` in [runtime.{1}]")] UnknownModule(String, String),
    #[error("intent `{0}` not in allowlist")] IntentNotAllowed(String),
    // ── per-request（映射成 AppError）──
    #[error("input required")] InputRequired,
    #[error("input too long: {0} chars")] InputTooLong(usize),
    #[error("pipeline contract invalid")] PipelineContract,
    #[error("audit sink failed: {0}")] AuditSink(String),
}
```
> 規則：config 載入失敗 → 中止開機；per-request → 映射 `AppError`（既有 `server/error.rs`）。`runtime/` 的 request path **禁用 `unwrap`/`expect`**，一律 `?` 傳遞。

### `src/runtime/config.rs`（含模組組裝段）
```rust
pub struct RuntimeConfig {
    // ── 領域資料 ──
    pub intent_allowlist: Vec<String>,
    pub intents: Vec<IntentDef>,                    // id + keywords（非空）
    pub option_prefixes: BTreeMap<String, String>,  // 值須 ∈ allowlist
    pub metric_aliases: BTreeMap<String, String>,
    pub asset_allowlist: HashSet<String>,           // ← asset 未知判定來源（取代硬編）
    pub time_ranges: Vec<TimeRangePattern>,         // 預編譯 Regex + slot 範本
    pub top_n_pattern: Regex,
    pub thresholds: Thresholds,
    pub input: InputLimits,
    pub injection: InjectionRuleSet,                // version + Vec<Regex>
    pub registry_versions: RegistryVersions,
    // ── 模組組裝（PRD US-4~7）──
    pub assembly: Assembly,
}

pub struct Assembly {
    pub input_stages: Vec<String>,          // 啟用的同步 input pipeline stage（順序）
    pub answer_policy_backend: String,      // "rule" | ...
    pub llm_normalizer_enabled: bool,       // 預設 false；不進同步 input pipeline
    pub llm_normalizer_backend: Option<String>,
    pub memory_enabled: bool,
    pub memory_backend: String,             // "in-memory" | ...
    pub audit_sink: String,                 // "stdout" | ...
    pub audit_failure_policy: AuditFailurePolicy, // fail-open | fail-closed
    pub guardrails: Vec<String>,            // 啟用的 guardrail
    pub extractors: Vec<String>,            // 啟用的 slot extractor
    pub pipeline_evaluators: Vec<String>,   // P6：離線 evaluator
    pub response_evaluators: Vec<String>,   // P6：live/replay evaluator
}

pub struct Thresholds {
    pub confidence: ConfidenceTuning,   // answer_normal/answer_gray/option_path/llm_override_floor
    pub classifier: ClassifierTuning,   // ↓ 完整移植 COS_CLASSIFIER
    pub memory: MemoryLimits,
}
pub struct ClassifierTuning {
    pub option_match_confidence: f32,   // 0.95
    pub text_override_confidence: f32,  // 0.90
    pub unknown_confidence: f32,        // 0.25
    pub no_score_floor: f32,            // 0.20
    pub ambiguous_confidence: f32,      // 0.55
    pub margin_tiers: Vec<MarginTier>,  // {min_margin, confidence}
    pub keyword_long_chars: usize, pub keyword_long_weight: u32, pub keyword_short_weight: u32,
}

pub struct InputLimits {
    pub max_prompt_chars: usize,             // EV parity pack = 4000；host 仍保留 64KiB body cap
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AuditFailurePolicy { FailOpen, FailClosed }

impl RuntimeConfig {
    /// 開機驗證（PRD US-24）：
    /// 1. 每個 [[intent]].id 與 option_prefixes 值 ∈ allowlist
    /// 2. "unknown" ∈ allowlist（answer-policy off-scope 依賴）
    /// 3. keywords 非空、id 不重複
    /// 4. assembly 內每個 id 都能在 registry 找到（否則 UnknownModule）
    pub fn validate(&self, reg: &Registry) -> Result<(), RuntimeError>;
}
```

### `src/runtime/registry.rs`（模組拔插核心）
```rust
/// input pipeline 單一同步階段：依 config 啟用，依序執行。
pub trait PipelineStage: Send + Sync {
    fn id(&self) -> &str;
    fn run(&self, cx: &mut PipelineCx) -> Result<(), RuntimeError>;
}

/// 回答前策略：同步決策，不做 I/O；backend 由 config 選。
pub trait AnswerPolicy: Send + Sync {
    fn id(&self) -> &str;
    fn decide(&self, input: &NormalizedInput, thresholds: &Thresholds) -> AnswerAction;
}

/// 可選 LLM input normalizer/enhancer：預設關閉；由 orchestrator 在同步 input pipeline 後、
/// answer policy 前呼叫。它不得取代 rule pipeline，只能在低信心/灰區時補強。
#[async_trait]
pub trait LlmInputNormalizer: Send + Sync {
    fn id(&self) -> &str;
    async fn enhance(&self, input: NormalizedInput, ctx: &LlmNormalizerContext)
        -> Result<NormalizedInput, RuntimeError>;
}

pub struct LlmNormalizerContext {
    pub raw_prompt: String,
    pub option_id: Option<String>,
    pub session_id: Option<String>,
    pub confidence: f32,
    pub reason: String, // "low-confidence" | "ambiguous" | ...
}

/// 各類模組的 registry（id → 建構函式）。開機依 Assembly 查表組裝。
pub struct Registry {
    stages: HashMap<String, Box<dyn Fn(&RuntimeConfig) -> Box<dyn PipelineStage>>>,
    answer_policies: HashMap<String, Box<dyn Fn() -> Arc<dyn AnswerPolicy>>>,
    llm_normalizers: HashMap<String, Box<dyn Fn() -> Arc<dyn LlmInputNormalizer>>>,
    memory: HashMap<String, Box<dyn Fn() -> Arc<dyn SessionMemoryStore>>>,
    audit:  HashMap<String, Box<dyn Fn() -> Arc<dyn AuditSink>>>,
    extractors: HashMap<String, Box<dyn Fn() -> Box<dyn SlotExtractor>>>,
    evaluators: HashMap<String, Box<dyn Fn() -> Arc<dyn Evaluator>>>,   // P6
}
impl Registry {
    pub fn with_builtins() -> Self;   // 註冊 in-memory / stdout / rule policy / 內建 stage / extractor / evaluator
    pub fn build_input_pipeline(&self, cfg: &RuntimeConfig) -> Result<Vec<Box<dyn PipelineStage>>, RuntimeError>;
    pub fn build_answer_policy(&self, cfg: &RuntimeConfig) -> Result<Arc<dyn AnswerPolicy>, RuntimeError>;
    pub fn build_llm_normalizer(&self, cfg: &RuntimeConfig) -> Result<Option<Arc<dyn LlmInputNormalizer>>, RuntimeError>;
    pub fn build_memory(&self, cfg: &RuntimeConfig) -> Result<Option<Arc<dyn SessionMemoryStore>>, RuntimeError>;
    pub fn build_audit(&self, cfg: &RuntimeConfig) -> Result<Arc<dyn AuditSink>, RuntimeError>;
    pub fn build_evaluators(&self, cfg: &RuntimeConfig) -> Result<EvaluatorSet, RuntimeError>;
}
```
> 「可插拔模組」＝ config 的 `assembly.*` 字串對照 registry 內註冊項；未知 id 在 `validate()` 即 `UnknownModule` 中止開機。新增實作＝在 `with_builtins`（或外部）註冊一個 id，不改 orchestrator。

## config 範例

```toml
# config/config.toml（新增）
[runtime]
intents    = "runtime/intents.toml"
lexicon    = "runtime/lexicon.toml"
thresholds = "runtime/thresholds.toml"
injection  = "runtime/injection.toml"

[runtime.pipeline]
input_stages = ["normalize","input_guard","injection","intent","slots"]
[runtime.answer_policy]
backend = "rule"
[runtime.llm_normalizer]
enabled = false
backend = "disabled"
[runtime.memory]
enabled = true
backend = "in-memory"
[runtime.audit]
sink = "stdout"
failure_policy = "fail-open"
[runtime.guardrails]
enabled = ["injection","input_guard","answer_policy"]
[runtime.slots]
extractors = ["time_range","metric","asset","rank_limit"]
[runtime.eval]
pipeline_evaluators = ["pipeline-deterministic"]
response_evaluators = ["response-baseline","llm-judge"]
fixtures = "runtime/evals/inputs.json"
baseline = "runtime/evals/response-baseline.json"
```

```toml
# config/runtime/thresholds.toml（input limits）
[input]
max_prompt_chars = 4000

# config/runtime/thresholds.toml（含完整 classifier）
[confidence]
answer_normal = 0.7
answer_gray = 0.5
option_path = 0.95
llm_override_floor = 0.8

[classifier]
option_match_confidence = 0.95
text_override_confidence = 0.90
unknown_confidence = 0.25
no_score_floor = 0.20
ambiguous_confidence = 0.55
margin_tiers = [
  { min_margin = 3, confidence = 0.92 },
  { min_margin = 2, confidence = 0.85 },
  { min_margin = 1, confidence = 0.72 },
]
keyword_long_chars = 4
keyword_long_weight = 2
keyword_short_weight = 1

[memory]
max_turns = 5
user_summary_chars = 160
answer_summary_chars = 300
max_memory_context_chars = 1200
max_memory_injected_prompt_chars = 2000

[registry_versions]
intent = "chief-of-staff.intent.v1"
slots = "chief-of-staff.slots.v1"
```

```toml
# config/runtime/lexicon.toml（asset 走 allowlist，取代硬編）
asset_allowlist = ["starcharger", "星舟快充", "hdrenewables", "泓德"]
top_n_pattern = "(?:top\\s*|前\\s*)(\\d{1,2})"

[metric_aliases]
"capture rate" = "capture_rate"
"捕獲率" = "capture_rate"

[[time_range]]
pattern = "近六個月|近半年|近6個月"
label = "近六個月"
kind = "custom"
months = 6
```

## 測試（Rust 獨有，必測）
```rust
#[test] fn config_validate_rejects_unknown_module() { /* assembly 含未註冊 id（含 evaluator）→ UnknownModule */ }
#[test] fn config_validate_requires_unknown_intent() { /* allowlist 缺 "unknown" → Config error */ }
#[test] fn config_validate_option_prefix_in_allowlist() { /* option_prefixes 值不在 allowlist → 失敗 */ }
#[test] fn config_loads_full_classifier_block() { /* thresholds.classifier 全欄位載入 */ }
#[test] fn config_loads_input_limit_for_parity() { /* input.max_prompt_chars = 4000 */ }
#[test] fn registry_builds_answer_policy_and_evaluators() { /* rule policy + pipeline/response evaluators 可 build */ }
#[test] fn registry_builds_optional_llm_normalizer() { /* disabled -> None；unknown enabled backend -> UnknownModule */ }
```

## 錯誤處理
| PRD 對應 | 觸發 | 行為 | 實作 |
|---------|------|------|------|
| US-25 | config 載入/驗證失敗 | **中止開機** | `RuntimeError::Config/UnknownModule/IntentNotAllowed` |
