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

//! Test-only fixtures for tests that need a served [`Router`](axum::Router).
//!
//! [`build_router`](crate::server::route::build_router) takes an [`AppState`], and `AppState::mcp`
//! is an [`McpHandle`] wrapping a private rmcp `Peer<RoleClient>` that only a completed MCP
//! handshake produces. That is why the crate had no test reaching the assembled router: every
//! router test had to hand-build a throwaway `Router` instead.
//!
//! [`stub_mcp`] closes that gap by standing a stub MCP server up on a `tokio::io::duplex` pair, so
//! the handshake completes in memory with no live server and no network. [`app_state`] wraps it
//! into a full `AppState`.
//!
//! Everything here is `#[cfg(test)]` and lives behind dev-dependencies, so none of it reaches the
//! shipped binary.

use std::sync::Arc;

use reqwest::Client;
use rmcp::model::ServerInfo;
use rmcp::service::RunningService;
use rmcp::{RoleClient, ServerHandler, ServiceExt};
use tokio::sync::Mutex;

use crate::appstate::{AppState, LlmDefaults, PromptBank};
use crate::config::InsightGrants;
use crate::mcp_client::McpHandle;

/// The stub MCP server. Every [`ServerHandler`] method has a default, and these tests never call a
/// tool, so advertising server info is all it has to do — that alone is what the client's
/// `initialize` handshake waits for.
#[derive(Clone)]
struct StubMcpServer;

impl ServerHandler for StubMcpServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::default()
    }
}

/// A live in-memory MCP session.
///
/// Hold this for as long as the handle is used: dropping it tears the duplex pair down, which ends
/// the session behind [`McpHandle`].
pub(crate) struct StubMcpSession {
    pub(crate) handle: McpHandle,
    _client: RunningService<RoleClient, ()>,
    _server: tokio::task::JoinHandle<()>,
}

/// Stand up a stub MCP server over an in-memory duplex pair and return a handle onto it.
///
/// `()` is rmcp's unit client handler — the same one `McpClient::connect_http` uses in production,
/// so the client half of this is the shipped path, only over a different transport.
pub(crate) async fn stub_mcp() -> StubMcpSession {
    let (server_io, client_io) = tokio::io::duplex(4096);

    let server = tokio::spawn(async move {
        // A stub server that loses its transport is not a test failure: the client half may finish
        // and drop its end first. Swallow it rather than panicking inside the task.
        if let Ok(running) = StubMcpServer.serve(server_io).await {
            let _ = running.waiting().await;
        }
    });

    let client = ().serve(client_io).await.expect("in-memory MCP handshake should succeed");

    let handle = McpHandle::from_peer_for_tests(client.peer().clone());

    StubMcpSession {
        handle,
        _client: client,
        _server: server,
    }
}

/// LLM defaults with placeholder values.
///
/// Built literally rather than through `LlmDefaults::from_env`, so tests neither read nor need
/// `OPENROUTER_API_KEY`. Nothing here is dialled: the router tests below never reach an LLM.
fn llm_defaults() -> LlmDefaults {
    LlmDefaults {
        api_key: "test-key".to_string(),
        base_url: "http://127.0.0.1:1/v1".to_string(),
        model: "test/model".to_string(),
        app_url: None,
        app_title: None,
        temperature: 0.0,
        top_p: 1.0,
        max_tokens: 16,
    }
}

/// Empty prompt bodies. The retired-path tests never run a pipeline, so the contents are irrelevant
/// — only that the field is populated.
fn prompt_bank() -> PromptBank {
    PromptBank {
        agent_system: String::new(),
        greeting_fetcher_system: String::new(),
        greeting_analyst_system: String::new(),
        greeting_user: String::new(),
        fetcher_system: String::new(),
        analyst_system: String::new(),
        charter_system: String::new(),
        report_analyst_system: String::new(),
        report_composer_system: String::new(),
    }
}

/// The bearer token [`app_state`] accepts.
pub(crate) const TEST_TOKEN: &str = "test-token";

/// A full [`AppState`] backed by the in-memory MCP session.
///
/// `runtime: None` on purpose — the runtime is not wired, so the prompt endpoints answer `503`.
/// That is enough for route-table assertions (a missing route is a `404` regardless) and keeps the
/// fixture free of runtime config loading.
pub(crate) async fn app_state() -> (AppState, StubMcpSession) {
    let session = stub_mcp().await;

    let state = AppState {
        mcp: session.handle.clone(),
        tools: Arc::new(Vec::new()),
        instructions: Arc::new(None),
        llm: llm_defaults(),
        prompts: Arc::new(prompt_bank()),
        http: Client::new(),
        auth_token: Arc::new(TEST_TOKEN.to_string()),
        greetings: Arc::new(Mutex::new(Vec::new())),
        runtime: None,
        insight_grants: InsightGrants::default(),
        report_template: Arc::new(String::new()),
    };

    (state, session)
}
