//! # AgentPayload — normative contract + suggested implementation
//!
//! This file has two parts, and the distinction is load-bearing:
//!
//! - **PART A — NORMATIVE.** The payload data model and the shared error type. These are
//!   *binding*: every agent and every orchestrator must speak exactly these types, or
//!   they are not part of this system. The **behavioral rules** (morphism shape, falling
//!   convention, isolation semantics, produce-don't-mutate, producer-namespaced keys) are
//!   equally binding and are documented alongside the types.
//! - **PART B — SUGGESTED.** *One* Rust encoding of an agent: an async LLM capability, a
//!   tool abstraction, a generic tool-use loop, a `SubAgent` trait, and two example
//!   agents. This is *advisory*. Swap any of it out — a different trait shape, a different
//!   DI mechanism, no trait at all — as long as PART A and the behavioral rules still hold.
//!
//! ## LLM communication is common to every agent
//!
//! A data-fetcher is itself an LLM that *decides which tool to call*; a report-writer is
//! an LLM that writes prose. So the LLM capability is shared by all agents, and
//! **isolation is defined by the tool set an agent exposes to its LLM**, not by whether an
//! agent has an LLM. The LLM *selects* among the exposed tools at run time; the agent
//! *bounds* that set. A writer given no data tools cannot fetch — the tool is absent from
//! its set and any hallucinated call to it is rejected at dispatch.
//!
//! Because tools are a runtime set here, that boundary is enforced at *construction* plus a
//! *dispatch guard*, not at compile time (see `run_llm_loop`). To recover a compile-time
//! guarantee, make the tool set a typed capability parameter instead of a `Vec` — noted in
//! the Contract's open items.
//!
//! ## Async / async-openai
//!
//! The LLM capability is `async` to fit `async-openai`. The domain stays SDK-agnostic; a
//! reference adapter that implements [`LlmCapability`] with async-openai lives behind the
//! `openai` feature in [`openai_adapter`].

#![allow(dead_code)] // skeleton: some example variants/keys are not yet wired up

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt;

// ===========================================================================
// PART A — NORMATIVE CONTRACT (binding: the data every agent must speak)
// ===========================================================================

/// One user/assistant turn, used inside [`InitialPrompt::history`]. Structurally this is
/// `FinalResult` without metadata, but it is a *distinct* type on purpose.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Exchange {
    pub user: String,
    pub assistant: String,
}

/// A value carried in the artifact map. A *closed* enum: `Display` is the "cast to string
/// if you don't care about the type" view; a `match` is the checked "downcast". Closedness
/// is what lets the whole payload derive `Clone` + `Serialize`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum ArtifactValue {
    Text(String),
    Json(serde_json::Value),
    Number(f64),
    // EXTEND: variants the orchestration designer needs (Rows, Table, Bytes, ...).
}

impl fmt::Display for ArtifactValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ArtifactValue::Text(s) => f.write_str(s),
            ArtifactValue::Json(v) => write!(f, "{v}"),
            ArtifactValue::Number(n) => write!(f, "{n}"),
        }
    }
}

/// The *strict* key: a compile-time enum, so a mistyped key is a compile error. Variants
/// are namespaced by producer and render to a canonical dotted string (`fetcher.records`)
/// that is *both* the log form and the serialized form (JSON object keys must be strings).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Serialize, Deserialize)]
#[serde(into = "String", try_from = "String")]
pub enum ArtifactKey {
    FetcherRecords,
    FetcherSchema,
    WriterDraft,
    // EXTEND: one variant per wire the orchestration designer connects.
}

impl fmt::Display for ArtifactKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            ArtifactKey::FetcherRecords => "fetcher.records",
            ArtifactKey::FetcherSchema => "fetcher.schema",
            ArtifactKey::WriterDraft => "writer.draft",
        })
    }
}

