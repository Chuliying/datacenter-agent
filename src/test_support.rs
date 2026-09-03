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

use std::collections::VecDeque;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex as StdMutex, OnceLock};
use std::thread;
use std::time::Duration;

use async_trait::async_trait;
use reqwest::Client;
use rmcp::model::{
    CallToolRequestParams, CallToolResult, Content, ListToolsResult, PaginatedRequestParams,
    ServerInfo, Tool,
};
use rmcp::service::{RequestContext, RoleServer, RunningService};
use rmcp::{RoleClient, ServerHandler, ServiceExt};
use tokio::sync::Mutex;
use tracing_subscriber::fmt::MakeWriter;
use tracing_subscriber::layer::SubscriberExt;

use crate::appstate::{AppRuntime, AppState, LlmDefaults, PromptBank};
use crate::config::{AppConfig, InsightGrants};
use crate::mcp_client::McpHandle;
use crate::runtime::audit::{AuditFailurePolicy, NoopAuditSink};
use crate::runtime::config::RuntimeConfig;
use crate::runtime::input::pipeline::InputPipeline;
use crate::runtime::registry::BuiltinRegistry;
use crate::server::falcon::{
    Permissions, PermissionsFailure, PermissionsProvider, PermissionsResult,
};

/// The stub MCP server. It can advertise a caller-supplied tool set and return deterministic text
/// for each call, while still using the production rmcp client path over an in-memory transport.
#[derive(Clone)]
struct StubMcpServer {
    tools: Arc<Vec<Tool>>,
    calls: Arc<AtomicUsize>,
}

impl ServerHandler for StubMcpServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::default()
    }

    fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> impl std::future::Future<Output = Result<ListToolsResult, rmcp::ErrorData>> + Send + '_
    {
        std::future::ready(Ok(ListToolsResult::with_all_items(
            self.tools.as_ref().clone(),
        )))
    }

    fn call_tool(
        &self,
        request: CallToolRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> impl std::future::Future<Output = Result<CallToolResult, rmcp::ErrorData>> + Send + '_
    {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let text = format!("stub data for {}", request.name);
        std::future::ready(Ok(CallToolResult::success(vec![Content::text(text)])))
    }
}

/// A live in-memory MCP session.
///
/// Hold this for as long as the handle is used: dropping it tears the duplex pair down, which ends
/// the session behind [`McpHandle`].
pub(crate) struct StubMcpSession {
    pub(crate) handle: McpHandle,
    calls: Arc<AtomicUsize>,
    _client: RunningService<RoleClient, ()>,
    _server: tokio::task::JoinHandle<()>,
}

/// Stand up a stub MCP server over an in-memory duplex pair and return a handle onto it.
///
/// `()` is rmcp's unit client handler — the same one `McpClient::connect_http` uses in production,
/// so the client half of this is the shipped path, only over a different transport.
pub(crate) async fn stub_mcp() -> StubMcpSession {
    stub_mcp_with_tools(std::iter::empty::<String>()).await
}

/// Stand up a stub MCP server that advertises the named tools and counts calls.
pub(crate) async fn stub_mcp_with_tools<I, S>(tool_names: I) -> StubMcpSession
where
    I: IntoIterator<Item = S>,
    S: Into<String>,
{
    let tools = Arc::new(
        tool_names
            .into_iter()
            .map(|name| stub_tool(name.into()))
            .collect::<Vec<_>>(),
    );
    let calls = Arc::new(AtomicUsize::new(0));
    let server_handler = StubMcpServer {
        tools,
        calls: calls.clone(),
    };
    let (server_io, client_io) = tokio::io::duplex(4096);

    let server = tokio::spawn(async move {
        // A stub server that loses its transport is not a test failure: the client half may finish
        // and drop its end first. Swallow it rather than panicking inside the task.
        if let Ok(running) = server_handler.serve(server_io).await {
            let _ = running.waiting().await;
        }
    });

    let client = ().serve(client_io).await.expect("in-memory MCP handshake should succeed");

    let handle = McpHandle::from_peer_for_tests(client.peer().clone());

    StubMcpSession {
        handle,
        calls,
        _client: client,
        _server: server,
    }
}

