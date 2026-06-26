//! Response baseline loading.

use std::path::Path;

use serde::Deserialize;

use crate::runtime::error::{RuntimeError, RuntimeResult};

/// Baseline provenance status.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum BaselineStatus {
    /// Baseline is intentionally not available yet.
    Pending,
    /// Baseline was loaded from a replay artifact.
    Loaded,
}

/// Response replay baseline.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ResponseBaseline {
    /// Baseline status.
    pub status: BaselineStatus,
    /// Source/provenance note for the cases.
    pub provenance: String,
    /// Replay cases.
    #[serde(default)]
    pub cases: Vec<ResponseCase>,
}

/// One replay response case.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ResponseCase {
    /// Stable case id.
    pub id: String,
    /// Original user prompt.
    pub prompt: String,
    /// Expected response from TS or approved sample.
    #[serde(default)]
    pub expected_response: Option<String>,
    /// Actual replay response to compare.
    #[serde(default)]
    pub actual_response: Option<String>,
    /// Response substrings that must be present.
    #[serde(default)]
    pub must_include: Vec<String>,
    /// Response substrings that must be absent.
    #[serde(default)]
    pub must_not_include: Vec<String>,
    /// Maximum allowed latency in milliseconds.
    #[serde(default)]
    pub max_latency_ms: Option<u64>,
    /// Maximum allowed token count.
    #[serde(default)]
    pub max_tokens: Option<u64>,
    /// Expected refusal flag.
    #[serde(default)]
    pub expected_refused: Option<bool>,
    /// Expected fallback flag.
    #[serde(default)]
    pub expected_fallback: Option<bool>,
    /// Observed latency in milliseconds.
    #[serde(default)]
    pub latency_ms: Option<u64>,
    /// Observed token count.
    #[serde(default)]
    pub tokens: Option<u64>,
    /// Whether this case is a refusal.
    #[serde(default)]
    pub refused: bool,
    /// Whether this case used fallback behavior.
    #[serde(default)]
    pub fallback: bool,
}

/// Load a response replay baseline from JSON.
pub fn load_response_baseline(path: &Path) -> RuntimeResult<ResponseBaseline> {
    let text = std::fs::read_to_string(path)
        .map_err(|err| RuntimeError::Config(format!("read {}: {err}", path.display())))?;
    let baseline: ResponseBaseline = serde_json::from_str(&text)
        .map_err(|err| RuntimeError::Config(format!("parse {}: {err}", path.display())))?;
    if baseline.provenance.trim().is_empty() {
        return Err(RuntimeError::Config(format!(
            "{} response baseline provenance is empty",
            path.display()
        )));
    }
    Ok(baseline)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_response_baseline_records_pending_provenance() {
        let baseline =
            load_response_baseline(Path::new("config/runtime/evals/response-baseline.json"))
                .expect("baseline should parse");

        assert_eq!(baseline.status, BaselineStatus::Pending);
        assert!(!baseline.provenance.trim().is_empty());
    }
}