impl std::str::FromStr for ArtifactKey {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "fetcher.records" => Ok(ArtifactKey::FetcherRecords),
            "fetcher.schema" => Ok(ArtifactKey::FetcherSchema),
            "writer.draft" => Ok(ArtifactKey::WriterDraft),
            other => Err(format!("unknown ArtifactKey: {other}")),
        }
    }
}

impl From<ArtifactKey> for String {
    fn from(k: ArtifactKey) -> String {
        k.to_string()
    }
}
impl TryFrom<String> for ArtifactKey {
    type Error = String;
    fn try_from(s: String) -> Result<Self, Self::Error> {
        s.parse()
    }
}

/// Pipeline entry: the user's request plus prior turns. No system prompt — each agent
/// carries its own designed instruction. `history` is a plain `Vec` (no `Option`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct InitialPrompt {
    pub prompt: String,
    pub history: Vec<Exchange>,
}

/// Intermediate working data — never the user-facing output. `prompt` carries the
/// instruction forward; `artifacts` is the KV surface the orchestrator wires between agents.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct IntermediateData {
    pub prompt: String,
    pub artifacts: HashMap<ArtifactKey, ArtifactValue>,
}

/// The user-facing result. Its own type (not `Exchange`) so metadata can land here later.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FinalResult {
    pub user: String,
    pub assistant: String,
    // EXTEND: stop_reason, token usage, latency, ...
}

/// The value every agent consumes and produces. This sum type IS the contract.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum AgentPayload {
    Initial(InitialPrompt),
    Intermediate(IntermediateData),
    Final(FinalResult),
}

/// A cheap tag so acceptance checks and mismatch errors need not carry a whole payload.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PayloadKind {
    Initial,
    Intermediate,
    Final,
}

impl AgentPayload {
    pub fn kind(&self) -> PayloadKind {
        match self {
            AgentPayload::Initial(_) => PayloadKind::Initial,
            AgentPayload::Intermediate(_) => PayloadKind::Intermediate,
            AgentPayload::Final(_) => PayloadKind::Final,
        }
    }
}

/// The shared error type. Part of the contract: agents fail *into this*, so an
/// orchestrator can route or log uniformly. Mismatch is the "falling convention" — never a
/// panic when an agent is handed a variant it does not accept.
#[derive(Debug)]
pub enum AgentError {
    Mismatch {
        expected: &'static [PayloadKind],
        got: PayloadKind,
    },
    MissingArtifact(ArtifactKey),
    /// The LLM tried to call a tool this agent does not expose (isolation, at dispatch).
    UnknownTool(String),
    /// An underlying capability (LLM transport / tool) failed. EXTEND with a taxonomy.
    Capability(String),
}

impl fmt::Display for AgentError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AgentError::Mismatch { expected, got } => {
                write!(f, "payload mismatch: expected one of {expected:?}, got {got:?}")
            }
            AgentError::MissingArtifact(k) => write!(f, "missing artifact: {k}"),
            AgentError::UnknownTool(name) => write!(f, "agent does not expose tool: {name}"),
            AgentError::Capability(m) => write!(f, "capability error: {m}"),
        }
    }
}

impl std::error::Error for AgentError {}

// The NORMATIVE morphism, stated once: every agent behaves as
//     async fn(AgentPayload) -> Result<AgentPayload, AgentError>
// obeying the behavioral rules above. The `SubAgent` trait in PART B is one *encoding* of
// this; it is advisory. The morphism, the falling convention, and the isolation semantics
// are what bind.

// ===========================================================================
// PART B — SUGGESTED IMPLEMENTATION (advisory: one way to build an agent)
// ===========================================================================

// --- The LLM transport, abstracted so the vendor SDK stays at the edge -----

/// A message in the chat protocol, minimal but sufficient for tool use. Maps directly onto
/// async-openai's message types (see the adapter).
#[derive(Clone, Debug)]
pub enum LlmMessage {
    System(String),
    User(String),
    Assistant {
        content: Option<String>,
        tool_calls: Vec<ToolCall>,
    },
    Tool {
        tool_call_id: String,
        content: String,
    },
}