impl StubMcpSession {
    pub(crate) fn tool_calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

fn stub_tool(name: String) -> Tool {
    Tool {
        name: name.into(),
        title: None,
        description: Some("deterministic test data tool".to_string().into()),
        input_schema: Arc::new(
            serde_json::json!({
                "type": "object",
                "properties": {},
                "additionalProperties": true
            })
            .as_object()
            .expect("stub schema is an object")
            .clone(),
        ),
        output_schema: None,
        annotations: None,
        execution: None,
        icons: None,
        meta: None,
    }
}

/// LLM defaults with placeholder values.
///
/// Built literally rather than through `LlmDefaults::from_env`, so tests neither read nor need
/// `OPENROUTER_API_KEY`. Nothing here is dialled: the router tests below never reach an LLM.
fn llm_defaults_with_base_url(base_url: impl Into<String>) -> LlmDefaults {
    LlmDefaults {
        api_key: "test-key".to_string(),
        base_url: base_url.into(),
        model: "test/model".to_string(),
        app_url: None,
        app_title: None,
        temperature: 0.0,
        top_p: 1.0,
        max_tokens: 16,
    }
}

fn llm_defaults() -> LlmDefaults {
    llm_defaults_with_base_url("http://127.0.0.1:1/v1")
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
        ss_fetcher_system: String::new(),
        ss_analyst_system: String::new(),
        ss_charter_system: String::new(),
    }
}

#[derive(Clone)]
struct SharedTracingWriter {
    buffer: Arc<StdMutex<Vec<u8>>>,
}

impl Write for SharedTracingWriter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.buffer
            .lock()
            .expect("tracing capture lock should not be poisoned")
            .extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> MakeWriter<'a> for SharedTracingWriter {
    type Writer = Self;

    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

static TRACING_CAPTURE: OnceLock<Arc<StdMutex<Vec<u8>>>> = OnceLock::new();
static TRACING_CAPTURE_INIT: OnceLock<()> = OnceLock::new();

/// Install one process-wide tracing capture for tests and return its shared buffer.
///
/// The global installation is intentionally test-only. Production logging remains configured by
/// `main`, while this fixture lets assertions observe events emitted by spawned async tasks.
pub(crate) fn install_tracing_capture() -> Arc<StdMutex<Vec<u8>>> {
    let buffer = TRACING_CAPTURE
        .get_or_init(|| Arc::new(StdMutex::new(Vec::new())))
        .clone();
    TRACING_CAPTURE_INIT.get_or_init(|| {
        let writer = SharedTracingWriter {
            buffer: buffer.clone(),
        };
        let subscriber = tracing_subscriber::registry().with(
            tracing_subscriber::fmt::layer()
                .with_ansi(false)
                .with_writer(writer),
        );
        let _ = tracing::subscriber::set_global_default(subscriber);
    });
    buffer
}

/// The bearer token [`app_state`] accepts.
pub(crate) const TEST_TOKEN: &str = "test-token";

/// Default provider for route-table tests. The identity middleware is not reached by those tests;
/// tests that exercise it replace this field with a scripted provider.
pub(crate) struct UnusedPermissionsProvider;

#[async_trait]
impl PermissionsProvider for UnusedPermissionsProvider {
    async fn permissions(&self, _token: &str) -> Result<Permissions, PermissionsFailure> {
        Err(PermissionsFailure::Unavailable)
    }
}

/// Scriptable in-process Falcon provider for identity and router tests.
///
/// Responses are consumed in order and then the last configured response is replayed. The call
/// counter makes cache assertions independent from the provider implementation.
pub(crate) struct ScriptedPermissionsProvider {
    responses: tokio::sync::Mutex<VecDeque<PermissionsResult>>,
    fallback: PermissionsResult,
    calls: AtomicUsize,
}

