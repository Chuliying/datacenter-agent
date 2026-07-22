//! Reproduction harness for the intermittent `missing artifact: report.data` failure.
//!
//! The `/report` pipeline is `fetcher → analyst → composer → renderer`. The composer's whole job
//! is to call the `emit_report` sink tool once, which deposits the schema-validated
//! [`ReportData`](datacenter_agent::agent::report::ReportData) at the `report.data` artifact key;
//! the pure-logic renderer then injects it into the HTML template. When the (flaky, small) LLM
//! returns a plain text message *without* ever successfully calling `emit_report`, the composer
//! still finishes `Ok` — it has `capture_message: false` and never checks that its required output
//! artifact exists — so the renderer fails with `missing artifact: report.data`.
//!
//! This test isolates that exact handoff. It skips the fetcher/analyst (which need the live MCP +
//! VPN) and feeds the composer a realistic `Intermediate` payload of canned material, then runs
//! `composer → renderer` in a loop against the **real** OpenRouter model from `.env` until the
//! failure reproduces (or the loop budget is exhausted).
//!
//! It is `#[ignore]`d: it needs an `OPENROUTER_API_KEY` and spends real tokens.
//!
//! ```sh
//! RUST_LOG='agent::probe=debug' \
//!   cargo test --test repro_report_data -- --ignored --nocapture
//! ```
//!
//! Tune with env: `REPRO_MAX_ITERS` (default 15), `REPRO_MODEL` (default: `OPENROUTER_MODEL`).

use std::collections::HashMap;
use std::sync::Arc;

use chrono::{FixedOffset, TimeZone};

use datacenter_agent::agent::config::{
    OutputShape, Provider, ReasoningEffort, ResolvedLlm, SubAgentConfig,
};
use datacenter_agent::agent::engine::{ConfiguredAgent, SubAgent};
use datacenter_agent::agent::llm::OpenAiLlm;
use datacenter_agent::agent::payload::{
    AgentError, AgentPayload, ArtifactKey, ArtifactValue, IntermediateData,
};
use datacenter_agent::agent::pipeline::{report_composer_config, Renderer};
use datacenter_agent::agent::tools::emit_report_tool;

fn require_env(key: &str) -> String {
    std::env::var(key)
        .unwrap_or_else(|_| panic!("repro test needs `{key}` set (see .env); run with --ignored"))
}