/// A tool call the model decided to make.
#[derive(Clone, Debug)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: serde_json::Value,
}

/// What the model returned: either a final message, or a batch of tool calls to run.
#[derive(Clone, Debug)]
pub enum LlmResponse {
    Message(String),
    ToolCalls(Vec<ToolCall>),
}

/// The function schema advertised to the model for one tool.
#[derive(Clone, Debug)]
pub struct ToolSchema {
    pub name: String,
    pub description: String,
    /// JSON Schema for the tool's arguments.
    pub parameters: serde_json::Value,
}

/// The capability COMMON to every agent: talk to an LLM. `async` to fit async-openai.
#[async_trait]
pub trait LlmCapability: Send + Sync {
    async fn chat(
        &self,
        messages: &[LlmMessage],
        tools: &[ToolSchema],
    ) -> Result<LlmResponse, AgentError>;
}

/// The result of one tool invocation. **`Rejected` is not an error** — it is a retryable,
/// model-facing outcome (bad arguments, failed validation), distinct from a fatal
/// `Err(AgentError)` (transport/wiring). See the tool contract; `run_llm_loop` feeds a
/// `Rejected { reason }` back to the model so it can correct, and does **not** record an
/// artifact for it. This is what makes a validating/sink tool "loop until valid" for free.
#[derive(Clone, Debug)]
pub enum ToolOutcome {
    /// The call succeeded; the value fills the tool's `target` artifact slot.
    Produced(ArtifactValue),
    /// The call was rejected (e.g. schema validation failed). Fed back to the model as a
    /// tool message; no artifact is recorded. Retryable within the loop's step cap.
    Rejected { reason: String },
}

/// A tool the LLM may choose to call — a **named capability the LLM invokes, whose result
/// fills an artifact slot** (`target`). This spans data fetches (MCP), output *sinks* that
/// validate the model's own structured output, and pure *validators/compute* (a calculator).
/// `target` is wired by the orchestration designer, never chosen by the LLM.
#[async_trait]
pub trait Tool: Send + Sync {
    fn schema(&self) -> ToolSchema;
    fn target(&self) -> ArtifactKey;
    /// `Ok(Produced)` fills `target`; `Ok(Rejected)` is fed back for a retry; `Err` is fatal.
    async fn call(&self, arguments: serde_json::Value) -> Result<ToolOutcome, AgentError>;
}

/// The tool-use loop: the model sees `tools` and DECIDES which to call; each call is
/// dispatched to the matching [`Tool`], results are fed back, and we repeat until the model
/// returns a final message. Results are collected into `produced`, keyed by each tool's
/// `target`. A call to a tool this agent does not own is rejected — an agent can only ever
/// run tools in its own set. This is the isolation boundary, guarded at dispatch.
pub async fn run_llm_loop<L: LlmCapability>(
    llm: &L,
    system: &str,
    user: &str,
    tools: &[Box<dyn Tool>],
) -> Result<(String, HashMap<ArtifactKey, ArtifactValue>), AgentError> {
    let schemas: Vec<ToolSchema> = tools.iter().map(|t| t.schema()).collect();
    let mut messages = vec![
        LlmMessage::System(system.to_string()),
        LlmMessage::User(user.to_string()),
    ];
    let mut produced: HashMap<ArtifactKey, ArtifactValue> = HashMap::new();

    const MAX_STEPS: usize = 8; // guard against a non-terminating tool loop
    for _ in 0..MAX_STEPS {
        match llm.chat(&messages, &schemas).await? {
            LlmResponse::Message(text) => return Ok((text, produced)),
            LlmResponse::ToolCalls(calls) => {
                messages.push(LlmMessage::Assistant {
                    content: None,
                    tool_calls: calls.clone(),
                });
                for call in calls {
                    let ToolCall { id, name, arguments } = call;
                    let tool = tools
                        .iter()
                        .find(|t| t.schema().name == name)
                        .ok_or_else(|| AgentError::UnknownTool(name.clone()))?;
                    // A fatal `Err` aborts (`?`); a `Rejected` is fed back for a retry.
                    let content = match tool.call(arguments).await? {
                        ToolOutcome::Produced(value) => {
                            produced.insert(tool.target(), value.clone());
                            value.to_string()
                        }
                        // Do NOT record an artifact: the model must correct and call again.
                        ToolOutcome::Rejected { reason } => format!("REJECTED: {reason}"),
                    };
                    messages.push(LlmMessage::Tool {
                        tool_call_id: id,
                        content,
                    });
                }
            }
        }
    }
    Err(AgentError::Capability("tool-use loop exceeded max steps".into()))
}

