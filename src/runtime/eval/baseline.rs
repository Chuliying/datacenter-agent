//! Response baseline loading.

use std::collections::HashSet;
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
    validate_response_baseline(path, &baseline)?;
    Ok(baseline)
}

fn validate_response_baseline(path: &Path, baseline: &ResponseBaseline) -> RuntimeResult<()> {
    if baseline.status == BaselineStatus::Loaded && baseline.cases.is_empty() {
        return Err(RuntimeError::Config(format!(
            "{} loaded response baseline has no cases",
            path.display()
        )));
    }

    let mut ids = HashSet::new();
    for case in &baseline.cases {
        if case.id.trim().is_empty() {
            return Err(RuntimeError::Config(format!(
                "{} response baseline has a case with empty id",
                path.display()
            )));
        }
        if !ids.insert(case.id.as_str()) {
            return Err(RuntimeError::Config(format!(
                "{} response baseline has duplicate case id `{}`",
                path.display(),
                case.id
            )));
        }
        if case.prompt.trim().is_empty() {
            return Err(RuntimeError::Config(format!(
                "{} response baseline case `{}` has empty prompt",
                path.display(),
                case.id
            )));
        }
    }

    Ok(())
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

    #[test]
    fn loaded_response_baseline_requires_cases() {
        let err = serde_json::from_str::<ResponseBaseline>(
            r#"{"status":"loaded","provenance":"unit test","cases":[]}"#,
        )
        .map_err(|err| RuntimeError::Config(err.to_string()))
        .and_then(|baseline| validate_response_baseline(Path::new("baseline.json"), &baseline))
        .expect_err("loaded baseline with no cases should fail");

        assert!(err.to_string().contains("has no cases"));
    }

    #[test]
    fn response_baseline_rejects_duplicate_case_ids() {
        let baseline = serde_json::from_str::<ResponseBaseline>(
            r#"{
              "status": "loaded",
              "provenance": "unit test",
              "cases": [
                {"id":"same","prompt":"營收"},
                {"id":"same","prompt":"站點"}
              ]
            }"#,
        )
        .expect("baseline JSON should parse");

        let err = validate_response_baseline(Path::new("baseline.json"), &baseline)
            .expect_err("duplicate ids should fail");

        assert!(err.to_string().contains("duplicate case id"));
    }

    #[test]
    fn response_baseline_rejects_empty_case_prompt() {
        let baseline = serde_json::from_str::<ResponseBaseline>(
            r#"{
              "status": "loaded",
              "provenance": "unit test",
              "cases": [
                {"id":"empty-prompt","prompt":"   "}
              ]
            }"#,
        )
        .expect("baseline JSON should parse");

        let err = validate_response_baseline(Path::new("baseline.json"), &baseline)
            .expect_err("empty prompt should fail");

        assert!(err.to_string().contains("empty prompt"));
    }
}
