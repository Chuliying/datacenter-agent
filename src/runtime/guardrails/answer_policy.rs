//! Confidence-driven answer policy.

use crate::runtime::config::ConfidenceThresholds;
use crate::runtime::schema::NormalizedInput;

/// Policy decision for one normalized input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AnswerDecision {
    /// Continue normally.
    Answer,
    /// Emit disclaimer before continuing.
    Disclaimer(String),
    /// Refuse semantically with a 200 response.
    Refuse(String),
}

/// Answer policy trait.
pub trait AnswerPolicy: Send + Sync {
    /// Decide how to handle the normalized input.
    fn decide(&self, input: &NormalizedInput) -> AnswerDecision;
}

/// Confidence-driven rule policy.
///
/// Thresholds are sourced from runtime config (`[confidence]`) so tuning
/// `thresholds.toml` actually changes behavior — no magic numbers here.
#[derive(Debug, Clone)]
pub struct RuleAnswerPolicy {
    /// At or above this confidence, answer without a disclaimer.
    answer_normal: f32,
    /// Below this confidence (or `unknown` intent), refuse as off-scope.
    answer_gray: f32,
}

impl RuleAnswerPolicy {
    /// Build the policy from the runtime confidence thresholds.
    pub fn new(thresholds: &ConfidenceThresholds) -> Self {
        Self {
            answer_normal: thresholds.answer_normal,
            answer_gray: thresholds.answer_gray,
        }
    }
}

impl AnswerPolicy for RuleAnswerPolicy {
    fn decide(&self, input: &NormalizedInput) -> AnswerDecision {
        if input
            .warnings
            .iter()
            .any(|warning| warning.code == "prompt_injection_detected")
        {
            return AnswerDecision::Refuse("prompt_injection".to_string());
        }

        if input.intent == "unknown" || input.confidence < self.answer_gray {
            return AnswerDecision::Refuse("off_scope".to_string());
        }

        if input.confidence < self.answer_normal {
            return AnswerDecision::Disclaimer("low_confidence".to_string());
        }

        AnswerDecision::Answer
    }
}

#[cfg(test)]
mod tests {
    use crate::runtime::schema::{NormalizedInput, NormalizedSlots, RuntimeWarning};

    use super::*;

    fn policy() -> RuleAnswerPolicy {
        RuleAnswerPolicy::new(&ConfidenceThresholds {
            answer_normal: 0.7,
            answer_gray: 0.5,
            option_path: 0.95,
            llm_override_floor: 0.8,
        })
    }

    fn normalized(intent: &str, confidence: f32, warnings: Vec<RuntimeWarning>) -> NormalizedInput {
        NormalizedInput {
            prompt: "prompt".to_string(),
            intent: intent.to_string(),
            confidence,
            candidate_intents: Vec::new(),
            intent_source: None,
            slots: NormalizedSlots::default(),
            warnings,
        }
    }

    #[test]
    fn refuses_prompt_injection_warning() {
        let input = normalized(
            "revenue",
            0.95,
            vec![RuntimeWarning {
                code: "prompt_injection_detected".to_string(),
                message: "matched injection heuristic".to_string(),
            }],
        );

        assert_eq!(
            policy().decide(&input),
            AnswerDecision::Refuse("prompt_injection".to_string())
        );
    }

    #[test]
    fn refuses_unknown_or_low_confidence_off_scope() {
        assert_eq!(
            policy().decide(&normalized("unknown", 0.9, Vec::new())),
            AnswerDecision::Refuse("off_scope".to_string())
        );
        assert_eq!(
            policy().decide(&normalized("revenue", 0.3, Vec::new())),
            AnswerDecision::Refuse("off_scope".to_string())
        );
    }

    #[test]
    fn adds_disclaimer_for_gray_confidence() {
        assert_eq!(
            policy().decide(&normalized("revenue", 0.6, Vec::new())),
            AnswerDecision::Disclaimer("low_confidence".to_string())
        );
    }

    #[test]
    fn answers_when_confidence_is_clear() {
        assert_eq!(
            policy().decide(&normalized("revenue", 0.8, Vec::new())),
            AnswerDecision::Answer
        );
    }
}