fn env_or<T: std::str::FromStr>(key: &str, default: T) -> T {
    std::env::var(key)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

/// Canned upstream material: three months of revenue (fetcher.records) plus an analyst narrative
/// (analyst.message). Mirrors the shape the composer sees in production — enough for it to fill a
/// full `ReportData` without inventing numbers.
fn canned_material() -> HashMap<ArtifactKey, ArtifactValue> {
    let records = serde_json::json!({
        "months": [
            { "month": "2026-05", "revenue": 1_820_000, "kwh": 240_000, "sessions": 31_500,
              "newMembers": 420, "totalMembers": 18_900, "activeMembers": 9_200,
              "stations": 46, "chargers": 512 },
            { "month": "2026-06", "revenue": 2_050_000, "kwh": 268_000, "sessions": 34_800,
              "newMembers": 510, "totalMembers": 19_410, "activeMembers": 9_760,
              "stations": 47, "chargers": 528 },
            { "month": "2026-07", "revenue": 690_000, "kwh": 92_000, "sessions": 12_100,
              "newMembers": 180, "totalMembers": 19_590, "activeMembers": 5_100,
              "stations": 47, "chargers": 531, "partial": true }
        ],
        "stationRanking": [
            { "name": "台北南港站", "revenue": 512_000, "kwh": 61_000, "utilization": 78.5, "revenuePerKw": 8.4 },
            { "name": "台中烏日站", "revenue": 431_000, "kwh": 54_500, "utilization": 71.2, "revenuePerKw": 7.9 },
            { "name": "高雄左營站", "revenue": 388_000, "kwh": 49_800, "utilization": 66.9, "revenuePerKw": 7.8 }
        ]
    });

    let narrative = "近三個月營收呈成長態勢：5 月營收 1,820,000 元，6 月成長至 2,050,000 元，\
        月增約 12.6%。7 月為進行中的部分月份，累計 690,000 元，尚未完整，不應視為衰退。\
        會員數穩定成長，總會員自 18,900 增至 19,590。站點營收以台北南港站居首。";

    let mut m = HashMap::new();
    m.insert(ArtifactKey::fetcher_records(), ArtifactValue::Json(records));
    m.insert(
        ArtifactKey::message("analyst"),
        ArtifactValue::Text(narrative.to_string()),
    );
    m
}

#[tokio::test]
#[ignore = "spends real OpenRouter tokens; run with --ignored"]
async fn reproduce_missing_report_data() {
    let _ = dotenvy::dotenv();
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "agent::probe=debug".into()),
        )
        .with_test_writer()
        .try_init();

    let api_key = require_env("OPENROUTER_API_KEY");
    let base_url = std::env::var("OPENROUTER_BASE_URL")
        .unwrap_or_else(|_| "https://openrouter.ai/api/v1".into());
    let model = std::env::var("REPRO_MODEL").unwrap_or_else(|_| require_env("OPENROUTER_MODEL"));
    let max_iters: usize = env_or("REPRO_MAX_ITERS", 15);

    // Rebuild the composer's LLM exactly as `build_report_pipeline` does: the shared resolution,
    // lowered to Minimal reasoning for this mechanical stage.
    let resolved = ResolvedLlm {
        provider: Provider::OpenRouter,
        base_url,
        model: model.clone(),
        temperature: env_or("OPENROUTER_TEMPERATURE", 0.2_f32),
        top_p: 0.1,
        max_tokens: env_or("OPENROUTER_MAX_TOKENS", 8192_u32),
        api_key: Some(api_key),
        reasoning_effort: Some(ReasoningEffort::Minimal),
        app_url: std::env::var("OPENROUTER_APP_URL").ok(),
        app_title: std::env::var("OPENROUTER_APP_TITLE").ok(),
    };
    let llm: Arc<_> = Arc::new(OpenAiLlm::from_resolved(&resolved).expect("build composer LLM"));

    // The composer: real config + real `emit_report` sink, exactly as wired in production.
    let composer_cfg: SubAgentConfig = report_composer_config();
    let composer = ConfiguredAgent::new(
        &composer_cfg,
        llm,
        vec![Box::new(emit_report_tool())],
        OutputShape::Intermediate,
    );

    // The renderer: pure logic over the boot-loaded HTML template.
    let template =
        std::fs::read_to_string("config/report_template/report.html").expect("read template");
    let renderer = Renderer::with_template(Arc::new(template));

    let now = FixedOffset::east_opt(8 * 3600)
        .unwrap()
        .with_ymd_and_hms(2026, 7, 21, 10, 30, 0)
        .unwrap();
    let prompt = "我們最近三個月的營收狀況如何？幫我做一份完整的報告";

    let mut successes = 0usize;
    let mut failures = 0usize;

    for i in 1..=max_iters {
        let payload = AgentPayload::Intermediate(IntermediateData {
            prompt: prompt.to_string(),
            artifacts: canned_material(),
            now,
        });

        // Stage 1: composer. A composer error here (e.g. exceeded MAX_STEPS) is a *different*
        // failure than the one under investigation — record and continue.
        let composed = match composer.run(payload).await {
            Ok(p) => p,
            Err(e) => {
                eprintln!("[iter {i:>2}] composer ERRORED: {e}");
                failures += 1;
                continue;
            }
        };

        // Stage 2: renderer. This is where `missing artifact: report.data` surfaces.
        match renderer.run(composed).await {
            Ok(_) => {
                eprintln!("[iter {i:>2}] OK — report.data produced, renderer succeeded");
                successes += 1;
            }
            Err(AgentError::MissingArtifact(k)) => {
                eprintln!(
                    "[iter {i:>2}] REPRODUCED — renderer failed: missing artifact: {k} \
                     (composer finished Ok but never produced report.data)"
                );
                failures += 1;
            }
            Err(e) => {
                eprintln!("[iter {i:>2}] renderer failed with other error: {e}");
                failures += 1;
            }
        }
    }

    eprintln!(
        "\n=== repro summary (model={model}): {successes} ok / {failures} failed of {max_iters} ==="
    );
    assert!(
        successes + failures == max_iters,
        "every iteration should be accounted for"
    );
}
