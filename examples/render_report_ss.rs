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

//! Run the **星星電力** `/ss-report` pipeline once (or several times) and write each report to an
//! **HTML file you can open in a browser**.
//!
//! `render_report_st`'s counterpart, and the same idea: the real pipeline (real MCP tools, real
//! model, real `emit_ss_report` sink, real renderer), keeping the output instead of asserting on
//! it, so a missing section can be *seen* rather than inferred.
//!
//! It is an `example`, not a `bin`, on purpose: `cargo build --release` never builds it, so this
//! developer tool cannot end up in the shipped image.
//!
//! ```sh
//! # one report, written to ./report-ss-1.html
//! cargo run --example render_report_ss
//!
//! # three reports over a custom window, written into /tmp/reports/
//! cargo run --example render_report_ss -- --runs 3 --out-dir /tmp/reports \
//!     --prompt "請產生 2026 年第 2 季的完整營運報告"
//!
//! # a narrow ask, to check the sweep does NOT fire
//! cargo run --example render_report_ss -- --prompt "只要日照資料"
//! ```
//!
//! Each run writes `report-ss-N.html` (the standalone document, fence stripped) and
//! `report-ss-N.json` (the composer's `ss_report.data` — the exact payload the template reads),
//! then prints, per section, whether it has rows and would therefore render at all.
//!
//! **A missing section is not automatically a bug here.** Unlike the starcharger report, the SS
//! payload expresses "this tool returned nothing" as an empty section that the template hides. So
//! the coverage lines distinguish two things: whether the fetcher *called* each tool, and whether
//! the composer put any rows in the matching section. A tool called but a section left empty is
//! the interesting failure.
//!
//! Required environment (a `.env` at the repo root is loaded automatically, like `main`):
//! `DATACENTER_MCP_URL`, `OPENROUTER_API_KEY`, `OPENROUTER_MODEL`.

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{anyhow, Context, Result};
use clap::Parser;

use datacenter_agent::agent::clock::{Clock, SystemClock};
use datacenter_agent::agent::payload::{AgentPayload, ArtifactKey, ArtifactValue, InitialPrompt};
use datacenter_agent::agent::pipeline::ss_report_pipeline_id;
use datacenter_agent::agent::ss_report::SsReportData;
use datacenter_agent::agent::wiring::build_ss_report_pipeline;
use datacenter_agent::appstate::{LlmDefaults, PromptBank};
use datacenter_agent::config::AppConfig;
use datacenter_agent::mcp_client::McpClient;

/// The report keywords the `/ss-report` handler sweeps on.
///
/// Duplicated from `handler::SS_REPORT_SWEEP_KEYWORDS` (which is private) so this tool reports the
/// same decision the endpoint would make. Kept deliberately small — if the two ever disagree, the
/// printed line says which one this run used.
const SWEEP_KEYWORDS: &[&str] = &["report", "報告", "報表", "完整", "整體", "各條業務線"];

#[derive(Debug, Parser)]
#[command(about = "Generate a real 星星電力 report and write it to an HTML file")]
struct Args {
    /// The user request to run the report pipeline on.
    #[arg(long, default_value = "請產生本季星星電力投資平台的完整營運報告。")]
    prompt: String,
    /// How many reports to generate (each is an independent pipeline run).
    #[arg(long, default_value_t = 1)]
    runs: usize,
    /// Directory the `report-ss-N.html` / `report-ss-N.json` files are written to.
    #[arg(long, default_value = ".")]
    out_dir: PathBuf,
    /// Path to the runtime config.
    #[arg(long, default_value = "config/config.toml")]
    config: PathBuf,
}

/// One section of the report, and whether it would actually render.
struct SectionCoverage {
    /// The section as the reader sees it.
    label: &'static str,
    /// The `ss_*` tool that feeds it.
    tool: &'static str,
    /// How many rows the composer put in it. `0` ⇒ the template hides the section.
    rows: usize,
}

