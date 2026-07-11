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

//! Live integration test: the full **`/agent`** pipeline against the real datacenter.
//!
//! Exercises the four-stage composition end to end —
//! [`fetcher`](datacenter_agent::agent::pipeline::fetcher_config) →
//! [`analyst`](datacenter_agent::agent::pipeline::analyst_config) →
//! [`charter`](datacenter_agent::agent::pipeline::charter_config) →
//! [`Finalizer`](datacenter_agent::agent::pipeline::Finalizer) — driven by real LLM calls and a
//! real MCP data tool, all sharing one
//! [`ChannelSink`](datacenter_agent::agent::events::ChannelSink) so the thinking + tool-using
//! process streams live:
//!
//! - the `fetcher` (buffered LLM) calls `bill_revenue` — you see `ToolStarted` / `ToolProduced`;
//! - the `analyst` (streaming LLM) writes its report — you see it stream token-by-token as
//!   `ContentDelta`, auto-captured as the `analyst.message` artifact;
//! - the `charter` (buffered LLM) may call `emit_chart` — you see `ToolStarted` / `ToolProduced`;
//! - the `finalizer` (pure logic) concatenates the report with each chart as a ```` ```falcon-chart ````
//!   block and the run emits the terminal `Finished`.
//!
//! It is **`#[ignore]`d** — it needs a running MCP server and an OpenRouter key. Run it by hand and
//! watch the pipeline work:
//!
//! ```text
//! cargo test --test agent_pipeline -- --ignored --nocapture
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
//! - `OPENROUTER_BASE_URL` — defaults to `https://openrouter.ai/api/v1`
//! - `AGENT_PROMPT` — the user request (default asks for a two-month revenue summary, which should
//!   trigger a chart)

use std::collections::HashMap;
use std::sync::Arc;

use datacenter_agent::agent::config::{OutputShape, PipelineConfig, Provider, ResolvedLlm, SubAgentId};
use datacenter_agent::agent::clock::{Clock, SystemClock};
use datacenter_agent::agent::engine::{resolve_pipeline, ConfiguredAgent, Orchestrator, SubAgent};
use datacenter_agent::agent::events::{AgentEvent, ChannelSink, EventSink};
use datacenter_agent::agent::llm::{OpenAiLlm, StreamingOpenAiLlm};
use datacenter_agent::agent::payload::{
    AgentPayload, ArtifactKey, InitialPrompt, LlmCapability, Tool,
};
use datacenter_agent::agent::pipeline::{
    agent_pipeline_id, analyst_config, charter_config, fetcher_config, Finalizer,
};
use datacenter_agent::agent::tools::{emit_chart_tool, McpTool, StreamingTool, ToolId};
use datacenter_agent::mcp_client::McpClient;
use tokio::sync::mpsc;

/// Read a required env var, failing with a message that names what to set.
fn require_env(key: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| {
        panic!(
            "integration test needs `{key}` set (see this file's module docs); run with --ignored"
        )
    })
}

/// A one-line rendering of a structural event for the live log.
fn summarize(ev: &AgentEvent) -> String {
    match ev {
        AgentEvent::StageStarted { agent, input } => {
            format!("[stage started] {} <- {input:?}", agent.0)
        }
        AgentEvent::StageProduced { agent, keys } => {
            format!("[stage produced] {} -> {keys:?}", agent.0)
        }
        AgentEvent::StageFinished { agent } => format!("[stage finished] {}", agent.0),
        AgentEvent::ToolStarted { name } => format!("[tool started] {name}"),
        AgentEvent::ToolProduced { name, target } => format!("[tool produced] {name} -> {target}"),
        AgentEvent::ToolRejected { name, reason } => format!("[tool rejected] {name}: {reason}"),
        AgentEvent::ToolCallProposed { id, name } => format!("[tool proposed] {name} (#{id})"),
        AgentEvent::ToolArgsDelta { fragment, .. } => format!("[tool args] {fragment}"),
        AgentEvent::ReasoningDelta { text } => format!("[reasoning] {text}"),
        AgentEvent::ContentDelta { text } => format!("[content] {text}"),
        AgentEvent::Finished { assistant } => {
            format!("[finished] {} chars", assistant.chars().count())
        }
        AgentEvent::Error { message } => format!("[error] {message}"),
    }
}

