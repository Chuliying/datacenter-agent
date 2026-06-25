//! Eval runner skeleton.

use crate::config::AppConfig;
use crate::runtime::error::RuntimeResult;
use crate::runtime::eval::fixtures::load_pipeline_fixtures;
use crate::runtime::eval::report::EvalReport;
use crate::runtime::input::pipeline::InputPipeline;
use crate::runtime::{config::RuntimeConfig, registry::BuiltinRegistry};

/// Eval mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EvalMode {
    /// Pipeline-only mode is CI-safe and does not require provider credentials.
    PipelineOnly,
}

/// Run evals in the requested mode.
pub fn run(mode: EvalMode) -> RuntimeResult<EvalReport> {
    match mode {
        EvalMode::PipelineOnly => run_pipeline_only(),
    }
}

fn run_pipeline_only() -> RuntimeResult<EvalReport> {
    let app_config = AppConfig::load("config/config.toml")
        .map_err(|err| crate::runtime::error::RuntimeError::Config(err.to_string()))?;
    let refs = app_config.runtime.ok_or_else(|| {
        crate::runtime::error::RuntimeError::Config("runtime refs missing".into())
    })?;
    let registry = BuiltinRegistry::default();
    let runtime_config = RuntimeConfig::load(&refs, &registry)?;
    let fixtures = load_pipeline_fixtures(&refs.eval_fixtures)?;
    let pipeline = InputPipeline::default();

    let mut report = EvalReport::default();
    for fixture in fixtures {
        let input = pipeline.run_with_config(
            &runtime_config,
            &fixture.prompt,
            fixture.option_id.as_deref(),
        )?;
        let passed = input.intent == fixture.expected_intent
            && input.slots.metric == fixture.expected_metric
            && input.slots.asset == fixture.expected_asset
            && input.slots.rank_limit == fixture.expected_rank_limit;
        if passed {
            report.passed += 1;
        } else {
            report.failed += 1;
        }
    }

    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pipeline_only_runs_default_pack_fixtures() {
        let report = run(EvalMode::PipelineOnly).expect("pipeline eval should run");

        assert!(report.passed > 0);
        assert_eq!(report.failed, 0);
    }
}