fn section_coverage(data: &SsReportData) -> Vec<SectionCoverage> {
    vec![
        SectionCoverage {
            label: "日照資源 (solar)",
            tool: "ss_sunshine_hours",
            rows: data.solar.stations.len(),
        },
        SectionCoverage {
            label: "儲能投標 (storage.bids)",
            tool: "ss_energy_storage",
            rows: data.storage.bids.len(),
        },
        SectionCoverage {
            label: "儲能案場收益 (storage.cases)",
            tool: "ss_energy_storage",
            rows: data.storage.cases.len(),
        },
        SectionCoverage {
            label: "綠電轉供 (wheeling)",
            tool: "ss_power_wheeling",
            rows: data.wheeling.sites.len(),
        },
        SectionCoverage {
            label: "售電漏斗 (pipeline.sell)",
            tool: "ss_power_pipeline",
            rows: data.pipeline.sell.len(),
        },
        SectionCoverage {
            label: "購電漏斗 (pipeline.purchase)",
            tool: "ss_power_pipeline",
            rows: data.pipeline.purchase.len(),
        },
        SectionCoverage {
            label: "BTM 潛在案件 (btm.potential)",
            tool: "ss_btm_projects",
            rows: data.btm.potential.len(),
        },
        SectionCoverage {
            label: "BTM 星展50 (btm.starSpark)",
            tool: "ss_btm_projects",
            rows: data.btm.star_spark.len(),
        },
        SectionCoverage {
            label: "FTM 建置案 (ftm.projects)",
            tool: "ss_ftm_projects",
            rows: data.ftm.projects.len(),
        },
        SectionCoverage {
            label: "FTM 工項 (ftm.stages)",
            tool: "ss_ftm_projects",
            rows: data.ftm.stages.len(),
        },
        SectionCoverage {
            label: "FTM 待收請款 (ftm.receivables)",
            tool: "ss_ftm_projects",
            rows: data.ftm.receivables.len(),
        },
    ]
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

    // ── boot exactly as the server does: config, prompts, SS template, SS grant, LLM ──
    let app_config =
        AppConfig::load(&args.config).with_context(|| format!("load {}", args.config.display()))?;
    let prompts = PromptBank::from_app_config(&app_config).context("resolve the prompt bank")?;
    let template = Arc::new(app_config.ss_report_template.clone());
    let grant = app_config.ss_report_grants.fetcher.clone();
    if grant.is_empty() {
        return Err(anyhow!("[ss_report.grants].fetcher is empty"));
    }
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

    // The endpoint decides the fetcher's sweep from the prompt's wording; mirror that here so the
    // run is comparable to what `/ss-report` would do with the same prompt.
    let lowered = args.prompt.to_lowercase();
    let sweep = SWEEP_KEYWORDS.iter().any(|k| lowered.contains(k));

    println!(
        "model={} tools={} runs={} sweep_all_tools={sweep}",
        resolved.model,
        grant.len(),
        args.runs
    );
    if !sweep {
        println!(
            "  note: the prompt trips no report keyword, so the fetcher will fetch only what it \
             needs — expect some sections to be legitimately absent."
        );
    }

    for run in 1..=args.runs {
        println!("\n──────── run {run}/{} ────────", args.runs);

        // `sink: None` — fully buffered. Streaming frames add nothing when the artifact is a file.
        let orchestrator = build_ss_report_pipeline(
            handle.clone(),
            &discovered,
            None,
            &prompts.ss_fetcher_system,
            &prompts.ss_report_analyst_system,
            &prompts.ss_report_composer_system,
            &grant,
            sweep,
            &resolved,
            template.clone(),
            None,
        )
        .context("build the ss report pipeline")?;

        let input = AgentPayload::Initial(InitialPrompt {
            prompt: args.prompt.clone(),
            history: vec![],
            now: SystemClock::default().now(), // stamp the turn once at the boundary
        });

        let final_result = match orchestrator.run(&ss_report_pipeline_id(), input).await {
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

        // Which tools the fetcher actually reached. On a sweep turn every one of them should
        // appear; on a narrow turn a subset is correct.
        let mut fetched: Vec<String> = final_result
            .artifacts
            .keys()
            .map(ToString::to_string)
            .filter_map(|k| k.strip_prefix("fetcher.").map(str::to_string))
            .collect();
        fetched.sort();
        println!("  fetched {}/{}: {fetched:?}", fetched.len(), grant.len());
        if sweep {
            for tool in &grant {
                if !fetched.contains(tool) {
                    println!("  !! SKIPPED `{tool}` on a report turn — its section will be absent");
                }
            }
        }

        // The composer's payload is what the template reads, so write it beside the HTML.
        let json_path = args.out_dir.join(format!("report-ss-{run}.json"));
        if let Some(ArtifactValue::Json(raw)) =
            final_result.artifacts.get(&ArtifactKey::ss_report_data())
        {
            std::fs::write(&json_path, serde_json::to_string_pretty(raw)?)
                .with_context(|| format!("write {}", json_path.display()))?;
            match serde_json::from_value::<SsReportData>(raw.clone()) {
                Ok(data) => {
                    println!(
                        "  period={:?} snapshot={:?} insight_paragraphs={}",
                        data.report.period_label,
                        data.report.snapshot_label,
                        data.insight.paragraphs.len()
                    );
                    for section in section_coverage(&data) {
                        let called = fetched.iter().any(|t| t == section.tool);
                        // The case worth looking at: the tool was called, yet the section is empty.
                        let mark = match (called, section.rows) {
                            (_, rows) if rows > 0 => format!("{rows:>4} rows"),
                            (true, _) => "  EMPTY!".to_string(),
                            (false, _) => " absent ".to_string(),
                        };
                        println!("    [{mark}] {}  ({})", section.label, section.tool);
                    }
                    if !data.report.data_note.is_empty() {
                        println!("  dataNote: {}", data.report.data_note);
                    }
                }
                Err(e) => eprintln!("  !! ss_report.data does not parse as SsReportData: {e}"),
            }
        } else {
            eprintln!("  !! no `ss_report.data` artifact — the composer never emitted one");
        }

        // The pipeline's answer is a ```falcon-report fenced block (the frontend wire shape);
        // strip the fence so the file is a standalone document a browser can open.
        let html_path = args.out_dir.join(format!("report-ss-{run}.html"));
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
