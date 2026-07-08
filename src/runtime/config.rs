//! Runtime capability pack configuration.

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

use serde::Deserialize;

use super::audit::AuditFailurePolicy;
use super::error::{RuntimeError, RuntimeResult};
use super::guardrails::injection::InjectionDetector;
use super::registry::BuiltinRegistry;

/// References from the host config to runtime pack files.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeRefs {
    /// Intent config path.
    pub intents: Option<PathBuf>,
    /// Lexicon config path.
    pub lexicon: Option<PathBuf>,
    /// Threshold config path.
    pub thresholds: Option<PathBuf>,
    /// Injection config path.
    pub injection: Option<PathBuf>,
}

/// Runtime input limits.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct InputConfig {
    /// Maximum prompt length in Unicode scalar values.
    pub max_prompt_chars: usize,
}

impl Default for InputConfig {
    fn default() -> Self {
        Self {
            max_prompt_chars: 4_000,
        }
    }
}

/// Runtime confidence thresholds.
#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ConfidenceThresholds {
    /// Confidence at or above this value can answer normally.
    pub answer_normal: f32,
    /// Below this value should refuse or fallback.
    pub answer_gray: f32,
    /// Option-path confidence.
    pub option_path: f32,
    /// Minimum confidence for LLM override.
    pub llm_override_floor: f32,
}

/// One classifier margin tier.
#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct MarginTier {
    /// Minimum score margin.
    pub min_margin: u32,
    /// Confidence assigned for this margin.
    pub confidence: f32,
}

/// Intent classifier tuning.
#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ClassifierTuning {
    /// Confidence for known option ids.
    pub option_match_confidence: f32,
    /// Confidence at which text can override option path.
    pub text_override_confidence: f32,
    /// Confidence for unknown intent.
    pub unknown_confidence: f32,
    /// Floor when no keyword scores.
    pub no_score_floor: f32,
    /// Confidence for ambiguous results.
    pub ambiguous_confidence: f32,
    /// Margin-to-confidence tiers.
    pub margin_tiers: Vec<MarginTier>,
    /// Long keyword character threshold.
    pub keyword_long_chars: usize,
    /// Long keyword score weight.
    pub keyword_long_weight: u32,
    /// Short keyword score weight.
    pub keyword_short_weight: u32,
}

/// Memory context limits.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct MemoryLimits {
    /// Maximum remembered turns.
    pub max_turns: usize,
    /// Maximum memory context chars injected into a prompt.
    pub max_memory_context_chars: usize,
}

/// Runtime threshold pack.
#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Thresholds {
    /// Input limits.
    pub input: InputConfig,
    /// Answer confidence thresholds.
    pub confidence: ConfidenceThresholds,
    /// Classifier tuning.
    pub classifier: ClassifierTuning,
    /// Memory limits.
    pub memory: MemoryLimits,
}

/// One configured intent.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct IntentDef {
    /// Intent id.
    pub id: String,
    /// Intent keywords.
    pub keywords: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct IntentPack {
    intent_allowlist: Vec<String>,
    #[serde(default)]
    option_prefixes: BTreeMap<String, String>,
    #[serde(default)]
    intents: Vec<IntentDef>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct LexiconPack {
    #[serde(default)]
    metric_aliases: BTreeMap<String, String>,
    #[serde(default)]
    asset_allowlist: HashSet<String>,
}

/// Runtime injection pattern pack.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct InjectionRuleSet {
    /// Injection pattern version.
    pub version: u32,
    /// Regex patterns.
    pub patterns: Vec<String>,
}

/// Runtime module assembly.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Assembly {
    /// Ordered input pipeline stage ids.
    pub input_stages: Vec<String>,
    /// Answer policy backend id.
    pub answer_policy_backend: String,
    /// Whether LLM normalizer is enabled.
    pub llm_normalizer_enabled: bool,
    /// LLM normalizer backend id.
    pub llm_normalizer_backend: String,
    /// Whether server memory is enabled.
    pub memory_enabled: bool,
    /// Memory backend id.
    pub memory_backend: String,
    /// Audit sink id.
    pub audit_sink: String,
    /// Audit failure policy.
    pub audit_failure_policy: AuditFailurePolicy,
    /// Enabled guardrails.
    pub guardrails: Vec<String>,
    /// Enabled slot extractors.
    pub extractors: Vec<String>,
    /// Enabled pipeline evaluators.
    pub pipeline_evaluators: Vec<String>,
    /// Enabled response evaluators.
    pub response_evaluators: Vec<String>,
    /// Eval fixtures path.
    pub fixtures: PathBuf,
    /// Response baseline path.
    pub baseline: PathBuf,
}