// --- The agent contract, as a trait (one encoding of the morphism) ---------

#[async_trait]
pub trait SubAgent: Send + Sync {
    fn accepts(&self) -> &'static [PayloadKind];
    fn produces(&self) -> &'static [PayloadKind];
    async fn run(&self, input: AgentPayload) -> Result<AgentPayload, AgentError>;
}

/// Kleisli composition, threaded over `await`: the first failure short-circuits.
pub async fn run_pipeline(
    stages: &[&dyn SubAgent],
    input: AgentPayload,
) -> Result<AgentPayload, AgentError> {
    let mut acc = input;
    for stage in stages {
        acc = stage.run(acc).await?;
    }
    Ok(acc)
}

/// Graph-build-time wiring check: each stage must accept a kind the previous may produce.
pub fn validate_pipeline(stages: &[&dyn SubAgent]) -> Result<(), String> {
    for pair in stages.windows(2) {
        let (prev, next) = (pair[0], pair[1]);
        let compatible = prev.produces().iter().any(|k| next.accepts().contains(k));
        if !compatible {
            return Err(format!(
                "wiring error: a stage produces {:?} but the next accepts {:?}",
                prev.produces(),
                next.accepts()
            ));
        }
    }
    Ok(())
}

// --- Two example agents. They differ only by their TOOL SET. ----------------

/// An LLM that decides which of its *data* tools to call. Its tool set IS its isolation
/// boundary: it holds only data-access tools.
pub struct DataFetcher<L: LlmCapability> {
    pub llm: L,
    pub system: String,
    pub tools: Vec<Box<dyn Tool>>,
}

#[async_trait]
impl<L: LlmCapability> SubAgent for DataFetcher<L> {
    fn accepts(&self) -> &'static [PayloadKind] {
        &[PayloadKind::Initial]
    }
    fn produces(&self) -> &'static [PayloadKind] {
        &[PayloadKind::Intermediate]
    }
    async fn run(&self, input: AgentPayload) -> Result<AgentPayload, AgentError> {
        match input {
            AgentPayload::Initial(p) => {
                // The LLM chooses among self.tools; their results become the artifacts.
                let (_summary, artifacts) =
                    run_llm_loop(&self.llm, &self.system, &p.prompt, &self.tools).await?;
                Ok(AgentPayload::Intermediate(IntermediateData {
                    prompt: p.prompt,
                    artifacts,
                }))
            }
            other => Err(AgentError::Mismatch {
                expected: self.accepts(),
                got: other.kind(),
            }),
        }
    }
}

/// An LLM that writes a report from provided material. It exposes NO tools, so it cannot
/// fetch — there is simply nothing for its LLM to call. It reads only granted artifacts.
pub struct ReportWriter<L: LlmCapability> {
    pub llm: L,
    pub system: String,
}

