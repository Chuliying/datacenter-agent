//! Ordered input pipeline skeleton.

use crate::runtime::config::RuntimeConfig;
use crate::runtime::error::RuntimeResult;
use crate::runtime::schema::{IntentSource, NormalizedInput, NormalizedSlots};

use super::intent::{classify_intent, UNKNOWN_INTENT};
use super::normalizer::normalize_text;
use super::slots::extract_slots;

/// Deterministic pipeline.
#[derive(Debug, Clone, Default)]
pub struct InputPipeline {
    /// Ordered stage ids.
    pub stages: Vec<String>,
}

impl InputPipeline {
    /// Run the currently implemented skeleton pipeline.
    pub fn run(&self, prompt: &str) -> RuntimeResult<NormalizedInput> {
        Ok(NormalizedInput {
            prompt: normalize_text(prompt),
            intent: UNKNOWN_INTENT.to_string(),
            confidence: 0.25,
            candidate_intents: Vec::new(),
            intent_source: Some(IntentSource::Unknown),
            slots: NormalizedSlots::default(),
            warnings: Vec::new(),
        })
    }

    /// Run the deterministic input pipeline with runtime config.
    pub fn run_with_config(
        &self,
        cfg: &RuntimeConfig,
        prompt: &str,
        option_id: Option<&str>,
    ) -> RuntimeResult<NormalizedInput> {
        let clean_prompt = normalize_text(prompt);
        let intent = classify_intent(cfg, &clean_prompt, option_id);
        let slot_extraction = extract_slots(cfg, &clean_prompt);
        let mut warnings = intent.warnings;
        warnings.extend(slot_extraction.warnings);
        Ok(NormalizedInput {
            prompt: clean_prompt.clone(),
            intent: intent.intent,
            confidence: intent.confidence,
            candidate_intents: intent.candidate_intents,
            intent_source: Some(intent.source),
            slots: slot_extraction.slots,
            warnings,
        })
    }
}

#[cfg(test)]
mod tests {
    use crate::config::AppConfig;
    use crate::runtime::config::RuntimeConfig;
    use crate::runtime::registry::BuiltinRegistry;
    use crate::runtime::schema::IntentSource;

    use super::*;

    fn default_runtime_config() -> RuntimeConfig {
        let refs = AppConfig::load("config/config.toml")
            .expect("app config should load")
            .runtime
            .expect("runtime refs should exist");
        RuntimeConfig::load(&refs, &BuiltinRegistry::default()).expect("runtime config should load")
    }

    #[test]
    fn option_id_maps_to_option_path_intent() {
        let cfg = default_runtime_config();
        let pipeline = InputPipeline::default();

        let input = pipeline
            .run_with_config(&cfg, "本月狀況", Some("charging.monthly"))
            .expect("pipeline should run");

        assert_eq!(input.intent, "charging");
        assert_eq!(input.confidence, 0.95);
        assert_eq!(input.intent_source, Some(IntentSource::OptionPath));
        assert_eq!(input.candidate_intents, ["charging"]);
    }

    #[test]
    fn text_override_beats_option_path_when_confident() {
        let cfg = default_runtime_config();
        let pipeline = InputPipeline::default();

        let input = pipeline
            .run_with_config(&cfg, "revenue 營收 收入 賺多少", Some("charging.monthly"))
            .expect("pipeline should run");

        assert_eq!(input.intent, "revenue");
        assert!(input.confidence >= cfg.thresholds.classifier.text_override_confidence);
        assert_eq!(input.intent_source, Some(IntentSource::TextOverride));
        assert_eq!(input.candidate_intents, ["charging", "revenue"]);
    }

    #[test]
    fn extracts_time_metric_asset_and_rank_slots() {
        let cfg = default_runtime_config();
        let pipeline = InputPipeline::default();

        let input = pipeline
            .run_with_config(&cfg, "近六個月站點建置 top 5 營收", None)
            .expect("pipeline should run");

        assert_eq!(input.slots.time_range.as_deref(), Some("近六個月"));
        assert_eq!(input.slots.metric.as_deref(), Some("revenue"));
        assert_eq!(input.slots.asset.as_deref(), Some("站點"));
        assert_eq!(input.slots.rank_limit, Some(5));
    }

    #[test]
    fn unknown_option_prefix_warns_and_falls_back_to_text() {
        let cfg = default_runtime_config();
        let pipeline = InputPipeline::default();

        let input = pipeline
            .run_with_config(&cfg, "營收 收入 賺多少", Some("mystery.card"))
            .expect("pipeline should run");

        assert_eq!(input.intent, "revenue");
        assert_eq!(input.intent_source, Some(IntentSource::RuleLexicon));
        assert!(input
            .warnings
            .iter()
            .any(|warning| warning.code == "unknown_option_prefix"));
    }

    #[test]
    fn unknown_asset_warns_without_hardcoded_allowance() {
        let cfg = default_runtime_config();
        let pipeline = InputPipeline::default();

        let input = pipeline
            .run_with_config(&cfg, "Zeta 充電量", None)
            .expect("pipeline should run");

        assert_eq!(input.slots.asset, None);
        assert!(input
            .warnings
            .iter()
            .any(|warning| warning.code == "unknown_asset"));
    }
}