#[tokio::test]
#[ignore = "touches the live datacenter MCP server + OpenRouter; run with --ignored"]
async fn agent_pipeline_fetch_analyse_chart_finalize_against_the_datacenter() {
    let _ = dotenvy::dotenv();

    let mcp_url = require_env("DATACENTER_MCP_URL");
    let api_key = require_env("OPENROUTER_API_KEY");
    let model = require_env("OPENROUTER_MODEL");
    let base_url = std::env::var("OPENROUTER_BASE_URL")
        .unwrap_or_else(|_| "https://openrouter.ai/api/v1".into());
    let prompt = std::env::var("AGENT_PROMPT").unwrap_or_else(|_| {
        "比較最近兩個月的營收，並用一段話總結趨勢。".into()
    });

    // ── connect to the datacenter MCP server and discover the data tool ──
    let client = McpClient::connect_http(&mcp_url)
        .await
        .unwrap_or_else(|e| panic!("connect MCP at {mcp_url}: {e:#}"));
    let handle = client.handle();
    let discovered = handle
        .list_openrouter_tools()
        .await
        .expect("list MCP tools");
    let tool_def = discovered
        .iter()
        .find(|t| t.function.name == "bill_revenue")
        .unwrap_or_else(|| panic!("server does not advertise `bill_revenue`"));
    let parameters = tool_def
        .function
        .parameters
        .clone()
        .unwrap_or_else(|| serde_json::json!({ "type": "object", "properties": {} }));
    let description = tool_def.function.description.clone().unwrap_or_default();

    // ── one shared sink drives the SSE for every stage ──
    let (tx, mut rx) = mpsc::channel::<AgentEvent>(4096);
    let sink: Arc<dyn EventSink> = Arc::new(ChannelSink(tx));

    let resolved = ResolvedLlm {
        provider: Provider::OpenRouter,
        base_url,
        model,
        temperature: 0.2,
        top_p: 0.1,
        max_tokens: 2048,
        api_key: Some(api_key),
    };
    // Buffered LLM for the tool-using stages; streaming LLM for the analyst so its report streams.
    let buffered: Arc<dyn LlmCapability> =
        Arc::new(OpenAiLlm::from_resolved(&resolved).expect("build OpenAiLlm"));
    let streaming: Arc<dyn LlmCapability> = Arc::new(
        StreamingOpenAiLlm::from_resolved(&resolved, sink.clone()).expect("build StreamingOpenAiLlm"),
    );

    // ── build the four stages ──
    let fetch_mcp: Box<dyn Tool> = Box::new(McpTool::new(
        handle,
        ToolId::BillRevenue,
        "bill_revenue",
        description,
        parameters,
        ArtifactKey::fetcher_records(),
    ));
    let fetcher: Arc<dyn SubAgent> = Arc::new(ConfiguredAgent::new(
        &fetcher_config(),
        buffered.clone(),
        StreamingTool::wrap_all(vec![fetch_mcp], sink.clone()),
        OutputShape::Intermediate,
    ));
    let analyst: Arc<dyn SubAgent> = Arc::new(ConfiguredAgent::new(
        &analyst_config(),
        streaming.clone(),
        vec![],
        OutputShape::Intermediate,
    ));
    let charter: Arc<dyn SubAgent> = Arc::new(ConfiguredAgent::new(
        &charter_config(),
        buffered.clone(),
        StreamingTool::wrap_all(vec![Box::new(emit_chart_tool())], sink.clone()),
        OutputShape::Intermediate,
    ));
    let finalizer: Arc<dyn SubAgent> = Arc::new(Finalizer::default_stage());

    let mut agents: HashMap<SubAgentId, Arc<dyn SubAgent>> = HashMap::new();
    agents.insert(SubAgentId("fetcher".into()), fetcher);
    agents.insert(SubAgentId("analyst".into()), analyst);
    agents.insert(SubAgentId("charter".into()), charter);
    agents.insert(SubAgentId("finalizer".into()), finalizer);

    let pipe = PipelineConfig {
        id: agent_pipeline_id(),
        stages: vec![
            SubAgentId("fetcher".into()),
            SubAgentId("analyst".into()),
            SubAgentId("charter".into()),
            SubAgentId("finalizer".into()),
        ],
    };
    let mut orch = Orchestrator::new();
    orch.insert(agent_pipeline_id(), resolve_pipeline(&pipe, &agents).unwrap());

    // ── run the pipeline on a task; drain the sink live in this task ──
    let run = tokio::spawn(async move {
        orch.run_emitting(
            &agent_pipeline_id(),
            AgentPayload::Initial(InitialPrompt {
                prompt,
                history: vec![],
                now: SystemClock::default().now(), // stamp the turn once at the boundary
            }),
            &*sink,
        )
        .await
    });

    // Stop on the terminal event, NOT on channel close: `agents` still holds every stage (and thus
    // the sink clones), so the channel outlives the run — waiting on close would deadlock. A real
    // SSE consumer likewise stops on the terminal frame.
    let mut events = Vec::new();
    while let Some(ev) = rx.recv().await {
        let terminal = matches!(ev, AgentEvent::Finished { .. } | AgentEvent::Error { .. });
        match &ev {
            AgentEvent::ContentDelta { text } => eprint!("{text}"),
            other => eprintln!("\n{}", summarize(other)),
        }
        events.push(ev);
        if terminal {
            break;
        }
    }
    eprintln!();
    let outcome = run.await.expect("join the pipeline task");
    let _ = client.shutdown().await;

    // ── assert the pipeline completed with a real, combined answer ──
    assert!(outcome.is_ok(), "pipeline run failed: {:?}", outcome.err());
    let final_answer = match outcome.unwrap() {
        AgentPayload::Final(f) => f.assistant,
        other => panic!("expected Final, got {:?}", other.kind()),
    };
    assert!(
        !final_answer.trim().is_empty(),
        "the finalizer must produce a non-empty answer"
    );

    // The analyst streamed its report as content deltas.
    let content_deltas = events
        .iter()
        .filter(|e| matches!(e, AgentEvent::ContentDelta { .. }))
        .count();
    assert!(
        content_deltas >= 1,
        "the analyst must stream its report as ContentDelta events"
    );

    println!("{}", final_answer);

    // If the charter decided a chart helped, the finalizer wrapped it in a falcon-chart block whose
    // JSON parses. (We don't force a chart — the model decides — but a present one must be valid.)
    if let Some(block) = final_answer
        .split("```falcon-chart")
        .nth(1)
        .and_then(|s| s.split("```").next())
    {
        let parsed: serde_json::Value =
            serde_json::from_str(block.trim()).expect("falcon-chart block must be valid JSON");
        assert!(
            parsed.get("chartType").is_some(),
            "a falcon-chart block must carry a chartType"
        );
        eprintln!("charted: {}", parsed["title"]);
    } else {
        eprintln!("no chart emitted (charter judged none needed)");
    }

    eprintln!(
        "\nOK — {content_deltas} content deltas across {} events; {} chars of final answer.",
        events.len(),
        final_answer.chars().count()
    );
}