impl ScriptedPermissionsProvider {
    pub(crate) fn new(responses: impl IntoIterator<Item = PermissionsResult>) -> Arc<Self> {
        let responses = responses.into_iter().collect::<VecDeque<_>>();
        let fallback = responses
            .back()
            .cloned()
            .unwrap_or(Err(PermissionsFailure::Unavailable));
        Arc::new(Self {
            responses: tokio::sync::Mutex::new(responses),
            fallback,
            calls: AtomicUsize::new(0),
        })
    }

    pub(crate) fn documented(user_id: i64, codes: &[&str]) -> Arc<Self> {
        let permissions = Permissions {
            user_id,
            codes: codes.iter().map(|code| (*code).to_string()).collect(),
        };
        Self::new([Ok(permissions)])
    }

    pub(crate) fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl PermissionsProvider for ScriptedPermissionsProvider {
    async fn permissions(&self, _token: &str) -> PermissionsResult {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.responses
            .lock()
            .await
            .pop_front()
            .unwrap_or_else(|| self.fallback.clone())
    }
}

/// A full [`AppState`] backed by the in-memory MCP session.
///
/// `runtime: None` on purpose — the runtime is not wired, so the prompt endpoints answer `503`.
/// That is enough for route-table assertions (a missing route is a `404` regardless) and keeps the
/// fixture free of runtime config loading.
pub(crate) async fn app_state() -> (AppState, StubMcpSession) {
    app_state_with_provider(Arc::new(UnusedPermissionsProvider)).await
}

/// A full app state with an injectable permissions provider.
pub(crate) async fn app_state_with_provider(
    provider: Arc<dyn PermissionsProvider>,
) -> (AppState, StubMcpSession) {
    let session = stub_mcp().await;
    let config = AppConfig::load("config/config.toml").expect("shipped config should load");

    let state = AppState {
        mcp: session.handle.clone(),
        tools: Arc::new(Vec::new()),
        instructions: Arc::new(None),
        llm: llm_defaults(),
        prompts: Arc::new(prompt_bank()),
        http: Client::new(),
        auth_token: Arc::new(TEST_TOKEN.to_string()),
        actor_key_pepper: Arc::new(b"test-actor-key-pepper-0123456789".to_vec()),
        permissions_provider: provider,
        greetings: Arc::new(Mutex::new(Vec::new())),
        runtime: None,
        insight_grants: InsightGrants::default(),
        ss_chat_grants: crate::config::SsChatGrants::default(),
        report_template: Arc::new(String::new()),
        rate_limit: crate::config::RateLimitConfig::default(),
        authz: config.authz.expect("shipped config declares authz"),
        report_grants: config
            .report_grants
            .expect("shipped config declares report grants"),
    };

    (state, session)
}

/// The same fixture with the shipped runtime capability pack fully wired.
///
/// The LLM/MCP handles remain deterministic test doubles; callers can exercise the runtime prelude,
/// actor-scoped memory, audit sequencing, and identity path without depending on process ENV or a
/// live Falcon/OpenRouter service.
pub(crate) async fn runtime_app_state(
    provider: Arc<dyn PermissionsProvider>,
) -> (AppState, StubMcpSession) {
    let (mut state, session) = app_state_with_provider(provider).await;
    let app_config = AppConfig::load("config/config.toml").expect("shipped config should load");
    let refs = app_config
        .runtime
        .as_ref()
        .expect("runtime refs should exist");
    let registry = BuiltinRegistry::default();
    let runtime_config = RuntimeConfig::load(refs, &registry).expect("runtime config should load");
    let answer_policy = registry
        .build_answer_policy(&runtime_config)
        .expect("answer policy should build");
    let llm_normalizer = registry
        .build_llm_normalizer(&runtime_config)
        .expect("normalizer should build");
    let sessions = registry
        .build_memory(&runtime_config)
        .expect("memory should build");
    state.runtime = Some(Arc::new(AppRuntime {
        enabled: true,
        config: Arc::new(runtime_config),
        input_pipeline: InputPipeline::default(),
        answer_policy,
        llm_normalizer,
        sessions,
        audit_sink: Arc::new(NoopAuditSink),
        audit_failure_policy: AuditFailurePolicy::FailClosed,
    }));
    (state, session)
}

/// The runtime-wired fixture with production prompt/tool discovery and a caller-supplied local
/// OpenAI-compatible endpoint. This is used by integration tests that must cross the handler →
/// pipeline → MCP boundary without credentials or live services.
pub(crate) async fn runtime_app_state_with_fixtures(
    provider: Arc<dyn PermissionsProvider>,
    llm_base_url: impl Into<String>,
    tool_names: &[&str],
) -> (AppState, StubMcpSession) {
    let session = stub_mcp_with_tools(tool_names.iter().copied()).await;
    let config = AppConfig::load("config/config.toml").expect("shipped config should load");
    let tools = Arc::new(
        session
            .handle
            .list_openrouter_tools()
            .await
            .expect("stub MCP tools should convert to OpenAI tools"),
    );
    let refs = config.runtime.as_ref().expect("runtime refs should exist");
    let registry = BuiltinRegistry::default();
    let runtime_config = RuntimeConfig::load(refs, &registry).expect("runtime config should load");
    let answer_policy = registry
        .build_answer_policy(&runtime_config)
        .expect("answer policy should build");
    let llm_normalizer = registry
        .build_llm_normalizer(&runtime_config)
        .expect("normalizer should build");
    let sessions = registry
        .build_memory(&runtime_config)
        .expect("memory should build");
    let prompts = PromptBank::from_app_config(&config).expect("shipped prompts should load");
    let state = AppState {
        mcp: session.handle.clone(),
        tools,
        instructions: Arc::new(None),
        llm: llm_defaults_with_base_url(llm_base_url),
        prompts: Arc::new(prompts),
        http: Client::new(),
        auth_token: Arc::new(TEST_TOKEN.to_string()),
        actor_key_pepper: Arc::new(b"test-actor-key-pepper-0123456789".to_vec()),
        permissions_provider: provider,
        ss_chat_grants: crate::config::SsChatGrants::default(),
        greetings: Arc::new(Mutex::new(Vec::new())),
        runtime: Some(Arc::new(AppRuntime {
            enabled: true,
            config: Arc::new(runtime_config),
            input_pipeline: InputPipeline::default(),
            answer_policy,
            llm_normalizer,
            sessions,
            audit_sink: Arc::new(NoopAuditSink),
            audit_failure_policy: AuditFailurePolicy::FailClosed,
        })),
        insight_grants: config.insight_grants.clone(),
        report_template: Arc::new(config.report_template.clone()),
        rate_limit: config.rate_limit.clone(),
        authz: config.authz.expect("shipped config declares authz"),
        report_grants: config
            .report_grants
            .expect("shipped config declares report grants"),
    };
    (state, session)
}

/// A local streaming `chat/completions` endpoint for tests of the real async-openai adapter.
///
/// The script chooses a data-tool call, `emit_chart`, or `emit_report` from the advertised tool
/// names, then returns a final message after the tool result is present. It is deliberately a
/// tiny HTTP listener rather than an HTTP mock dependency, so the request URL, JSON body, and
/// stream framing all cross the same client code used in production.
pub(crate) struct ScriptedChatCompletions {
    pub(crate) base_url: String,
    requests: Arc<AtomicUsize>,
    /// Every request body the stub served, in order. Lets a test assert *what* reached the
    /// LLM — e.g. that a second turn's fetcher request carries the first turn's memory
    /// context — rather than only how many calls happened.
    bodies: Arc<StdMutex<Vec<serde_json::Value>>>,
    stop: Arc<AtomicBool>,
    thread: Option<thread::JoinHandle<()>>,
}

impl ScriptedChatCompletions {
    pub(crate) fn start() -> Self {
        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind scripted LLM listener");
        listener
            .set_nonblocking(true)
            .expect("scripted listener should be nonblocking");
        let address = listener.local_addr().expect("scripted listener address");
        let requests = Arc::new(AtomicUsize::new(0));
        let bodies: Arc<StdMutex<Vec<serde_json::Value>>> = Arc::new(StdMutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let thread_requests = requests.clone();
        let thread_bodies = bodies.clone();
        let thread_stop = stop.clone();
        let thread = thread::spawn(move || {
            while !thread_stop.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((stream, _)) => {
                        thread_requests.fetch_add(1, Ordering::SeqCst);
                        stream
                            .set_nonblocking(false)
                            .expect("scripted connection should be blocking");
                        if let Err(err) = serve_scripted_completion(stream, &thread_bodies) {
                            eprintln!("scripted LLM listener failed: {err}");
                            break;
                        }
                    }
                    Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(1));
                    }
                    Err(err) => {
                        eprintln!("scripted LLM listener accept failed: {err}");
                        break;
                    }
                }
            }
        });
        Self {
            base_url: format!("http://{address}/v1"),
            requests,
            bodies,
            stop,
            thread: Some(thread),
        }
    }

    pub(crate) fn requests(&self) -> usize {
        self.requests.load(Ordering::SeqCst)
    }

    /// Every request body served so far, serialized to one string per request.
    pub(crate) fn request_bodies(&self) -> Vec<String> {
        self.bodies
            .lock()
            .expect("scripted LLM body lock should not be poisoned")
            .iter()
            .map(|body| body.to_string())
            .collect()
    }
}

