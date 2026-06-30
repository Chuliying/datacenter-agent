//! Lexicon + option-path intent classification.

use crate::runtime::config::RuntimeConfig;
use crate::runtime::schema::{IntentSource, RuntimeWarning};

/// Fallback intent id required by the runtime contract.
pub const UNKNOWN_INTENT: &str = "unknown";

/// Intent classification result.
#[derive(Debug, Clone, PartialEq)]
pub struct IntentResult {
    /// Intent id.
    pub intent: String,
    /// Confidence score.
    pub confidence: f32,
    /// Candidate intent ids.
    pub candidate_intents: Vec<String>,
    /// Classification source.
    pub source: IntentSource,
    /// Non-fatal warnings.
    pub warnings: Vec<RuntimeWarning>,
}

impl Default for IntentResult {
    fn default() -> Self {
        Self {
            intent: UNKNOWN_INTENT.to_string(),
            confidence: 0.25,
            candidate_intents: Vec::new(),
            source: IntentSource::Unknown,
            warnings: Vec::new(),
        }
    }
}

/// Classify intent from normalized prompt text and optional option id.
pub fn classify_intent(cfg: &RuntimeConfig, prompt: &str, option_id: Option<&str>) -> IntentResult {
    let mut warnings = Vec::new();
    let option_intent = option_id.and_then(|id| match option_prefix(id) {
        Some(prefix) => match cfg.option_prefixes.get(prefix) {
            Some(intent) => Some(intent.clone()),
            None => {
                warnings.push(RuntimeWarning {
                    code: "unknown_option_prefix".to_string(),
                    message: format!("unknown option prefix `{prefix}`"),
                });
                None
            }
        },
        None => None,
    });

    let text_result = classify_by_lexicon(cfg, prompt);

    match option_intent {
        Some(option_intent)
            if text_result.intent != UNKNOWN_INTENT
                && text_result.confidence >= cfg.thresholds.classifier.text_override_confidence
                && text_result.intent != option_intent =>
        {
            IntentResult {
                intent: text_result.intent.clone(),
                confidence: text_result.confidence,
                candidate_intents: vec![option_intent, text_result.intent],
                source: IntentSource::TextOverride,
                warnings,
            }
        }
        Some(option_intent) => IntentResult {
            intent: option_intent.clone(),
            confidence: cfg.thresholds.classifier.option_match_confidence,
            candidate_intents: vec![option_intent],
            source: IntentSource::OptionPath,
            warnings,
        },
        None => IntentResult {
            warnings,
            ..text_result
        },
    }
}

fn option_prefix(option_id: &str) -> Option<&str> {
    option_id
        .split(['.', ':', '/', '_', '-'])
        .next()
        .filter(|prefix| !prefix.is_empty())
}

fn classify_by_lexicon(cfg: &RuntimeConfig, prompt: &str) -> IntentResult {
    let mut scores = cfg
        .intents
        .iter()
        .map(|intent| {
            let score = intent
                .keywords
                .iter()
                .filter(|keyword| prompt.contains(&keyword.to_lowercase()))
                .map(|keyword| {
                    if keyword.chars().count() >= cfg.thresholds.classifier.keyword_long_chars {
                        cfg.thresholds.classifier.keyword_long_weight
                    } else {
                        cfg.thresholds.classifier.keyword_short_weight
                    }
                })
                .sum::<u32>();
            (intent.id.clone(), score)
        })
        .collect::<Vec<_>>();
    scores.sort_by(|left, right| right.1.cmp(&left.1).then_with(|| left.0.cmp(&right.0)));

    let Some((top_intent, top_score)) = scores.first().cloned() else {
        return IntentResult::default();
    };
    if top_score == 0 {
        return IntentResult {
            confidence: cfg.thresholds.classifier.unknown_confidence,
            ..IntentResult::default()
        };
    }

    let second_score = scores.get(1).map(|(_, score)| *score).unwrap_or(0);
    let margin = top_score.saturating_sub(second_score);
    let confidence = cfg
        .thresholds
        .classifier
        .margin_tiers
        .iter()
        .find(|tier| margin >= tier.min_margin)
        .map(|tier| tier.confidence)
        .unwrap_or(cfg.thresholds.classifier.ambiguous_confidence);

    IntentResult {
        intent: top_intent.clone(),
        confidence,
        candidate_intents: scores
            .into_iter()
            .filter(|(_, score)| *score > 0)
            .map(|(intent, _)| intent)
            .collect(),
        source: IntentSource::RuleLexicon,
        warnings: Vec::new(),
    }
}