#[async_trait]
impl<L: LlmCapability> SubAgent for ReportWriter<L> {
    fn accepts(&self) -> &'static [PayloadKind] {
        &[PayloadKind::Intermediate]
    }
    fn produces(&self) -> &'static [PayloadKind] {
        &[PayloadKind::Final]
    }
    async fn run(&self, input: AgentPayload) -> Result<AgentPayload, AgentError> {
        match input {
            AgentPayload::Intermediate(data) => {
                // Assemble context from granted artifacts ONLY, sorted for a deterministic
                // prompt (HashMap order is not stable).
                let mut entries: Vec<(&ArtifactKey, &ArtifactValue)> =
                    data.artifacts.iter().collect();
                entries.sort_by_key(|(k, _)| **k);
                let mut material = String::new();
                for (key, value) in entries {
                    material.push_str(&format!("[{key}] {value}\n"));
                }

                let user = format!("{}\n\nMaterial:\n{material}", data.prompt);
                let no_tools: Vec<Box<dyn Tool>> = Vec::new(); // the isolation boundary
                let (report, _) =
                    run_llm_loop(&self.llm, &self.system, &user, &no_tools).await?;
                Ok(AgentPayload::Final(FinalResult {
                    user: data.prompt,
                    assistant: report,
                }))
            }
            other => Err(AgentError::Mismatch {
                expected: self.accepts(),
                got: other.kind(),
            }),
        }
    }
}

// ===========================================================================
// ADAPTER (advisory) — LlmCapability implemented with async-openai
// Build with: `cargo build --features openai`. SDK types stay confined here.
// ===========================================================================

#[cfg(feature = "openai")]
pub mod openai_adapter {
    use super::*;
    use async_openai::config::OpenAIConfig;
    use async_openai::types::chat::{
        ChatCompletionMessageToolCall, ChatCompletionMessageToolCalls,
        ChatCompletionRequestAssistantMessageArgs, ChatCompletionRequestMessage,
        ChatCompletionRequestSystemMessageArgs, ChatCompletionRequestToolMessageArgs,
        ChatCompletionRequestUserMessageArgs, ChatCompletionTool, ChatCompletionTools,
        CreateChatCompletionRequestArgs, FunctionCall, FunctionObjectArgs,
    };
    use async_openai::Client;

    /// Implements the common LLM capability against the OpenAI chat-completions API.
    pub struct OpenAiLlm {
        pub client: Client<OpenAIConfig>,
        pub model: String,
    }

    fn cap(e: impl std::fmt::Display) -> AgentError {
        AgentError::Capability(e.to_string())
    }

    #[async_trait]
    impl LlmCapability for OpenAiLlm {
        async fn chat(
            &self,
            messages: &[LlmMessage],
            tools: &[ToolSchema],
        ) -> Result<LlmResponse, AgentError> {
            let oai_messages: Vec<ChatCompletionRequestMessage> =
                messages.iter().map(to_oai_message).collect::<Result<_, _>>()?;
            let oai_tools: Vec<ChatCompletionTools> =
                tools.iter().map(to_oai_tool).collect::<Result<_, _>>()?;

            let mut builder = CreateChatCompletionRequestArgs::default();
            builder.model(self.model.clone()).messages(oai_messages);
            if !oai_tools.is_empty() {
                builder.tools(oai_tools);
            }
            let request = builder.build().map_err(cap)?;

            let response = self.client.chat().create(request).await.map_err(cap)?;
            let message = response
                .choices
                .into_iter()
                .next()
                .ok_or_else(|| AgentError::Capability("no choices returned".into()))?
                .message;

            let tool_calls: Vec<ToolCall> = message
                .tool_calls
                .unwrap_or_default()
                .into_iter()
                .filter_map(|tc| match tc {
                    ChatCompletionMessageToolCalls::Function(f) => Some(ToolCall {
                        id: f.id,
                        name: f.function.name,
                        arguments: serde_json::from_str(&f.function.arguments)
                            .unwrap_or_else(|_| serde_json::json!({})),
                    }),
                    ChatCompletionMessageToolCalls::Custom(_) => None,
                })
                .collect();

            if tool_calls.is_empty() {
                Ok(LlmResponse::Message(message.content.unwrap_or_default()))
            } else {
                Ok(LlmResponse::ToolCalls(tool_calls))
            }
        }
    }