/// Resolved runtime configuration.
#[derive(Debug, Clone)]
pub struct RuntimeConfig {
    /// Intent allowlist.
    pub intent_allowlist: Vec<String>,
    /// Intent definitions.
    pub intents: Vec<IntentDef>,
    /// Option id prefix to intent map.
    pub option_prefixes: BTreeMap<String, String>,
    /// Metric aliases.
    pub metric_aliases: BTreeMap<String, String>,
    /// Asset allowlist.
    pub asset_allowlist: HashSet<String>,
    /// Thresholds.
    pub thresholds: Thresholds,
    /// Input limits.
    pub input: InputConfig,
    /// Compiled prompt-injection detector. Authoritative source of injection
    /// rules at runtime; the raw pattern pack is consumed at load and not
    /// retained, so there is nothing to drift out of sync with.
    pub injection_detector: InjectionDetector,
    /// Runtime module assembly.
    pub assembly: Assembly,
    /// Ordered input stage ids.
    pub input_stages: Vec<String>,
}

impl RuntimeConfig {
    /// Load a runtime capability pack from resolved host refs.
    ///
    /// # Errors
    ///
    /// Returns config errors for malformed files or validation errors for unknown
    /// ids and inconsistent allowlists.
    pub fn load(
        refs: &crate::config::RuntimeRefs,
        registry: &BuiltinRegistry,
    ) -> RuntimeResult<Self> {
        let intent_pack: IntentPack = read_toml(&refs.intents)?;
        let lexicon_pack: LexiconPack = read_toml(&refs.lexicon)?;
        let thresholds: Thresholds = read_toml(&refs.thresholds)?;
        let injection: InjectionRuleSet = read_toml(&refs.injection)?;
        let injection_detector =
            InjectionDetector::new(injection.version, injection.patterns.clone())
                .map_err(|err| RuntimeError::Config(format!("invalid injection pattern: {err}")))?;
        let audit_failure_policy = parse_audit_failure_policy(&refs.audit_failure_policy)?;

        let assembly = Assembly {
            input_stages: refs.input_stages.clone(),
            answer_policy_backend: refs.answer_policy_backend.clone(),
            llm_normalizer_enabled: refs.llm_normalizer_enabled,
            llm_normalizer_backend: refs.llm_normalizer_backend.clone(),
            memory_enabled: refs.memory_enabled,
            memory_backend: refs.memory_backend.clone(),
            audit_sink: refs.audit_sink.clone(),
            audit_failure_policy,
            guardrails: refs.guardrails.clone(),
            extractors: refs.slot_extractors.clone(),
            pipeline_evaluators: refs.pipeline_evaluators.clone(),
            response_evaluators: refs.response_evaluators.clone(),
            fixtures: refs.eval_fixtures.clone(),
            baseline: refs.response_baseline.clone(),
        };

        let cfg = Self {
            intent_allowlist: intent_pack.intent_allowlist,
            intents: intent_pack.intents,
            option_prefixes: intent_pack.option_prefixes,
            metric_aliases: lexicon_pack.metric_aliases,
            asset_allowlist: lexicon_pack.asset_allowlist,
            input: thresholds.input.clone(),
            thresholds,
            injection_detector,
            input_stages: assembly.input_stages.clone(),
            assembly,
        };
        cfg.validate(registry)?;
        Ok(cfg)
    }

    /// Validate runtime config consistency and registry references.
    pub fn validate(&self, registry: &BuiltinRegistry) -> RuntimeResult<()> {
        if !self.intent_allowlist.iter().any(|id| id == "unknown") {
            return Err(RuntimeError::IntentNotAllowed("unknown".to_string()));
        }

        let allowlist: HashSet<&str> = self.intent_allowlist.iter().map(String::as_str).collect();
        let mut seen = HashSet::new();
        for intent in &self.intents {
            if !allowlist.contains(intent.id.as_str()) {
                return Err(RuntimeError::IntentNotAllowed(intent.id.clone()));
            }
            if !seen.insert(intent.id.as_str()) {
                return Err(RuntimeError::Config(format!(
                    "duplicate intent id `{}`",
                    intent.id
                )));
            }
            if intent.keywords.is_empty() {
                return Err(RuntimeError::Config(format!(
                    "intent `{}` must define at least one keyword",
                    intent.id
                )));
            }
        }

        for target in self.option_prefixes.values() {
            if !allowlist.contains(target.as_str()) {
                return Err(RuntimeError::IntentNotAllowed(target.clone()));
            }
        }
        for stage in &self.assembly.input_stages {
            registry.require_input_stage(stage, "runtime.pipeline")?;
        }
        registry.require_answer_policy(
            &self.assembly.answer_policy_backend,
            "runtime.answer_policy",
        )?;
        registry.require_llm_normalizer(
            &self.assembly.llm_normalizer_backend,
            "runtime.llm_normalizer",
        )?;
        registry.require_memory_backend(&self.assembly.memory_backend, "runtime.memory")?;
        registry.require_audit_sink(&self.assembly.audit_sink, "runtime.audit")?;
        for guardrail in &self.assembly.guardrails {
            registry.require_guardrail(guardrail, "runtime.guardrails")?;
        }
        for extractor in &self.assembly.extractors {
            registry.require_slot_extractor(extractor, "runtime.slots")?;
        }
        for evaluator in &self.assembly.pipeline_evaluators {
            registry.require_evaluator(evaluator, "runtime.eval")?;
        }
        for evaluator in &self.assembly.response_evaluators {
            registry.require_evaluator(evaluator, "runtime.eval")?;
        }

        Ok(())
    }
}

