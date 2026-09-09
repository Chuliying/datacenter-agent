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

//! Run the `/report` pipeline once (or several times) and write each report to an **HTML file you
//! can open in a browser**.
//!
//! This is the by-hand counterpart to `tests/report_pipeline_coverage.rs`: the same real pipeline
//! (real MCP tools, real model, real `emit_report` sink, real renderer), but it *keeps the output*
//! instead of asserting on it, so an empty chart can be seen rather than inferred.
//!
//! It is an `example`, not a `bin`, on purpose: `cargo build --release` never builds it, so this
//! developer tool cannot end up in the shipped image.
//!
//! ```sh
//! # one report, written to ./report.html
//! cargo run --example render_report
//!
//! # three reports over a custom window, written into /tmp/reports/
//! cargo run --example render_report -- --runs 3 --out-dir /tmp/reports \
//!     --prompt "請產生 2026 年上半年的營運報表"
//! ```
//!
//! Each run writes `report-N.html` (the standalone document, fence stripped) and
//! `report-N.json` (the composer's `report.data` — the exact payload the template reads), then
//! prints a coverage line per chart so a gap is visible without opening the file.
//!
//! Required environment (a `.env` at the repo root is loaded automatically, like `main`):
//! `DATACENTER_MCP_URL`, `OPENROUTER_API_KEY`, `OPENROUTER_MODEL`.

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{anyhow, Context, Result};
use clap::Parser;

use datacenter_agent::agent::clock::{Clock, SystemClock};
use datacenter_agent::agent::payload::{AgentPayload, ArtifactKey, ArtifactValue, InitialPrompt};
use datacenter_agent::agent::pipeline::report_pipeline_id;
use datacenter_agent::agent::report::ReportData;
use datacenter_agent::agent::wiring::build_report_pipeline;
use datacenter_agent::appstate::{LlmDefaults, PromptBank};
use datacenter_agent::config::AppConfig;
use datacenter_agent::mcp_client::McpClient;

