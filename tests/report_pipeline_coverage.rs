// Copyright 2026 Wayne Hong (h-alice) <contact@halice.art>
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! Live coverage harness for the `/report` pipeline: **does every chart in the delivered report
//! actually have data?**
//!
//! The report template renders four trend charts — revenue, charging, membership, infrastructure —
//! plus a monthly table and a station ranking, and each is fed by a *different* MCP tool. When the
//! fetcher skips a tool, the composer has nothing to transcribe for that topic, the field lands as
//! `0`, and the chart renders flat: a report that looks complete but is not. Nothing downstream can
//! detect that, which is exactly why it needs a live check.
//!
//! So this test runs the **real** `build_report_pipeline` (real MCP tools, real model, real
//! `emit_report` sink, real renderer) several times over and asserts, per run:
//!
//! 1. the fetcher swept **every granted tool** — one `fetcher.{tool}` artifact each;
//! 2. `report.data` exists and parses as [`ReportData`];
//! 3. every chart series carries a non-zero value somewhere — the *observable* symptom;
//! 4. the station ranking is non-empty.
//!
//! It is **`#[ignore]`d**: it needs a running MCP server, an OpenRouter key, and it spends real
//! tokens on several full reports.
//!
//! ```sh
//! cargo test --test report_pipeline_coverage -- --ignored --nocapture
//! ```
//!
//! Required environment (a `.env` at the repo root is loaded automatically, like `main`):
//!
//! - `DATACENTER_MCP_URL` — the datacenter MCP `/mcp` endpoint
//! - `OPENROUTER_API_KEY` — the LLM key
//! - `OPENROUTER_MODEL` — the model id
//!
//! Optional:
//!
//! - `REPORT_RUNS` — how many full reports to generate (default 3)
//! - `REPORT_PROMPT` — the user request (default asks for a six-month report)

use std::collections::BTreeSet;
use std::sync::Arc;

use datacenter_agent::agent::clock::{Clock, SystemClock};
use datacenter_agent::agent::config::{Provider, ResolvedLlm};
use datacenter_agent::agent::payload::{AgentPayload, ArtifactKey, ArtifactValue, InitialPrompt};
use datacenter_agent::agent::pipeline::report_pipeline_id;
use datacenter_agent::agent::report::ReportData;
use datacenter_agent::agent::wiring::build_report_pipeline;
use datacenter_agent::appstate::PromptBank;
use datacenter_agent::config::AppConfig;
use datacenter_agent::mcp_client::McpClient;

fn require_env(key: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| {
        panic!(
            "integration test needs `{key}` set (see this file's module docs); run with --ignored"
        )
    })
}

