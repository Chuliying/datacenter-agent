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

//! The payload contract: the value that flows between sub-agents, and the abstract capabilities
//! an agent is built from.
//!
//! Port of PART A (normative) plus the LLM/tool capability abstractions and the tool-use loop.
//!
//! The concrete async-openai implementation of [`LlmCapability`] is **not** here — it lives in
//! [`crate::agent::llm`], built against the async-openai already in the tree.
//! Keeping it out of this module leaves the payload layer SDK-agnostic and unit-testable with a
//! scripted LLM.
//!
//! Everything in PART A is binding: any code that consumes or produces a payload must speak
//! exactly these types.
//!
//! # References
//!
//! - Payload contract, PART A — `.spec/contract/agent_payload/agent_payload.rs`

#![allow(dead_code)] // groundwork: not every artifact key / value variant is wired yet.

use async_trait::async_trait;
use chrono::{DateTime, FixedOffset};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt;

// ===========================================================================
// Data model (normative): the value every agent consumes and produces
// ===========================================================================

/// One user/assistant turn, used inside [`InitialPrompt::history`].
///
/// Structurally this is a `FinalResult` without metadata, but it is a *distinct* type on
/// purpose — the two are free to evolve independently.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Exchange {
    pub user: String,
    pub assistant: String,
}

/// A value carried in the artifact map.
///
/// A *closed* enum. `Display` is the "cast to string if you don't care about the type" view;
/// a `match` is the checked "downcast".
///
/// Closedness is what lets the whole payload derive `Clone` + `Serialize`.
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

/// An **open, producer-namespaced** artifact key: `{agent}.{name}`.
///
/// Any agent freely names its outputs — there is no closed set to extend — so a tool result, an
/// agent's own message, or any computed value can all be keyed uniformly. The `agent` half is the
/// producer namespace (contract §2.5); the canonical dotted string (`fetcher.records`,
/// `analyst.message`) is *both* the log form and the serialized form (JSON object keys are
/// strings).
///
/// Prefer the named constructors ([`fetcher_records`](Self::fetcher_records),
/// [`charts_spec`](Self::charts_spec), [`message`](Self::message)) for the well-known wires so the
/// vocabulary stays in one place; use [`new`](Self::new) for anything else.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Serialize, Deserialize)]
#[serde(into = "String", try_from = "String")]
pub struct ArtifactKey {
    agent: String,
    name: String,
}

impl ArtifactKey {
    /// Builds a key `{agent}.{name}` from an arbitrary producer + slot.
    pub fn new(agent: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            agent: agent.into(),
            name: name.into(),
        }
    }

    /// The artifact holding an agent's own **message** — the prose its reasoning produced, keyed
    /// `{agent}.message`. Captured for every LLM stage so an `Intermediate` boundary never drops
    /// it (the analyst's report is exactly this).
    pub fn message(agent: &str) -> Self {
        Self::new(agent, "message")
    }

    /// The `fetcher`'s data records wire (`fetcher.records`).
    pub fn fetcher_records() -> Self {
        Self::new("fetcher", "records")
    }

    /// The `fetcher`'s schema wire (`fetcher.schema`).
    pub fn fetcher_schema() -> Self {
        Self::new("fetcher", "schema")
    }

    /// The `charter`'s schema-validated chart batch (`charts.spec`), a serialized
    /// [`ChartBatch`](crate::agent::chart::ChartBatch) from the `emit_chart` sink.
    pub fn charts_spec() -> Self {
        Self::new("charts", "spec")
    }

    /// The `composer`'s schema-validated report payload (`report.data`), a serialized
    /// [`ReportData`](crate::agent::report::ReportData) from the `emit_report` sink — the single
    /// artifact the `renderer` injects into the HTML template.
    pub fn report_data() -> Self {
        Self::new("report", "data")
    }

    /// The producer namespace (the `agent` half).
    pub fn agent(&self) -> &str {
        &self.agent
    }

    /// The slot name (the `name` half).
    pub fn name(&self) -> &str {
        &self.name
    }
}

impl fmt::Display for ArtifactKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}", self.agent, self.name)
    }
}

impl std::str::FromStr for ArtifactKey {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        // Split on the *first* dot: the producer namespace cannot contain one, the slot name may.
        match s.split_once('.') {
            Some((agent, name)) if !agent.is_empty() && !name.is_empty() => {
                Ok(Self::new(agent, name))
            }
            _ => Err(format!("artifact key must be `agent.name`, got: {s:?}")),
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

/// Pipeline entry: the user's request plus prior turns.
///
/// No system prompt — each agent carries its own designed instruction.
/// `history` is a plain `Vec` (no `Option`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct InitialPrompt {
    pub prompt: String,
    pub history: Vec<Exchange>,
    /// The wall-clock instant this turn began, in an explicit offset (Asia/Taipei `+08:00` by
    /// default).
    ///
    /// Stamped **once at the boundary** and threaded as data — never read ambiently inside a stage
    /// — so every stage shares one consistent `now`, an eval fixture can pin it for
    /// reproducibility, and the value is auditable end to end. Each LLM stage renders it into a
    /// `# Current Time` header ([`current_time_header`](crate::agent::clock::current_time_header))
    /// so the model can tell an in-progress trailing period from a genuine drop.
    ///
    /// This is the seed of a broader per-turn context (request id, locale, tenant) — extend here.
    pub now: DateTime<FixedOffset>,
}

/// Intermediate working data — never the user-facing output.
///
/// `prompt` carries the instruction forward.
/// `artifacts` is the KV surface the orchestrator wires between agents.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct IntermediateData {
    pub prompt: String,
    pub artifacts: HashMap<ArtifactKey, ArtifactValue>,
    /// The turn's timestamp, carried forward unchanged from [`InitialPrompt::now`] so every stage
    /// agrees on `now` (see there).
    pub now: DateTime<FixedOffset>,
}

