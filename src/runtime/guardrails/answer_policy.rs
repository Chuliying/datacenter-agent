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

/// The **intent-filtering-free** policy: answer every prompt, whatever its intent or confidence.
///
/// [`RuleAnswerPolicy`] refuses `unknown` / low-confidence input as `off_scope`, which is right for
/// a front door whose intent pack covers its domain. `/ss-chat` has no such pack — the runtime's
/// `intents.toml` is the EV-charging one, so every 星星電力 question ("這一季的日照條件怎麼樣？")
/// classifies as `unknown` and would be refused before the pipeline ever ran. That endpoint
/// therefore substitutes this policy: intent still resolves (and is still audited and emitted as
/// `intent.resolved`), it just no longer gates the answer.
///
/// **Prompt injection is still refused.** That refusal is a security guardrail, not intent
/// filtering, so it is kept verbatim from [`RuleAnswerPolicy`] — this policy relaxes scope, not
/// safety, and the rest of the prelude (prompt-length validation, the injection detector, audit,
/// memory) is unchanged.
#[derive(Debug, Clone, Copy, Default)]
pub struct AlwaysAnswerPolicy;

impl AnswerPolicy for AlwaysAnswerPolicy {
    fn decide(&self, input: &NormalizedInput) -> AnswerDecision {
        if input
            .warnings
            .iter()
            .any(|warning| warning.code == "prompt_injection_detected")
        {
            return AnswerDecision::Refuse("prompt_injection".to_string());
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

    #[test]
    fn always_answer_policy_answers_unknown_and_low_confidence() {
        // The exact two cases `RuleAnswerPolicy` refuses as `off_scope`. Every SS question lands
        // here, because the runtime's intent pack is the EV-charging one.
        assert_eq!(
            AlwaysAnswerPolicy.decide(&normalized("unknown", 0.25, Vec::new())),
            AnswerDecision::Answer
        );
        assert_eq!(
            AlwaysAnswerPolicy.decide(&normalized("revenue", 0.3, Vec::new())),
            AnswerDecision::Answer
        );
        // No low-confidence disclaimer either — there is no scope to be unsure about.
        assert_eq!(
            AlwaysAnswerPolicy.decide(&normalized("revenue", 0.6, Vec::new())),
            AnswerDecision::Answer
        );
    }

    #[test]
    fn always_answer_policy_still_refuses_prompt_injection() {
        // Relaxing scope must not relax safety: the injection refusal is kept verbatim.
        let input = normalized(
            "unknown",
            0.25,
            vec![RuntimeWarning {
                code: "prompt_injection_detected".to_string(),
                message: "matched injection heuristic".to_string(),
            }],
        );

        assert_eq!(
            AlwaysAnswerPolicy.decide(&input),
            AnswerDecision::Refuse("prompt_injection".to_string())
        );
    }
}