impl Drop for ScriptedChatCompletions {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn serve_scripted_completion(
    mut stream: TcpStream,
    bodies: &StdMutex<Vec<serde_json::Value>>,
) -> std::io::Result<()> {
    let body = read_http_body(&mut stream)?;
    let request: serde_json::Value = serde_json::from_slice(&body).map_err(|err| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("invalid request: {err}"),
        )
    })?;
    bodies
        .lock()
        .expect("scripted LLM body lock should not be poisoned")
        .push(request.clone());
    let (kind, name, arguments, text) = scripted_reply(&request);
    let payload = match kind {
        ScriptedReply::Tool => tool_call_stream(&name, &arguments),
        ScriptedReply::Message => message_stream(&text),
    };
    stream.write_all(
        b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nCache-Control: no-cache\r\nConnection: close\r\n\r\n",
    )?;
    stream.write_all(payload.as_bytes())?;
    stream.flush()
}

fn read_http_body(stream: &mut TcpStream) -> std::io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    let mut chunk = [0_u8; 4096];
    let (header_end, content_length) = loop {
        let read = stream.read(&mut chunk)?;
        if read == 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "request ended before headers",
            ));
        }
        bytes.extend_from_slice(&chunk[..read]);
        if let Some(end) = bytes.windows(4).position(|window| window == b"\r\n\r\n") {
            let header_end = end + 4;
            let headers = String::from_utf8_lossy(&bytes[..end]);
            let length = headers
                .lines()
                .find_map(|line| {
                    line.strip_prefix("Content-Length:")
                        .or_else(|| line.strip_prefix("content-length:"))
                })
                .and_then(|value| value.trim().parse::<usize>().ok())
                .unwrap_or(0);
            break (header_end, length);
        }
    };
    while bytes.len() < header_end + content_length {
        let read = stream.read(&mut chunk)?;
        if read == 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "request ended before body",
            ));
        }
        bytes.extend_from_slice(&chunk[..read]);
    }
    Ok(bytes[header_end..header_end + content_length].to_vec())
}

