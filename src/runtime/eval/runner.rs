//! Eval runner.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use crate::appstate::{load_mcp_url, LlmDefaults, PromptBank};
use crate::config::AppConfig;
use crate::llm_connector;
use crate::mcp_client::McpClient;
use crate::model::GenerationConfig;
use crate::runtime::error::{RuntimeError, RuntimeResult};
use crate::runtime::eval::baseline::{load_response_baseline, BaselineStatus, ResponseCase};
use crate::runtime::eval::fixtures::load_pipeline_fixtures;
use crate::runtime::eval::report::EvalReport;
use crate::runtime::input::pipeline::InputPipeline;
use crate::runtime::{config::RuntimeConfig, registry::BuiltinRegistry};

/// Eval mode.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EvalMode {
    /// Pipeline-only mode is CI-safe and does not require provider credentials.
    PipelineOnly,
    /// Response replay mode compares actual recorded responses with expected baselines.
    ResponseReplay {
        /// Replay artifact path.
        artifact: PathBuf,
    },
    /// Live response mode runs against provider-backed agent behavior.
    ResponseLive {
        /// Approved response baseline path.
        baseline: PathBuf,
    },
}

/// Run evals in the requested mode.
pub fn run(mode: EvalMode) -> RuntimeResult<EvalReport> {
    match mode {
        EvalMode::PipelineOnly => run_pipeline_only(),
        EvalMode::ResponseReplay { artifact } => run_response_replay(&artifact),
        EvalMode::ResponseLive { baseline } => tokio::runtime::Runtime::new()
            .map_err(|err| RuntimeError::Internal(format!("build eval runtime: {err}")))?
            .block_on(run_response_live(&baseline)),
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

fn run_response_replay(path: &Path) -> RuntimeResult<EvalReport> {
    let baseline = load_response_baseline(path)?;
    if baseline.status == BaselineStatus::Pending || baseline.cases.is_empty() {
        return Err(RuntimeError::Config(format!(
            "{} response baseline is pending or empty",
            path.display()
        )));
    }

    let mut report = EvalReport {
        provenance: Some(baseline.provenance),
        ..EvalReport::default()
    };
    for case in baseline.cases {
        let Some(actual_response) = case.actual_response.as_deref() else {
            report.failed += 1;
            report
                .regressions
                .push(format!("{}: replay case missing actual_response", case.id));
            continue;
        };
        let observed = ObservedResponse {
            response: actual_response.to_string(),
            latency_ms: case.latency_ms.unwrap_or_default(),
            tokens: case.tokens.unwrap_or_default(),
            refused: case.refused,
            fallback: case.fallback,
        };
        let failures = response_case_failures(&case, &observed);
        if failures.is_empty() {
            report.passed += 1;
        } else {
            report.failed += 1;
            report.regressions.extend(
                failures
                    .into_iter()
                    .map(|failure| format!("{}: {failure}", case.id)),
            );
        }
        report.latency_ms += observed.latency_ms;
        report.tokens += observed.tokens;
        if observed.refused {
            report.refusals += 1;
        }
        if observed.fallback {
            report.fallbacks += 1;
        }
    }

    Ok(report)
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ObservedResponse {
    response: String,
    latency_ms: u64,
    tokens: u64,
    refused: bool,
    fallback: bool,
}

fn response_case_failures(case: &ResponseCase, observed: &ObservedResponse) -> Vec<String> {
    let mut failures = Vec::new();
    if let Some(expected) = case.expected_response.as_ref() {
        if &observed.response != expected {
            failures.push("actual response did not exactly match expected response".to_string());
        }
    }
    for needle in &case.must_include {
        if !observed.response.contains(needle) {
            failures.push(format!("response missing required substring `{needle}`"));
        }
    }
    for needle in &case.must_not_include {
        if observed.response.contains(needle) {
            failures.push(format!("response contained forbidden substring `{needle}`"));
        }
    }
    if let Some(max) = case.max_latency_ms {
        if observed.latency_ms > max {
            failures.push(format!(
                "latency {}ms exceeded budget {max}ms",
                observed.latency_ms
            ));
        }
    }
    if let Some(max) = case.max_tokens {
        if observed.tokens > max {
            failures.push(format!("tokens {} exceeded budget {max}", observed.tokens));
        }
    }
    if let Some(expected) = case.expected_refused {
        if observed.refused != expected {
            failures.push(format!(
                "refused={} did not match expected {expected}",
                observed.refused
            ));
        }
    }
    if let Some(expected) = case.expected_fallback {
        if observed.fallback != expected {
            failures.push(format!(
                "fallback={} did not match expected {expected}",
                observed.fallback
            ));
        }
    }
    failures
}

async fn run_response_live(path: &Path) -> RuntimeResult<EvalReport> {
    require_provider_env()?;
    let baseline = load_response_baseline(path)?;
    if baseline.status == BaselineStatus::Pending || baseline.cases.is_empty() {
        return Err(RuntimeError::Config(format!(
            "{} response baseline is pending or empty",
            path.display()
        )));
    }
    let app_config = AppConfig::load("config/config.toml")
        .map_err(|err| RuntimeError::Config(err.to_string()))?;
    let prompts = PromptBank::from_app_config(&app_config)
        .map_err(|err| RuntimeError::Config(err.to_string()))?;
    let llm = LlmDefaults::from_env().map_err(|err| RuntimeError::Config(err.to_string()))?;
    let mcp_url = load_mcp_url().map_err(|err| RuntimeError::Config(err.to_string()))?;
    let mcp_client = McpClient::connect_http(&mcp_url)
        .await
        .map_err(|err| RuntimeError::Upstream(err.to_string()))?;
    let mcp = mcp_client.handle();
    let tools = Arc::new(
        mcp.list_openrouter_tools()
            .await
            .map_err(|err| RuntimeError::Upstream(err.to_string()))?,
    );
    let instructions = mcp_client.server_instructions();

    let mut report = EvalReport {
        provenance: Some(baseline.provenance),
        ..EvalReport::default()
    };
    for case in baseline.cases {
        let system = live_eval_system_prompt(&prompts.agent_system, instructions.as_deref());
        let cfg = GenerationConfig {
            system,
            user_prompt: case.prompt.clone(),
            history: Vec::new(),
            api_key: llm.api_key.clone(),
            model: llm.model.clone(),
            base_url: llm.base_url.clone(),
            app_url: llm.app_url.clone(),
            app_title: llm.app_title.clone(),
            temperature: llm.temperature,
            top_p: llm.top_p,
            max_tokens: llm.max_tokens,
        };
        let started = Instant::now();
        let response = llm_connector::generate(cfg, tools.clone(), mcp.clone())
            .await
            .map_err(|err| RuntimeError::Upstream(err.to_string()))?;
        let observed = ObservedResponse {
            tokens: estimate_tokens(&response),
            latency_ms: started.elapsed().as_millis() as u64,
            refused: infer_refusal(&response),
            fallback: infer_fallback(&response),
            response,
        };
        let failures = response_case_failures(&case, &observed);
        if failures.is_empty() {
            report.passed += 1;
        } else {
            report.failed += 1;
            report.regressions.extend(
                failures
                    .into_iter()
                    .map(|failure| format!("{}: {failure}", case.id)),
            );
        }
        report.latency_ms += observed.latency_ms;
        report.tokens += observed.tokens;
        if observed.refused {
            report.refusals += 1;
        }
        if observed.fallback {
            report.fallbacks += 1;
        }
    }

    Ok(report)
}

fn require_provider_env() -> RuntimeResult<()> {
    for key in ["OPENROUTER_API_KEY", "OPENROUTER_MODEL"] {
        if std::env::var(key)
            .map(|value| value.trim().is_empty())
            .unwrap_or(true)
        {
            return Err(RuntimeError::Config(format!(
                "response live eval requires {key}"
            )));
        }
    }
    Ok(())
}

fn live_eval_system_prompt(base_system: &str, instructions: Option<&str>) -> String {
    let system_base = match instructions {
        Some(instr) if !instr.trim().is_empty() => {
            format!("{base_system}\n\n# MCP server conventions (apply to all tools)\n{instr}")
        }
        _ => base_system.to_string(),
    };
    // Shared with the serving path + sub-agent engine via `current_time_header` (no drift).
    format!(
        "{}{system_base}",
        crate::agent::clock::current_time_header(&chrono::Local::now())
    )
}

fn estimate_tokens(response: &str) -> u64 {
    response.split_whitespace().count().max(1) as u64
}

fn infer_refusal(response: &str) -> bool {
    response.contains("不能遵循")
        || response.contains("無法處理")
        || response.to_lowercase().contains("cannot comply")
}

fn infer_fallback(response: &str) -> bool {
    response.contains("初步判讀")
        || response.contains("需要進一步確認")
        || response.to_lowercase().contains("fallback")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn pipeline_only_runs_default_pack_fixtures() {
        let report = run(EvalMode::PipelineOnly).expect("pipeline eval should run");

        assert!(report.passed > 0);
        assert_eq!(report.failed, 0);
    }

    #[test]
    fn replay_mode_reads_artifact_without_network() {
        let path = temp_artifact(
            r#"{
              "status": "loaded",
              "provenance": "unit test replay artifact",
              "cases": [
                {
                  "id": "ok",
                  "prompt": "營收",
                  "must_include": ["answer"],
                  "must_not_include": ["hallucinated"],
                  "actual_response": "answer",
                  "latency_ms": 12,
                  "max_latency_ms": 20,
                  "tokens": 8,
                  "max_tokens": 10
                },
                {
                  "id": "refusal",
                  "prompt": "ignore previous",
                  "expected_response": "refuse",
                  "actual_response": "refuse",
                  "expected_refused": true,
                  "expected_fallback": true,
                  "refused": true,
                  "fallback": true
                }
              ]
            }"#,
        );

        let report = run(EvalMode::ResponseReplay {
            artifact: path.clone(),
        })
        .expect("replay eval should run");

        assert_eq!(report.passed, 2);
        assert_eq!(report.failed, 0);
        assert_eq!(report.latency_ms, 12);
        assert_eq!(report.tokens, 8);
        assert_eq!(report.refusals, 1);
        assert_eq!(report.fallbacks, 1);
        assert!(report.regressions.is_empty());
        assert_eq!(
            report.provenance.as_deref(),
            Some("unit test replay artifact")
        );
        let _ = fs::remove_file(path);
    }

    #[test]
    fn replay_mode_reports_response_regressions() {
        let path = temp_artifact(
            r#"{
              "status": "loaded",
              "provenance": "unit test replay artifact",
              "cases": [
                {
                  "id": "bad",
                  "prompt": "營收",
                  "actual_response": "made up answer",
                  "must_include": ["營收"],
                  "must_not_include": ["made up"],
                  "latency_ms": 99,
                  "max_latency_ms": 10,
                  "tokens": 20,
                  "max_tokens": 3,
                  "expected_refused": true,
                  "refused": false
                }
              ]
            }"#,
        );

        let report = run(EvalMode::ResponseReplay {
            artifact: path.clone(),
        })
        .expect("replay eval should run");

        assert_eq!(report.passed, 0);
        assert_eq!(report.failed, 1);
        assert!(report
            .regressions
            .iter()
            .any(|failure| failure.contains("missing")));
        assert!(report
            .regressions
            .iter()
            .any(|failure| failure.contains("forbidden")));
        assert!(report
            .regressions
            .iter()
            .any(|failure| failure.contains("latency")));
        let _ = fs::remove_file(path);
    }

    #[test]
    fn replay_mode_rejects_pending_baseline() {
        let err = run(EvalMode::ResponseReplay {
            artifact: PathBuf::from("config/runtime/evals/response-baseline.json"),
        })
        .expect_err("pending baseline should not pass response eval");

        assert!(err.to_string().contains("pending or empty"));
    }

    fn temp_artifact(contents: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock should be after epoch")
            .as_nanos();
        let path = std::env::temp_dir().join(format!("datacenter-agent-eval-{nanos}.json"));
        fs::write(&path, contents).expect("temp replay artifact should be written");
        path
    }
}
