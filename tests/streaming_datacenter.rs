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

//! Live integration test: **streaming** a terminal sub-agent against the real datacenter.
//!
//! The counterpart to `fetcher_datacenter.rs`, but exercising the Path-A streaming stack end to
//! end: a [`StreamingOpenAiLlm`](datacenter_agent::agent::llm::StreamingOpenAiLlm) and
//! [`StreamingTool`](datacenter_agent::agent::tools::StreamingTool)-wrapped
//! [`McpTool`](datacenter_agent::agent::tools::McpTool), driven by
//! [`Orchestrator::run_emitting`](datacenter_agent::agent::engine::Orchestrator), all sharing one
//! [`ChannelSink`](datacenter_agent::agent::events::ChannelSink). We drain the channel live and
//! assert the model's answer arrived as streamed [`AgentEvent::ContentDelta`] fragments plus a
//! terminal [`AgentEvent::Finished`].
//!
//! It is **`#[ignore]`d** because it needs a running MCP server and an OpenRouter key — run it by
//! hand and watch the tokens stream:
//!
//! ```text
//! cargo test --test streaming_datacenter -- --ignored --nocapture
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
//! - `STREAM_TOOL` — the MCP tool the agent is granted (default `bill_revenue`)
//! - `STREAM_PROMPT` — the user request (default asks for a short revenue summary)

use std::collections::HashMap;
use std::sync::Arc;

use datacenter_agent::agent::config::{
    OutputShape, PipelineConfig, PipelineId, Provider, ResolvedLlm, SubAgentConfig, SubAgentId,
};
use datacenter_agent::agent::clock::{Clock, SystemClock};
use datacenter_agent::agent::engine::{resolve_pipeline, ConfiguredAgent, Orchestrator, SubAgent};
use datacenter_agent::agent::events::{AgentEvent, ChannelSink, EventSink};
use datacenter_agent::agent::llm::StreamingOpenAiLlm;
use datacenter_agent::agent::payload::{
    AgentPayload, ArtifactKey, InitialPrompt, LlmCapability, PayloadKind, Tool,
};
use datacenter_agent::agent::tools::{McpTool, StreamingTool, ToolId};
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

