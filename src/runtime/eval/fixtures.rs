//! Eval fixture loading skeleton.

use std::path::Path;

use serde::Deserialize;

use crate::runtime::error::{RuntimeError, RuntimeResult};

/// One pipeline eval fixture.
#[derive(Debug, Clone, Deserialize)]
pub struct PipelineFixture {
    /// Fixture id.
    pub id: String,
    /// User prompt.
    pub prompt: String,
    /// Optional option id.
    #[serde(default)]
    pub option_id: Option<String>,
    /// Expected intent id.
    pub expected_intent: String,
    /// Expected metric slot.
    #[serde(default)]
    pub expected_metric: Option<String>,
    /// Expected asset slot.
    #[serde(default)]
    pub expected_asset: Option<String>,
    /// Expected rank limit slot.
    #[serde(default)]
    pub expected_rank_limit: Option<u32>,
}

/// Load pipeline fixtures from JSON.
pub fn load_pipeline_fixtures(path: &Path) -> RuntimeResult<Vec<PipelineFixture>> {
    let text = std::fs::read_to_string(path)
        .map_err(|err| RuntimeError::Config(format!("read {}: {err}", path.display())))?;
    serde_json::from_str(&text)
        .map_err(|err| RuntimeError::Config(format!("parse {}: {err}", path.display())))
}