/// The user-facing result.
///
/// Its own type (not `Exchange`) so metadata can land here later.
///
/// `assistant` is the terminal *projection* (what the user sees); `artifacts` carries the full,
/// lossless provenance the pipeline accumulated — every stage's tool outputs and messages — so the
/// result is auditable end to end and a caller can inspect what produced the answer. (No `Eq`:
/// [`ArtifactValue::Number`] holds an `f64`.)
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FinalResult {
    pub user: String,
    pub assistant: String,
    /// The turn's timestamp (see [`InitialPrompt::now`]), stamped onto the terminal result so an
    /// audit record of the response carries when the turn occurred.
    pub now: DateTime<FixedOffset>,
    /// The accumulated artifact map — full provenance, carried through the terminal boundary
    /// rather than dropped. The producer decides what (if anything) a caller-facing view exposes.
    pub artifacts: HashMap<ArtifactKey, ArtifactValue>,
    // EXTEND: stop_reason, token usage, latency, ...
}

/// The value every agent consumes and produces.
///
/// This sum type IS the contract.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum AgentPayload {
    Initial(InitialPrompt),
    Intermediate(IntermediateData),
    Final(FinalResult),
}

/// A cheap tag for a payload's variant.
///
/// Lets acceptance checks and mismatch errors avoid carrying a whole payload.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PayloadKind {
    Initial,
    Intermediate,
    Final,
}

impl AgentPayload {
    /// The [`PayloadKind`] tag of this payload.
    pub fn kind(&self) -> PayloadKind {
        match self {
            AgentPayload::Initial(_) => PayloadKind::Initial,
            AgentPayload::Intermediate(_) => PayloadKind::Intermediate,
            AgentPayload::Final(_) => PayloadKind::Final,
        }
    }
}

/// The shared error type every agent fails *into*.
///
/// Part of the contract: because all agents converge on this type, an orchestrator can route or
/// log failures uniformly.
/// `Mismatch` is the "falling convention" — being handed a variant an agent does not accept is
/// an error value, never a panic.
///
/// # References
///
/// - Payload contract — the falling convention
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
                write!(
                    f,
                    "payload mismatch: expected one of {expected:?}, got {got:?}"
                )
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
// obeying the behavioral rules above. The `SubAgent` trait in `engine` is one *encoding* of
// this; it is advisory. The morphism, the falling convention, and the isolation semantics
// are what bind.

// ===========================================================================
// Capabilities (advisory shapes): the LLM transport and the tool
// ===========================================================================

/// A message in the chat protocol — minimal but sufficient for tool use.
///
/// Maps directly onto async-openai's message types (see [`crate::agent::llm`]).
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

/// The capability common to every agent: talk to an LLM.
///
/// `async` to fit async-openai's client.
#[async_trait]
pub trait LlmCapability: Send + Sync {
    /// Performs one chat round-trip: sends `messages` with `tools` advertised and returns the
    /// model's reply.
    async fn chat(
        &self,
        messages: &[LlmMessage],
        tools: &[ToolSchema],
    ) -> Result<LlmResponse, AgentError>;
}

/// The result of one tool invocation.
///
/// **`Rejected` is not an error.** It is a retryable, model-facing outcome (bad arguments,
/// failed validation), distinct from a fatal `Err(AgentError)` (transport / wiring).
///
/// [`run_llm_loop`] feeds a `Rejected { reason }` back to the model so it can correct, and does
/// **not** record an artifact for it.
/// This is what makes a validating / sink tool "loop until valid" for free.
///
/// # References
///
/// - Tool contract — the `Rejected` retry model
#[derive(Clone, Debug)]
pub enum ToolOutcome {
    /// The call succeeded; the value fills the tool's `target` artifact slot.
    Produced(ArtifactValue),
    /// The call was rejected (e.g. schema validation failed). Fed back to the model as a
    /// tool message; no artifact is recorded. Retryable within the loop's step cap.
    Rejected { reason: String },
}