/// A one-line, content-truncating rendering of an event for the live log.
fn summarize(ev: &AgentEvent) -> String {
    match ev {
        AgentEvent::StageStarted { agent, input } => {
            format!("[stage started] {} <- {input:?}", agent.0)
        }
        AgentEvent::StageProduced { agent, keys } => {
            format!("[stage produced] {} -> {keys:?}", agent.0)
        }
        AgentEvent::StageFinished { agent, outcome } => {
            format!("[stage finished] {} ({outcome:?})", agent.0)
        }
        AgentEvent::ToolStarted { name } => format!("[tool started] {name}"),
        AgentEvent::ToolProduced { name, target } => format!("[tool produced] {name} -> {target}"),
        AgentEvent::ToolRejected { name, reason } => format!("[tool rejected] {name}: {reason}"),
        AgentEvent::ToolCallProposed { id, name } => format!("[tool proposed] {name} (#{id})"),
        AgentEvent::ToolArgsDelta { fragment, .. } => format!("[tool args] {fragment}"),
        AgentEvent::Usage {
            prompt,
            completion,
            reasoning,
            total,
        } => format!("[usage] prompt={prompt} completion={completion} reasoning={reasoning:?} total={total}"),
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
async fn terminal_stage_streams_its_answer_from_the_datacenter() {
    let _ = dotenvy::dotenv();

    let mcp_url = require_env("DATACENTER_MCP_URL");
    let api_key = require_env("OPENROUTER_API_KEY");
    let model = require_env("OPENROUTER_MODEL");
    let base_url = std::env::var("OPENROUTER_BASE_URL")
        .unwrap_or_else(|_| "https://openrouter.ai/api/v1".into());
    let stream_tool = std::env::var("STREAM_TOOL").unwrap_or_else(|_| "bill_revenue".into());
    let prompt = std::env::var("STREAM_PROMPT").unwrap_or_else(|_| {
        "Fetch the most recent available revenue and give me a one-paragraph summary.".into()
    });

    // ── connect to the datacenter MCP server and discover its tools ──
    let client = McpClient::connect_http(&mcp_url)
        .await
        .unwrap_or_else(|e| panic!("connect MCP at {mcp_url}: {e:#}"));
    let handle = client.handle();
    let discovered = handle
        .list_openrouter_tools()
        .await
        .expect("list MCP tools");
    let names: Vec<&str> = discovered
        .iter()
        .map(|t| t.function.name.as_str())
        .collect();
    eprintln!("discovered {} MCP tools: {names:?}", discovered.len());

    let tool_def = discovered
        .iter()
        .find(|t| t.function.name == stream_tool)
        .unwrap_or_else(|| {
            panic!("tool `{stream_tool}` not advertised by the server; available: {names:?} — set STREAM_TOOL")
        });
    let parameters = tool_def
        .function
        .parameters
        .clone()
        .unwrap_or_else(|| serde_json::json!({ "type": "object", "properties": {} }));
    let description = tool_def.function.description.clone().unwrap_or_default();

    // ── build the streaming stack (Path A): one shared ChannelSink drives the SSE ──
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
        reasoning_effort: None,
    };
    let llm: Arc<dyn LlmCapability> = Arc::new(
        StreamingOpenAiLlm::from_resolved(&resolved, sink.clone()).expect("build StreamingOpenAiLlm"),
    );

    let mcp_tool: Box<dyn Tool> = Box::new(McpTool::new(
        handle,
        ToolId::BillRevenue,
        stream_tool.clone(),
        description,
        parameters,
        ArtifactKey::fetcher_records(),
    ));
    let tools = StreamingTool::wrap_all(vec![mcp_tool], sink.clone());

    // A single terminal stage that fetches AND answers, like the `analyst` — it streams its
    // answer token-by-token (OutputShape::Final).
    let cfg = SubAgentConfig {
        id: SubAgentId("analyst".into()),
        instruction: "You are a data analyst. Use the available tool(s) to fetch exactly the \
                      data the user asks for, then answer concisely. Do not invent numbers — \
                      only use what the tool returned."
            .into(),
        llm: None,
        tools: vec![ToolId::BillRevenue],
        accepts: vec![PayloadKind::Initial],
        output: None,
        capture_message: true, // terminal analyst — its answer prose is the result
    };
    let analyst: Arc<dyn SubAgent> =
        Arc::new(ConfiguredAgent::new(&cfg, llm, tools, OutputShape::Final));

    let mut agents: HashMap<SubAgentId, Arc<dyn SubAgent>> = HashMap::new();
    agents.insert(SubAgentId("analyst".into()), analyst);
    let pipe = PipelineConfig {
        id: PipelineId("stream".into()),
        stages: vec![SubAgentId("analyst".into())],
    };
    let mut orch = Orchestrator::new();
    orch.insert(pipe.id.clone(), resolve_pipeline(&pipe, &agents).unwrap());

    // ── run the pipeline on a task; drain the sink live in this task ──
    let run = tokio::spawn(async move {
        orch.run_emitting(
            &PipelineId("stream".into()),
            AgentPayload::Initial(InitialPrompt {
                prompt,
                history: vec![],
                now: SystemClock::default().now(), // stamp the turn once at the boundary
            }),
            &*sink,
        )
        .await
        // Returns the terminal payload. NB: this does NOT close the sink channel — the `agents`
        // map below still holds the analyst (and thus its LLM/tool sink clones), so the channel
        // stays open. The drain loop stops on the terminal *event*, not on channel close.
    });

    // Consume the stream until its terminal frame. We stop on `Finished` / `Error`, NOT on channel
    // close: `agents` + `orch` each hold the analyst (hence its `ChannelSink` clones), so the
    // channel outlives the run and never closes while this test's locals are alive — waiting on
    // close would deadlock. A real SSE consumer likewise stops on the terminal event, not on
    // socket close.
    let mut events = Vec::new();
    while let Some(ev) = rx.recv().await {
        let terminal = matches!(ev, AgentEvent::Finished { .. } | AgentEvent::Error { .. });
        // Stream answer tokens inline; put structural events (incl. the terminal one, which
        // `summarize` renders as `[finished] N chars`) on their own lines.
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

    // ── assert the answer arrived as a stream, then finished cleanly ──
    assert!(outcome.is_ok(), "pipeline run failed: {:?}", outcome.err());

    let content_deltas = events
        .iter()
        .filter(|e| matches!(e, AgentEvent::ContentDelta { .. }))
        .count();
    let finished: Vec<&str> = events
        .iter()
        .filter_map(|e| match e {
            AgentEvent::Finished { assistant } => Some(assistant.as_str()),
            _ => None,
        })
        .collect();

    eprintln!(
        "streamed {content_deltas} content deltas across {} events",
        events.len()
    );
    assert!(
        content_deltas >= 1,
        "the terminal stage must stream its answer as ContentDelta events"
    );
    assert_eq!(finished.len(), 1, "expected exactly one Finished event");
    assert!(
        !finished[0].trim().is_empty(),
        "the final answer must not be empty"
    );
}
