//! Slot extraction skeleton.

use crate::runtime::config::RuntimeConfig;
use crate::runtime::schema::{NormalizedSlots, RuntimeWarning};

/// Extract slots from normalized text.
pub trait SlotExtractor: Send + Sync {
    /// Extract slots from prompt text.
    fn extract(&self, prompt: &str, slots: &mut NormalizedSlots);
}

/// Slot extraction result.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SlotExtraction {
    /// Extracted slots.
    pub slots: NormalizedSlots,
    /// Non-fatal extraction warnings.
    pub warnings: Vec<RuntimeWarning>,
}

/// Extract configured runtime slots from normalized prompt text.
pub fn extract_slots(cfg: &RuntimeConfig, prompt: &str) -> SlotExtraction {
    let slots = NormalizedSlots {
        time_range: extract_time_range(prompt),
        metric: cfg.metric_aliases.iter().find_map(|(alias, metric)| {
            prompt
                .contains(&alias.to_lowercase())
                .then(|| metric.clone())
        }),
        asset: cfg.asset_allowlist.iter().find_map(|asset| {
            prompt
                .contains(&asset.to_lowercase())
                .then(|| asset.clone())
        }),
        rank_limit: extract_rank_limit(prompt),
    };

    let mut warnings = Vec::new();
    if slots.asset.is_none() && has_unknown_asset_hint(prompt) {
        warnings.push(RuntimeWarning {
            code: "unknown_asset".to_string(),
            message: "asset-like token is not in runtime asset allowlist".to_string(),
        });
    }

    SlotExtraction { slots, warnings }
}

fn extract_time_range(prompt: &str) -> Option<String> {
    ["近六個月", "近三個月", "這個月", "本月", "去年", "今年"]
        .iter()
        .find_map(|label| prompt.contains(label).then(|| (*label).to_string()))
}

fn extract_rank_limit(prompt: &str) -> Option<u32> {
    let captures = regex::Regex::new(r"(?i)\btop\s*(\d{1,3})\b")
        .ok()?
        .captures(prompt)?;
    captures.get(1)?.as_str().parse().ok()
}

fn has_unknown_asset_hint(prompt: &str) -> bool {
    prompt
        .split_whitespace()
        .any(|token| token.chars().any(|ch| ch.is_ascii_alphabetic()))
}