/// A tool the LLM may choose to call.
///
/// A **named capability the LLM invokes, whose result fills an artifact slot** (`target`).
/// This spans data fetches (MCP), output *sinks* that validate the model's own structured
/// output, and pure *validators / compute* (a calculator).
///
/// `target` is wired by the orchestration designer, never chosen by the LLM.
#[async_trait]
pub trait Tool: Send + Sync {
    /// The schema advertised to the model for this tool.
    fn schema(&self) -> ToolSchema;
    /// The artifact slot a successful call fills.
    fn target(&self) -> ArtifactKey;
    /// Invokes the tool.
    ///
    /// `Ok(Produced)` fills `target`; `Ok(Rejected)` is fed back for a retry; `Err` is fatal.
    async fn call(&self, arguments: serde_json::Value) -> Result<ToolOutcome, AgentError>;
}

/// Drives the model's tool-use loop until it returns a final message.
///
/// The model sees `tools` and decides which to call.
/// Each call is dispatched to the matching [`Tool`], its result is fed back, and the loop
/// repeats until the model stops calling tools.
/// Results are collected into the produced map, keyed by each tool's `target`.
///
/// A call to a tool this agent does not own is rejected — an agent can only ever run tools in
/// its own set.
/// This is the isolation boundary, guarded at dispatch.
///
/// # Arguments
///
/// - `llm`: the chat capability the loop talks to.
/// - `system`: the agent's system instruction.
/// - `user`: the assembled user turn (prompt plus any carried material).
/// - `tools`: the agent's granted tools — the only ones the model may call.
///
/// # Returns
///
/// Returns `Ok((text, produced))` — the model's final message and the artifacts each tool
/// produced, keyed by `target`.
///
/// # Errors
///
/// - [`AgentError::UnknownTool`] — the model called a tool outside this agent's set.
/// - [`AgentError::Capability`] — the LLM transport failed, a tool failed fatally, or the loop
///   exceeded its step cap (`MAX_STEPS`).
///
/// # References
///
/// - Payload contract §2.3 — tool isolation guarded at dispatch
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
                    let ToolCall {
                        id,
                        name,
                        arguments,
                    } = call;
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
    Err(AgentError::Capability(
        "tool-use loop exceeded max steps".into(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// An LLM whose responses are scripted, so the loop becomes a deterministic unit.
    struct ScriptedLlm {
        turns: Mutex<Vec<LlmResponse>>,
    }

    #[async_trait]
    impl LlmCapability for ScriptedLlm {
        async fn chat(
            &self,
            _messages: &[LlmMessage],
            _tools: &[ToolSchema],
        ) -> Result<LlmResponse, AgentError> {
            Ok(self.turns.lock().unwrap().remove(0))
        }
    }

    struct EchoTool;
    #[async_trait]
    impl Tool for EchoTool {
        fn schema(&self) -> ToolSchema {
            ToolSchema {
                name: "echo".into(),
                description: "echo".into(),
                parameters: serde_json::json!({ "type": "object", "properties": {} }),
            }
        }
        fn target(&self) -> ArtifactKey {
            ArtifactKey::fetcher_records()
        }
        async fn call(&self, _args: serde_json::Value) -> Result<ToolOutcome, AgentError> {
            Ok(ToolOutcome::Produced(ArtifactValue::Text("echoed".into())))
        }
    }

    #[test]
    fn artifact_key_round_trips_through_its_dotted_string() {
        assert_eq!(
            ArtifactKey::fetcher_records().to_string(),
            "fetcher.records"
        );
        assert_eq!(
            "fetcher.records".parse::<ArtifactKey>().unwrap(),
            ArtifactKey::fetcher_records()
        );
        // an arbitrary `agent.name` is valid now — the key space is open…
        assert_eq!(
            "custom.slot".parse::<ArtifactKey>().unwrap(),
            ArtifactKey::new("custom", "slot")
        );
        // …but a string with no namespace dot is not a key.
        assert!("nodot".parse::<ArtifactKey>().is_err());
    }

    #[tokio::test]
    async fn loop_dispatches_a_tool_then_returns_the_final_message() {
        let tools: Vec<Box<dyn Tool>> = vec![Box::new(EchoTool)];
        let llm = ScriptedLlm {
            turns: Mutex::new(vec![
                LlmResponse::ToolCalls(vec![ToolCall {
                    id: "c1".into(),
                    name: "echo".into(),
                    arguments: serde_json::json!({}),
                }]),
                LlmResponse::Message("done".into()),
            ]),
        };
        let (text, produced) = run_llm_loop(&llm, "sys", "go", &tools).await.unwrap();
        assert_eq!(text, "done");
        assert_eq!(
            produced.get(&ArtifactKey::fetcher_records()),
            Some(&ArtifactValue::Text("echoed".into()))
        );
    }

    #[tokio::test]
    async fn calling_a_tool_outside_the_set_is_rejected_at_dispatch() {
        let tools: Vec<Box<dyn Tool>> = vec![Box::new(EchoTool)];
        let llm = ScriptedLlm {
            turns: Mutex::new(vec![LlmResponse::ToolCalls(vec![ToolCall {
                id: "c1".into(),
                name: "not_granted".into(),
                arguments: serde_json::json!({}),
            }])]),
        };
        let err = run_llm_loop(&llm, "sys", "go", &tools).await.unwrap_err();
        assert!(matches!(err, AgentError::UnknownTool(name) if name == "not_granted"));
    }
}