    fn to_oai_tool(t: &ToolSchema) -> Result<ChatCompletionTools, AgentError> {
        let function = FunctionObjectArgs::default()
            .name(t.name.clone())
            .description(t.description.clone())
            .parameters(t.parameters.clone())
            .build()
            .map_err(cap)?;
        Ok(ChatCompletionTools::Function(ChatCompletionTool { function }))
    }

    fn to_oai_message(m: &LlmMessage) -> Result<ChatCompletionRequestMessage, AgentError> {
        Ok(match m {
            LlmMessage::System(s) => ChatCompletionRequestSystemMessageArgs::default()
                .content(s.clone())
                .build()
                .map_err(cap)?
                .into(),
            LlmMessage::User(s) => ChatCompletionRequestUserMessageArgs::default()
                .content(s.clone())
                .build()
                .map_err(cap)?
                .into(),
            LlmMessage::Tool { tool_call_id, content } => {
                ChatCompletionRequestToolMessageArgs::default()
                    .tool_call_id(tool_call_id.clone())
                    .content(content.clone())
                    .build()
                    .map_err(cap)?
                    .into()
            }
            LlmMessage::Assistant { content, tool_calls } => {
                let mut builder = ChatCompletionRequestAssistantMessageArgs::default();
                if let Some(c) = content {
                    builder.content(c.clone());
                }
                if !tool_calls.is_empty() {
                    let tcs: Vec<ChatCompletionMessageToolCalls> = tool_calls
                        .iter()
                        .map(|c| {
                            ChatCompletionMessageToolCalls::Function(
                                ChatCompletionMessageToolCall {
                                    id: c.id.clone(),
                                    function: FunctionCall {
                                        name: c.name.clone(),
                                        arguments: c.arguments.to_string(),
                                    },
                                },
                            )
                        })
                        .collect();
                    builder.tool_calls(tcs);
                }
                builder.build().map_err(cap)?.into()
            }
        })
    }
}