#[derive(Clone, Copy)]
enum ScriptedReply {
    Tool,
    Message,
}

fn scripted_reply(request: &serde_json::Value) -> (ScriptedReply, String, String, String) {
    let tool_names = request["tools"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|tool| tool["function"]["name"].as_str())
        .collect::<Vec<_>>();
    let has_tool_result = request["messages"]
        .as_array()
        .into_iter()
        .flatten()
        .any(|message| message["role"] == "tool");

    if !has_tool_result {
        if tool_names.contains(&"emit_report") {
            return (
                ScriptedReply::Tool,
                "emit_report".into(),
                serde_json::to_string(&sample_report_data()).expect("sample report serializes"),
                String::new(),
            );
        }
        if tool_names.contains(&"emit_chart") {
            return (
                ScriptedReply::Tool,
                "emit_chart".into(),
                r#"{"charts":[]}"#.into(),
                String::new(),
            );
        }
        // The first advertised data tool, whatever pipeline this is — hardcoding a name here
        // silently pins the stub to the starcharger tool set and breaks any other pipeline.
        if let Some(name) = tool_names
            .iter()
            .find(|name| **name != "emit_chart" && **name != "emit_report")
        {
            return (
                ScriptedReply::Tool,
                (*name).to_string(),
                "{}".into(),
                String::new(),
            );
        }
    }

    (
        ScriptedReply::Message,
        String::new(),
        String::new(),
        "scripted stage complete".into(),
    )
}