fn read_toml<T: for<'de> Deserialize<'de>>(path: &Path) -> RuntimeResult<T> {
    let text = std::fs::read_to_string(path)
        .map_err(|err| RuntimeError::Config(format!("read {}: {err}", path.display())))?;
    toml::from_str(&text)
        .map_err(|err| RuntimeError::Config(format!("parse {}: {err}", path.display())))
}

fn parse_audit_failure_policy(value: &str) -> RuntimeResult<AuditFailurePolicy> {
    match value {
        "fail-open" => Ok(AuditFailurePolicy::FailOpen),
        "fail-closed" => Ok(AuditFailurePolicy::FailClosed),
        other => Err(RuntimeError::UnknownModule {
            id: other.to_string(),
            section: "runtime.audit".to_string(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use crate::config::AppConfig;
    use crate::runtime::audit::AuditFailurePolicy;
    use crate::runtime::error::RuntimeError;
    use crate::runtime::registry::BuiltinRegistry;

    use super::*;

    fn default_runtime_refs() -> crate::config::RuntimeRefs {
        AppConfig::load("config/config.toml")
            .expect("app config should load")
            .runtime
            .expect("runtime refs should exist")
    }

    #[test]
    fn loads_ev_capability_pack_from_default_config() {
        let refs = default_runtime_refs();
        let registry = BuiltinRegistry::default();

        let cfg = RuntimeConfig::load(&refs, &registry).expect("runtime config should load");

        assert_eq!(cfg.input.max_prompt_chars, 4_000);
        assert!(cfg.intent_allowlist.iter().any(|id| id == "unknown"));
        assert!(cfg.intent_allowlist.iter().any(|id| id == "revenue"));
        assert_eq!(
            cfg.input_stages,
            ["normalize", "input_guard", "injection", "intent", "slots"]
        );
        assert_eq!(cfg.assembly.answer_policy_backend, "rule");
        assert_eq!(
            cfg.assembly.audit_failure_policy,
            AuditFailurePolicy::FailOpen
        );
        assert!(cfg.thresholds.confidence.answer_normal > cfg.thresholds.confidence.answer_gray);
        assert_eq!(cfg.thresholds.classifier.option_match_confidence, 0.95);
        assert_eq!(cfg.thresholds.memory.max_memory_context_chars, 1200);
        assert_eq!(cfg.injection_detector.version(), 1);
        assert!(cfg.metric_aliases.contains_key("營收"));
        assert!(cfg.asset_allowlist.contains("站點"));
    }

    #[test]
    fn rejects_unknown_assembly_module_ids() {
        let mut refs = default_runtime_refs();
        refs.input_stages.push("not-registered".to_string());
        let registry = BuiltinRegistry::default();

        let err = RuntimeConfig::load(&refs, &registry).expect_err("unknown stage should fail");

        assert!(matches!(
            err,
            RuntimeError::UnknownModule { ref id, ref section }
                if id == "not-registered" && section == "runtime.pipeline"
        ));
    }

    #[test]
    fn rejects_missing_unknown_intent() {
        let refs = default_runtime_refs();
        let registry = BuiltinRegistry::default();
        let mut cfg = RuntimeConfig::load(&refs, &registry).expect("runtime config should load");
        cfg.intent_allowlist.retain(|id| id != "unknown");

        let err = cfg
            .validate(&registry)
            .expect_err("missing unknown should fail");

        assert!(matches!(
            err,
            RuntimeError::IntentNotAllowed(ref id) if id == "unknown"
        ));
    }

    #[test]
    fn rejects_invalid_injection_regex() {
        // The injection pack is validated at load by compiling the detector;
        // there is no post-load mutation hook because the raw rules are not
        // retained. Point the injection ref at a pack with a bad pattern.
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default();
        let bad_path = std::env::temp_dir().join(format!(
            "runtime-bad-injection-{nanos}-{:?}.toml",
            std::thread::current().id()
        ));
        std::fs::write(&bad_path, "version = 1\npatterns = [\"(\"]\n")
            .expect("temp injection pack should write");

        let mut refs = default_runtime_refs();
        refs.injection = bad_path.clone();
        let registry = BuiltinRegistry::default();

        let err =
            RuntimeConfig::load(&refs, &registry).expect_err("invalid injection regex should fail");
        std::fs::remove_file(&bad_path).ok();

        assert!(
            matches!(err, RuntimeError::Config(ref msg) if msg.contains("invalid injection pattern"))
        );
    }
}