// ===========================================================================
// TESTS — capabilities are mocked, so each agent is a pure async unit
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// An LLM whose responses are scripted, so agents become deterministic units.
    struct ScriptedLlm {
        queue: Mutex<Vec<LlmResponse>>,
    }
    impl ScriptedLlm {
        fn new(responses: Vec<LlmResponse>) -> Self {
            Self { queue: Mutex::new(responses) }
        }
    }
    #[async_trait]
    impl LlmCapability for ScriptedLlm {
        async fn chat(
            &self,
            _messages: &[LlmMessage],
            _tools: &[ToolSchema],
        ) -> Result<LlmResponse, AgentError> {
            let mut q = self.queue.lock().unwrap();
            if q.is_empty() {
                Ok(LlmResponse::Message(String::new()))
            } else {
                Ok(q.remove(0))
            }
        }
    }

    struct FakeFetchTool;
    #[async_trait]
    impl Tool for FakeFetchTool {
        fn schema(&self) -> ToolSchema {
            ToolSchema {
                name: "query_records".into(),
                description: "fetch records".into(),
                parameters: serde_json::json!({ "type": "object", "properties": {} }),
            }
        }
        fn target(&self) -> ArtifactKey {
            ArtifactKey::FetcherRecords
        }
        async fn call(
            &self,
            _arguments: serde_json::Value,
        ) -> Result<ToolOutcome, AgentError> {
            Ok(ToolOutcome::Produced(ArtifactValue::Json(
                serde_json::json!([{ "id": 1 }, { "id": 2 }]),
            )))
        }
    }

    fn tool_call(name: &str) -> LlmResponse {
        LlmResponse::ToolCalls(vec![ToolCall {
            id: "c1".into(),
            name: name.into(),
            arguments: serde_json::json!({}),
        }])
    }

    #[tokio::test]
    async fn writer_produces_final_and_exposes_no_tools() {
        let writer = ReportWriter {
            llm: ScriptedLlm::new(vec![LlmResponse::Message("REPORT".into())]),
            system: "you write reports".into(),
        };
        let mut artifacts = HashMap::new();
        artifacts.insert(ArtifactKey::FetcherRecords, ArtifactValue::Text("rows".into()));
        let out = writer
            .run(AgentPayload::Intermediate(IntermediateData {
                prompt: "write it up".into(),
                artifacts,
            }))
            .await
            .unwrap();
        match out {
            AgentPayload::Final(f) => assert_eq!(f.assistant, "REPORT"),
            other => panic!("expected Final, got {:?}", other.kind()),
        }
    }

    #[tokio::test]
    async fn writer_falls_on_wrong_variant() {
        let writer = ReportWriter {
            llm: ScriptedLlm::new(vec![]),
            system: String::new(),
        };
        let out = writer
            .run(AgentPayload::Final(FinalResult {
                user: "u".into(),
                assistant: "a".into(),
            }))
            .await;
        assert!(matches!(
            out,
            Err(AgentError::Mismatch { got: PayloadKind::Final, .. })
        ));
    }

    #[tokio::test]
    async fn fetcher_lets_llm_pick_a_tool_then_finishes() {
        let fetcher = DataFetcher {
            llm: ScriptedLlm::new(vec![
                tool_call("query_records"),
                LlmResponse::Message("done".into()),
            ]),
            system: "fetch what is asked".into(),
            tools: vec![Box::new(FakeFetchTool)],
        };
        let out = fetcher
            .run(AgentPayload::Initial(InitialPrompt {
                prompt: "get the records".into(),
                history: vec![],
            }))
            .await
            .unwrap();
        match out {
            AgentPayload::Intermediate(d) => {
                assert!(d.artifacts.contains_key(&ArtifactKey::FetcherRecords));
            }
            other => panic!("expected Intermediate, got {:?}", other.kind()),
        }
    }

    #[tokio::test]
    async fn tool_call_outside_the_agents_set_is_rejected() {
        // The scripted LLM asks for a tool the fetcher was never given.
        let fetcher = DataFetcher {
            llm: ScriptedLlm::new(vec![tool_call("delete_everything")]),
            system: String::new(),
            tools: vec![Box::new(FakeFetchTool)],
        };
        let out = fetcher
            .run(AgentPayload::Initial(InitialPrompt {
                prompt: "x".into(),
                history: vec![],
            }))
            .await;
        assert!(matches!(out, Err(AgentError::UnknownTool(_))));
    }

    #[tokio::test]
    async fn fetcher_then_writer_pipeline() {
        let fetcher = DataFetcher {
            llm: ScriptedLlm::new(vec![
                tool_call("query_records"),
                LlmResponse::Message("fetched".into()),
            ]),
            system: "fetch".into(),
            tools: vec![Box::new(FakeFetchTool)],
        };
        let writer = ReportWriter {
            llm: ScriptedLlm::new(vec![LlmResponse::Message("SUMMARY".into())]),
            system: "write".into(),
        };
        let stages: [&dyn SubAgent; 2] = [&fetcher, &writer];

        assert!(validate_pipeline(&stages).is_ok());

        let out = run_pipeline(
            &stages,
            AgentPayload::Initial(InitialPrompt {
                prompt: "get and write".into(),
                history: vec![],
            }),
        )
        .await
        .unwrap();
        assert!(matches!(out, AgentPayload::Final(_)));
    }

    #[test]
    fn payload_round_trips_through_json() {
        let mut artifacts = HashMap::new();
        artifacts.insert(
            ArtifactKey::FetcherRecords,
            ArtifactValue::Json(serde_json::json!({ "n": 1 })),
        );
        artifacts.insert(ArtifactKey::FetcherSchema, ArtifactValue::Text("id:int".into()));
        let payload = AgentPayload::Intermediate(IntermediateData {
            prompt: "p".into(),
            artifacts,
        });
        let json = serde_json::to_string(&payload).unwrap();
        let back: AgentPayload = serde_json::from_str(&json).unwrap();
        assert_eq!(payload, back);
    }
}