#[derive(Debug, Parser)]
#[command(about = "Generate a real EOMC report and write it to an HTML file")]
struct Args {
    /// The user request to run the report pipeline on.
    #[arg(long, default_value = "請產生最近六個月的充電網路營運報表。")]
    prompt: String,
    /// How many reports to generate (each is an independent pipeline run).
    #[arg(long, default_value_t = 1)]
    runs: usize,
    /// Directory the `report-N.html` / `report-N.json` files are written to.
    #[arg(long, default_value = ".")]
    out_dir: PathBuf,
    /// Path to the runtime config.
    #[arg(long, default_value = "config/config.toml")]
    config: PathBuf,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    let _ = dotenvy::dotenv();
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "warn,datacenter_agent=info".into()),
        )
        .init();

    // ── boot exactly as the server does: config, prompts, template, report grant, LLM ──
    let app_config =
        AppConfig::load(&args.config).with_context(|| format!("load {}", args.config.display()))?;
    let prompts = PromptBank::from_app_config(&app_config).context("resolve the prompt bank")?;
    let template = Arc::new(app_config.report_template.clone());
    let grant = app_config
        .report_grants
        .clone()
        .ok_or_else(|| anyhow!("config has no [report.grants] section"))?
        .fetcher;
    let resolved = LlmDefaults::from_env()
        .context("resolve the LLM from the environment")?
        .resolved();

    // ── connect to the datacenter and discover its tools ──
    let mcp_url = std::env::var("DATACENTER_MCP_URL").context("env_error: DATACENTER_MCP_URL")?;
    let client = McpClient::connect_http(&mcp_url)
        .await
        .with_context(|| format!("connect MCP at {mcp_url}"))?;
    let handle = client.handle();
    let discovered = handle
        .list_openrouter_tools()
        .await
        .context("list MCP tools")?;

    std::fs::create_dir_all(&args.out_dir)
        .with_context(|| format!("create {}", args.out_dir.display()))?;

    println!(
        "model={} tools={} runs={}",
        resolved.model,
        grant.len(),
        args.runs
    );

    for run in 1..=args.runs {
        println!("\n──────── run {run}/{} ────────", args.runs);

        // `sink: None` — fully buffered. Streaming frames add nothing when the artifact is a file.
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
        .context("build the report pipeline")?;

        let input = AgentPayload::Initial(InitialPrompt {
            prompt: args.prompt.clone(),
            history: vec![],
            now: SystemClock::default().now(), // stamp the turn once at the boundary
        });

        let final_result = match orchestrator.run(&report_pipeline_id(), input).await {
            Ok(AgentPayload::Final(f)) => f,
            Ok(other) => {
                eprintln!("  run {run} FAILED: pipeline ended on {:?}", other.kind());
                continue;
            }
            Err(e) => {
                eprintln!("  run {run} FAILED: {e}");
                continue;
            }
        };

        // Which tools the fetcher actually reached — the usual cause of an empty section.
        let mut fetched: Vec<String> = final_result
            .artifacts
            .keys()
            .map(ToString::to_string)
            .filter_map(|k| k.strip_prefix("fetcher.").map(str::to_string))
            .collect();
        fetched.sort();
        println!("  fetched {}/{}: {fetched:?}", fetched.len(), grant.len());
        for tool in &grant {
            if !fetched.contains(tool) {
                println!("  !! SKIPPED `{tool}` — its section will be empty");
            }
        }

        // The composer's payload is what the template reads, so write it beside the HTML: it is far
        // easier to see "every `stations` is 0" here than in a rendered chart.
        let json_path = args.out_dir.join(format!("report-{run}.json"));
        if let Some(ArtifactValue::Json(raw)) =
            final_result.artifacts.get(&ArtifactKey::report_data())
        {
            std::fs::write(&json_path, serde_json::to_string_pretty(raw)?)
                .with_context(|| format!("write {}", json_path.display()))?;
            match serde_json::from_value::<ReportData>(raw.clone()) {
                Ok(data) => print_coverage(&data),
                Err(e) => eprintln!("  !! report.data does not parse as ReportData: {e}"),
            }
        } else {
            eprintln!("  !! no `report.data` artifact — the composer never emitted one");
        }

        // The pipeline's answer is a ```falcon-report fenced block (the frontend wire shape);
        // strip the fence so the file is a standalone document a browser can open.
        let html_path = args.out_dir.join(format!("report-{run}.html"));
        std::fs::write(&html_path, strip_report_fence(&final_result.assistant))
            .with_context(|| format!("write {}", html_path.display()))?;

        println!("  wrote {}", html_path.display());
        println!("  wrote {}", json_path.display());
    }

    Ok(())
}

/// Unwraps the ```` ```falcon-report ```` block the renderer emits, yielding the bare HTML.
///
/// Returns the input unchanged when it is not fenced, so a failure mode is still written to disk
/// and inspectable rather than being silently blanked.
fn strip_report_fence(assistant: &str) -> &str {
    assistant
        .strip_prefix("```falcon-report\n")
        .and_then(|rest| rest.strip_suffix("\n```"))
        .unwrap_or(assistant)
}

/// Prints one line per chart series, flagging any that is entirely zero.
///
/// An all-zero series is exactly what a reader perceives as a "missing chart", whether the tool was
/// skipped, the topic was absent from the material, or the composer zero-filled it.
fn print_coverage(data: &ReportData) {
    let p = &data.periods;
    let series: [(&str, bool); 7] = [
        ("每月營收 (revenue)", p.iter().all(|m| m.revenue == 0.0)),
        ("每月充電度數 (kwh)", p.iter().all(|m| m.kwh == 0.0)),
        ("每月充電次數 (sessions)", p.iter().all(|m| m.sessions == 0)),
        (
            "每月會員成長 (totalMembers)",
            p.iter().all(|m| m.total_members == 0),
        ),
        (
            "每月活躍會員 (activeMembers)",
            p.iter().all(|m| m.active_members == 0),
        ),
        ("站點擴張 (stations)", p.iter().all(|m| m.stations == 0)),
        ("充電樁擴張 (chargers)", p.iter().all(|m| m.chargers == 0)),
    ];
    println!(
        "  periods={} stations={} insight_paragraphs={}",
        p.len(),
        data.station_ranking.len(),
        data.insight.paragraphs.len()
    );
    for (chart, all_zero) in series {
        println!("    [{:>5}] {chart}", if all_zero { "EMPTY" } else { "ok" });
    }
    if data.station_ranking.is_empty() {
        println!("    [EMPTY] 站點營收排名 (stationRanking)");
    }
}