fn sample_report_data() -> serde_json::Value {
    serde_json::json!({
        "report": {
            "title": "測試報告",
            "organization": "測試網路",
            "brand": "Test",
            "periodLabel": "2026年",
            "dateFrom": "2026-01-01",
            "dateTo": "2026-01-31",
            "asOf": "2026-01-31",
            "locale": "zh-TW",
            "currency": "TWD",
            "partialPeriodNote": ""
        },
        "summary": {
            "latestCompletedPeriod": "2026-01",
            "topStationPeriodLabel": "2026年"
        },
        "insight": {"headline": "測試摘要", "paragraphs": ["測試段落"]},
        "periods": [{
            "period": "2026-01",
            "revenue": 1.0,
            "revenueMom": 0.0,
            "kwh": 1.0,
            "sessions": 1,
            "newMembers": 1,
            "totalMembers": 1,
            "activeMembers": 1,
            "stations": 1,
            "chargers": 1,
            "partial": false
        }],
        "stationRanking": []
    })
}

fn tool_call_stream(name: &str, arguments: &str) -> String {
    let chunk = serde_json::json!({
        "id": "chatcmpl-scripted",
        "object": "chat.completion.chunk",
        "created": 1,
        "model": "test/model",
        "choices": [{
            "index": 0,
            "delta": {
                "role": "assistant",
                "tool_calls": [{
                    "index": 0,
                    "id": "call-scripted",
                    "type": "function",
                    "function": {"name": name, "arguments": arguments}
                }]
            },
            "finish_reason": "tool_calls"
        }],
        "usage": null,
        "system_fingerprint": null
    });
    let usage = usage_chunk();
    format!("data: {chunk}\n\ndata: {usage}\n\ndata: [DONE]\n\n")
}

fn message_stream(text: &str) -> String {
    let chunk = serde_json::json!({
        "id": "chatcmpl-scripted",
        "object": "chat.completion.chunk",
        "created": 1,
        "model": "test/model",
        "choices": [{
            "index": 0,
            "delta": {"role": "assistant", "content": text},
            "finish_reason": "stop"
        }],
        "usage": null,
        "system_fingerprint": null
    });
    let usage = usage_chunk();
    format!("data: {chunk}\n\ndata: {usage}\n\ndata: [DONE]\n\n")
}

fn usage_chunk() -> serde_json::Value {
    serde_json::json!({
        "id": "chatcmpl-scripted",
        "object": "chat.completion.chunk",
        "created": 1,
        "model": "test/model",
        "choices": [],
        "usage": {"prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2},
        "system_fingerprint": null
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tracing_capture_observes_output_from_a_spawned_task() {
        // Must NOT clear() the buffer (finding #7): TRACING_CAPTURE is process-global and shared
        // with every other test, including AC-002 which asserts the *absence* of a secret in it.
        // A clear() landing between AC-002's request and its read would mask a real leak. This
        // canary instead emits a unique marker and only checks that marker is present.
        let buffer = install_tracing_capture();
        let marker = "ss-canary-marker-7f3a-spawned";

        std::thread::spawn(move || {
            tracing::info!(target: "test_support", fixture_marker = marker, "fixture event");
        })
        .join()
        .expect("spawned tracing task should finish");

        let output = String::from_utf8(
            buffer
                .lock()
                .expect("tracing capture lock should not be poisoned")
                .clone(),
        )
        .expect("captured tracing output should be UTF-8");
        assert!(
            output.contains(marker),
            "the spawned task's marker must reach the process-global capture buffer"
        );
    }
}
