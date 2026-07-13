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

//! Live integration test: the **fetcher** sub-agent against the real datacenter.
//!
//! This is the "actually touches the datacenter" counterpart to the fetcher unit test in
//! `src/agent/engine.rs`. Same [`ConfiguredAgent`] engine, but with a real
//! [`OpenAiLlm`](datacenter_agent::agent::llm::OpenAiLlm) and a real
//! [`McpTool`](datacenter_agent::agent::tools::McpTool) instead of a scripted LLM and a mock
//! tool. It is **`#[ignore]`d** because it needs a running MCP server and an OpenRouter key —
//! run it by hand:
//!
//! ```text
//! cargo test --test fetcher_datacenter -- --ignored --nocapture
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
//! - `FETCHER_TOOL` — the MCP tool the fetcher is granted (default `bill_revenue`)
//! - `FETCHER_PROMPT` — the user request (default asks for recent revenue)

use std::sync::Arc;

use datacenter_agent::agent::clock::{Clock, SystemClock};
use datacenter_agent::agent::config::{OutputShape, SubAgentConfig, SubAgentId};
use datacenter_agent::agent::config::{Provider, ResolvedLlm};
use datacenter_agent::agent::engine::{ConfiguredAgent, SubAgent};
use datacenter_agent::agent::llm::OpenAiLlm;
use datacenter_agent::agent::payload::{
    AgentPayload, ArtifactKey, InitialPrompt, LlmCapability, PayloadKind, Tool,
};
use datacenter_agent::agent::tools::{McpTool, ToolId};
use datacenter_agent::mcp_client::McpClient;

/// Read a required env var, failing with a message that names what to set.
fn require_env(key: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| {
        panic!(
            "integration test needs `{key}` set (see this file's module docs); run with --ignored"
        )
    })
}

#[tokio::test]
#[ignore = "touches the live datacenter MCP server + OpenRouter; run with --ignored"]
async fn fetcher_fetches_real_data_from_the_datacenter() {
    let _ = dotenvy::dotenv();

    let mcp_url = require_env("DATACENTER_MCP_URL");
    let api_key = require_env("OPENROUTER_API_KEY");
    let model = require_env("OPENROUTER_MODEL");
    let base_url = std::env::var("OPENROUTER_BASE_URL")
        .unwrap_or_else(|_| "https://openrouter.ai/api/v1".into());
    let fetcher_tool = std::env::var("FETCHER_TOOL").unwrap_or_else(|_| "bill_revenue".into());
    let prompt = std::env::var("FETCHER_PROMPT")
        .unwrap_or_else(|_| "Fetch the most recent available revenue figures.".into());

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

    // Pick the tool the fetcher is granted; its real schema drives the advertised parameters.
    let tool_def = discovered
        .iter()
        .find(|t| t.function.name == fetcher_tool)
        .unwrap_or_else(|| {
            panic!("tool `{fetcher_tool}` not advertised by the server; available: {names:?} — set FETCHER_TOOL")
        });
    let parameters = tool_def
        .function
        .parameters
        .clone()
        .unwrap_or_else(|| serde_json::json!({ "type": "object", "properties": {} }));
    let description = tool_def.function.description.clone().unwrap_or_default();

    // ── build the fetcher: a ConfiguredAgent with a real LLM + one real MCP data tool ──
    let resolved = ResolvedLlm {
        provider: Provider::OpenRouter,
        base_url,
        model,
        temperature: 0.2,
        top_p: 0.1,
        max_tokens: 2048,
        api_key: Some(api_key),
        reasoning_effort: None,
        app_url: None,
        app_title: None,
    };
    let llm: Arc<dyn LlmCapability> =
        Arc::new(OpenAiLlm::from_resolved(&resolved).expect("build OpenAiLlm"));

    // The advertised name is the canonical ToolId string (`bill_revenue`); `fetcher_tool` is
    // the raw MCP name actually sent to the server (they coincide by default).
    let mcp_tool: Box<dyn Tool> = Box::new(McpTool::new(
        handle,
        ToolId::BillRevenue,
        fetcher_tool.clone(),
        description,
        parameters,
        ArtifactKey::fetcher_records(),
    ));

    let cfg = SubAgentConfig {
        id: SubAgentId("fetcher".into()),
        instruction: "You are a data fetcher. Use the available tool(s) to fetch exactly the \
                      data the user asks for, then briefly confirm what you fetched. Do not \
                      invent numbers — only report what the tool returned."
            .into(),
        llm: None,
        tools: vec![ToolId::BillRevenue],
        accepts: vec![PayloadKind::Initial],
        output: None,
        capture_message: false, // tool-only fetcher — its confirmation note is throwaway
    };
    // The fetcher is non-terminal in the report pipeline → Intermediate.
    let fetcher = ConfiguredAgent::new(&cfg, llm, vec![mcp_tool], OutputShape::Intermediate);

    // ── run it against the real datacenter ──
    let out = fetcher
        .run(AgentPayload::Initial(InitialPrompt {
            prompt,
            history: vec![],
            now: SystemClock::default().now(), // stamp the turn once at the boundary
        }))
        .await
        .expect("fetcher run should succeed");

    let _ = client.shutdown().await;

    // ── assert it produced the fetcher.records artifact ──
    match out {
        AgentPayload::Intermediate(data) => {
            let records = data
                .artifacts
                .get(&ArtifactKey::fetcher_records())
                .expect("fetcher must produce a `fetcher.records` artifact from the tool result");
            eprintln!("fetcher.records =\n{records}");
            assert!(
                !records.to_string().trim().is_empty(),
                "the fetched records must not be empty"
            );
        }
        other => panic!("expected Intermediate, got {:?}", other.kind()),
    }
}