fn env_or<T: std::str::FromStr>(key: &str, default: T) -> T {
    std::env::var(key)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

/// One chart in the template, and the [`ReportData`] series that fills it.
///
/// `all_zero` is the check that matters: a chart whose every point is `0` is the exact thing a
/// reader sees as "missing", whether the tool was skipped, the topic was absent from the Material,
/// or the composer zero-filled it.
struct ChartCoverage {
    /// The chart as the reader sees it, for the failure message.
    chart: &'static str,
    /// Whether every value in the series is zero (or the series is empty).
    all_zero: bool,
}

fn chart_coverage(data: &ReportData) -> Vec<ChartCoverage> {
    let p = &data.periods;
    vec![
        ChartCoverage {
            chart: "每月營收 (revenue)",
            all_zero: p.iter().all(|m| m.revenue == 0.0),
        },
        ChartCoverage {
            chart: "每月充電度數 (kwh)",
            all_zero: p.iter().all(|m| m.kwh == 0.0),
        },
        ChartCoverage {
            chart: "每月充電次數 (sessions)",
            all_zero: p.iter().all(|m| m.sessions == 0),
        },
        ChartCoverage {
            chart: "每月會員成長 (totalMembers)",
            all_zero: p.iter().all(|m| m.total_members == 0),
        },
        ChartCoverage {
            chart: "每月活躍會員 (activeMembers)",
            all_zero: p.iter().all(|m| m.active_members == 0),
        },
        ChartCoverage {
            chart: "站點擴張 (stations)",
            all_zero: p.iter().all(|m| m.stations == 0),
        },
        ChartCoverage {
            chart: "充電樁擴張 (chargers)",
            all_zero: p.iter().all(|m| m.chargers == 0),
        },
    ]
}

#[tokio::test]
#[ignore = "touches the live datacenter MCP server + OpenRouter and spends real tokens; run with --ignored"]
async fn every_report_chart_gets_data_across_repeated_runs() {
    let _ = dotenvy::dotenv();

    let mcp_url = require_env("DATACENTER_MCP_URL");
    let api_key = require_env("OPENROUTER_API_KEY");
    let model = require_env("OPENROUTER_MODEL");
    let base_url = std::env::var("OPENROUTER_BASE_URL")
        .unwrap_or_else(|_| "https://openrouter.ai/api/v1".into());
    let runs: usize = env_or("REPORT_RUNS", 3);
    let prompt = std::env::var("REPORT_PROMPT")
        .unwrap_or_else(|_| "請產生最近六個月的充電網路營運報表。".into());

    // ── boot exactly as the server does: config, prompt bank, report template, report grant ──
    let app_config = AppConfig::load("config/config.toml").expect("load config/config.toml");
    let prompts = PromptBank::from_app_config(&app_config).expect("resolve prompt bank");
    let template = Arc::new(app_config.report_template.clone());
    let grant = app_config
        .report_grants
        .clone()
        .expect("config.toml must define [report.grants]")
        .fetcher;
    assert!(
        grant.len() > 1,
        "this harness is about multi-tool coverage; the grant has {} tool(s)",
        grant.len()
    );

    // ── connect to the datacenter and discover the advertised tools ──
    let client = McpClient::connect_http(&mcp_url)
        .await
        .unwrap_or_else(|e| panic!("connect MCP at {mcp_url}: {e:#}"));
    let handle = client.handle();
    let discovered = handle
        .list_openrouter_tools()
        .await
        .expect("list MCP tools");

    let resolved = ResolvedLlm {
        provider: Provider::OpenRouter,
        base_url,
        model: model.clone(),
        temperature: 0.2,
        top_p: 0.1,
        max_tokens: env_or("OPENROUTER_MAX_TOKENS", 8192),
        api_key: Some(api_key),
        reasoning_effort: None,
        app_url: None,
        app_title: None,
    };

    let expected_tools: BTreeSet<String> = grant.iter().cloned().collect();
    let mut failures: Vec<String> = Vec::new();

    for run in 1..=runs {
        println!("\n──────── run {run}/{runs} — model {model} ────────");

        // Fully buffered (`sink: None`): this harness asserts on artifacts, not on the SSE frames,
        // which `streaming_datacenter` already covers.
        let orchestrator = build_report_pipeline(
            handle.clone(),
            &discovered,
            None,
            &prompts.fetcher_system,
            &prompts.report_analyst_system,
            &prompts.report_composer_system,
            &grant,
            &resolved,
            template.clone(),
            None,
        )
        .expect("build the report pipeline");

        let input = AgentPayload::Initial(InitialPrompt {
            prompt: prompt.clone(),
            history: vec![],
            now: SystemClock::default().now(), // stamp the turn once at the boundary
        });

        let outcome = orchestrator.run(&report_pipeline_id(), input).await;
        let final_result = match outcome {
            Ok(AgentPayload::Final(f)) => f,
            Ok(other) => {
                failures.push(format!("run {run}: pipeline ended on {:?}", other.kind()));
                continue;
            }
            Err(e) => {
                failures.push(format!("run {run}: pipeline failed: {e}"));
                continue;
            }
        };

        // ── 1. the fetcher must have swept the whole grant ──
        let fetched: BTreeSet<String> = final_result
            .artifacts
            .keys()
            .filter(|k| k.to_string().starts_with("fetcher."))
            .map(|k| k.to_string().trim_start_matches("fetcher.").to_string())
            .collect();
        let skipped: Vec<&String> = expected_tools.difference(&fetched).collect();
        println!("  fetched {}/{}", fetched.len(), expected_tools.len());
        if !skipped.is_empty() {
            failures.push(format!(
                "run {run}: the fetcher skipped {skipped:?} (fetched {fetched:?})"
            ));
        }

        // ── 2. report.data must exist and parse ──
        let Some(ArtifactValue::Json(raw)) =
            final_result.artifacts.get(&ArtifactKey::report_data())
        else {
            failures.push(format!("run {run}: no `report.data` artifact was produced"));
            continue;
        };
        let data: ReportData = match serde_json::from_value(raw.clone()) {
            Ok(data) => data,
            Err(e) => {
                failures.push(format!("run {run}: `report.data` does not parse: {e}"));
                continue;
            }
        };
        println!(
            "  periods={} stations={} insight_paragraphs={}",
            data.periods.len(),
            data.station_ranking.len(),
            data.insight.paragraphs.len()
        );

        // ── 3. every chart series must carry data ──
        for coverage in chart_coverage(&data) {
            let mark = if coverage.all_zero { "EMPTY" } else { "ok" };
            println!("    [{mark:>5}] {}", coverage.chart);
            if coverage.all_zero {
                failures.push(format!(
                    "run {run}: chart `{}` has no data — every value is zero",
                    coverage.chart
                ));
            }
        }

        // ── 4. and the station ranking must not be empty ──
        if data.station_ranking.is_empty() {
            failures.push(format!("run {run}: the station ranking is empty"));
        }

        // The rendered answer is the `falcon-report` fence the frontend consumes.
        assert!(
            final_result.assistant.starts_with("```falcon-report"),
            "run {run}: the answer must be a falcon-report block"
        );
        assert!(
            !final_result.assistant.contains("__REPORT_DATA_JSON__"),
            "run {run}: the template placeholder must have been replaced"
        );
    }

    assert!(
        failures.is_empty(),
        "{} of {runs} run(s) had gaps:\n  - {}",
        failures.len(),
        failures.join("\n  - ")
    );
}
