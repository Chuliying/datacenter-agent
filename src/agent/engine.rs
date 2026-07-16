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

//! The engine (PART B): the [`SubAgent`] trait unifying config-defined and code-defined agents,
//! the generic [`ConfiguredAgent`], a code-defined [`HelloWorld`] template, and the
//! multi-pipeline [`Orchestrator`].
//!
//! Port of the sub-agent contract, PART B.
//!
//! A sub-agent is up to three optional components — **LLM**, **Tools**, **Logic** — behind one
//! trait.
//! [`ConfiguredAgent`]'s Logic is the built-in LLM tool-loop; [`HelloWorld`]'s is arbitrary
//! Rust.
//! The [`Orchestrator`] threads a payload through a selected pipeline and cannot tell which
//! provenance a stage came from.
//!
//! # References
//!
//! - Sub-agent contract, PART B — `.spec/contract/sub_agent/sub_agent.rs`

#![allow(dead_code)] // groundwork: the orchestrator is not wired behind AgentPort yet.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use async_trait::async_trait;

use crate::agent::clock::current_time_header;
use crate::agent::config::{
    OutputShape, PipelineConfig, PipelineId, ResolveError, SubAgentConfig, SubAgentId,
};
use crate::agent::events::{AgentEvent, EventSink, StageOutcome};
use crate::agent::payload::{
    run_llm_loop, AgentError, AgentPayload, ArtifactKey, ArtifactValue, FinalResult,
    IntermediateData, LlmCapability, LlmMessage, LlmResponse, PayloadKind, Tool, ToolSchema,
};

/// The sub-agent abstraction of this contract: a self-checking morphism, and the **single seam
/// that unifies config-defined and code-defined agents**.
///
/// [`ConfiguredAgent`] implements it from a [`SubAgentConfig`]; a hand-written type like
/// [`HelloWorld`] implements it directly.
/// The `run` method *is* the Logic component — the built-in loop for the former, arbitrary code
/// for the latter.
///
/// It deliberately omits any static `produces` — composition safety is the runtime falling
/// convention, not a static graph.
///
/// # References
///
/// - Sub-agent contract §1.1 — the unifying seam
/// - Sub-agent contract §2.4 — the falling convention over a static graph
#[async_trait]
pub trait SubAgent: Send + Sync {
    /// This agent's stable identity.
    fn id(&self) -> &SubAgentId;
    /// The payload variants this agent consumes; anything else falls via the mismatch check.
    fn accepts(&self) -> &'static [PayloadKind];
    /// Runs the agent as a self-checking morphism `AgentPayload -> Result<AgentPayload, _>`.
    async fn run(&self, input: AgentPayload) -> Result<AgentPayload, AgentError>;
}

// So a `ConfiguredAgent` can hold a type-erased LLM (`Arc<dyn LlmCapability>`) yet still call
// `run_llm_loop`, whose type parameter is `Sized`. The trait is local, so this impl is legal.
#[async_trait]
impl LlmCapability for Arc<dyn LlmCapability> {
    async fn chat(
        &self,
        messages: &[LlmMessage],
        tools: &[ToolSchema],
    ) -> Result<LlmResponse, AgentError> {
        (**self).chat(messages, tools).await
    }
}

/// Interns a runtime accept-set into a `'static` slice.
///
/// There are only 2³ subsets of the three [`PayloadKind`]s, so each maps to a fixed `'static`
/// slice.
/// This lets a config-driven agent satisfy the payload contract's `&'static` `Mismatch` without
/// leaking.
///
/// # Arguments
///
/// - `kinds`: the runtime accept-set to intern.
///
/// # Returns
///
/// Returns the matching `'static` slice, or an empty slice if `kinds` is empty.
fn intern_accepts(kinds: &[PayloadKind]) -> &'static [PayloadKind] {
    use PayloadKind::{Final, Initial, Intermediate};
    let bit = |k: &PayloadKind| match k {
        Initial => 0b001u8,
        Intermediate => 0b010,
        Final => 0b100,
    };
    match kinds.iter().fold(0u8, |m, k| m | bit(k)) {
        0b001 => &[Initial],
        0b010 => &[Intermediate],
        0b100 => &[Final],
        0b011 => &[Initial, Intermediate],
        0b101 => &[Initial, Final],
        0b110 => &[Intermediate, Final],
        0b111 => &[Initial, Intermediate, Final],
        _ => &[],
    }
}

/// The generic engine: the default [`SubAgent`], driven entirely by a [`SubAgentConfig`].
///
/// This *is* "the sub-agent is abstract, config drives it".
/// The **fetcher** and the report **writer** are both this type — they differ only in their
/// instruction, grant, and output.
pub struct ConfiguredAgent {
    id: SubAgentId,
    instruction: String,
    llm: Arc<dyn LlmCapability>,
    tools: Vec<Box<dyn Tool>>,
    accepts: &'static [PayloadKind],
    output: OutputShape,
    /// Whether to capture this stage's model message as the `{id}.message` artifact
    /// ([`SubAgentConfig::capture_message`](crate::agent::config::SubAgentConfig::capture_message)).
    capture_message: bool,
}

impl ConfiguredAgent {
    /// Assembles a runnable agent from resolved parts.
    ///
    /// The LLM capability is injected (built from a
    /// [`ResolvedLlm`](crate::agent::config::ResolvedLlm) by a factory, elsewhere), which is what
    /// makes the agent a **pure async function of its payload** in tests — a scripted LLM and
    /// mock tools, no config and no network.
    ///
    /// # Arguments
    ///
    /// - `cfg`: the authored config supplying id, instruction, and accept-set.
    /// - `llm`: the injected, type-erased chat capability.
    /// - `tools`: the resolved granted tools (already built from the config's grant).
    /// - `output`: the **resolved** output shape — the config's explicit value, or
    ///   [`effective_output`](crate::agent::config::effective_output)'s position-derived default.
    ///   Never `cfg.output` read directly, since the default can only be computed once every
    ///   pipeline referencing this agent is known.
    pub fn new(
        cfg: &SubAgentConfig,
        llm: Arc<dyn LlmCapability>,
        tools: Vec<Box<dyn Tool>>,
        output: OutputShape,
    ) -> Self {
        Self {
            id: cfg.id.clone(),
            instruction: cfg.instruction.clone(),
            llm,
            tools,
            accepts: intern_accepts(&cfg.accepts),
            output,
            capture_message: cfg.capture_message,
        }
    }

    /// Renders granted artifacts into a deterministic material block.
    ///
    /// `HashMap` iteration order is not stable, so entries are sorted by key.
    /// This gives a downstream stage its inputs in a stable order.
    ///
    /// # Arguments
    ///
    /// - `artifacts`: the carried-forward artifact map to render.
    ///
    /// # Returns
    ///
    /// Returns the rendered block, one `[key] value` line per artifact in key order.
    fn render_material(artifacts: &HashMap<ArtifactKey, ArtifactValue>) -> String {
        let mut entries: Vec<(&ArtifactKey, &ArtifactValue)> = artifacts.iter().collect();
        entries.sort_by_key(|(a, _)| *a); // ArtifactKey: Ord (no longer Copy)
        entries
            .iter()
            .map(|(k, v)| format!("[{k}] {v}\n"))
            .collect()
    }
}

#[async_trait]
impl SubAgent for ConfiguredAgent {
    fn id(&self) -> &SubAgentId {
        &self.id
    }

    fn accepts(&self) -> &'static [PayloadKind] {
        self.accepts
    }

    async fn run(&self, input: AgentPayload) -> Result<AgentPayload, AgentError> {
        // §2.4 self-check: fall on a variant we do not accept. Never panic.
        if !self.accepts.contains(&input.kind()) {
            return Err(AgentError::Mismatch {
                expected: self.accepts,
                got: input.kind(),
            });
        }

        // Assemble the user turn + carry-forward artifacts (append-only, payload §2.4). `now` is
        // turn data, threaded through unchanged so every stage shares one timestamp (payload B).
        let (prompt, incoming, now) = match input {
            AgentPayload::Initial(p) => (p.prompt, HashMap::new(), p.now),
            AgentPayload::Intermediate(d) => (d.prompt, d.artifacts, d.now),
            // Excluded by the accept-check above unless an agent explicitly accepts Final.
            AgentPayload::Final(f) => (f.user, HashMap::new(), f.now),
        };
        let material = Self::render_material(&incoming);
        let user = if material.is_empty() {
            prompt.clone()
        } else {
            format!("{prompt}\n\nMaterial:\n{material}")
        };

        // Make the stage time-aware: prepend a `# Current Time` header from the turn's `now` so the
        // model can tell an in-progress trailing period (e.g. the current month) from a genuine
        // drop. Deterministic — `now` came in with the payload, not from an ambient clock.
        let system = format!("{}{}", current_time_header(&now), self.instruction);

        // The LLM chooses among *only* the granted tools; out-of-set calls are rejected at
        // dispatch inside the loop (payload §2.3).
        let (text, produced) = run_llm_loop(&self.llm, &system, &user, &self.tools).await?;

        // Assemble this stage's full output: everything carried in, plus every tool artifact, plus
        // — when `capture_message` is set — the stage's own **message** as a first-class artifact
        // keyed `{id}.message`. A captured message is never dropped: an Intermediate carries it
        // forward and a Final keeps it as provenance (open-key contract). A tool-only stage leaves
        // it off so its throwaway note doesn't clutter the map.
        let mut artifacts = incoming;
        artifacts.extend(produced); // append-only merge
        if self.capture_message {
            artifacts.insert(
                ArtifactKey::message(&self.id.0),
                ArtifactValue::Text(text.clone()),
            );
        }

        match self.output {
            OutputShape::Intermediate => Ok(AgentPayload::Intermediate(IntermediateData {
                prompt,
                artifacts,
                now,
            })),
            // The message is *also* surfaced as the user-facing `assistant`; the artifact map rides
            // along as provenance (payload B / open-key auditability).
            OutputShape::Final => Ok(AgentPayload::Final(FinalResult {
                user: prompt,
                assistant: text,
                now,
                artifacts,
            })),
        }
    }
}

/// A hand-written [`SubAgent`] with **no LLM and no tools** — its entire behaviour is Logic.
///
/// Whatever `Initial` prompt it is handed, it returns a fixed `Final`.
/// It cannot be expressed as a [`SubAgentConfig`] (there is no prompt and no model to configure),
/// yet it is the *same* abstract [`SubAgent`] and drops into any pipeline beside config-defined
/// agents.
///
/// A real Logic-only agent (e.g. a session-memory keeper that queries a store and emits an
/// artifact) has exactly this shape with a non-trivial `run`.
///
/// # References
///
/// - Sub-agent contract §1.1 — Logic-only agents
pub struct HelloWorld {
    id: SubAgentId,
}

impl HelloWorld {
    /// Builds a [`HelloWorld`] with the given identity.
    pub fn new(id: SubAgentId) -> Self {
        Self { id }
    }
}

#[async_trait]
impl SubAgent for HelloWorld {
    fn id(&self) -> &SubAgentId {
        &self.id
    }

    fn accepts(&self) -> &'static [PayloadKind] {
        &[PayloadKind::Initial]
    }

    async fn run(&self, input: AgentPayload) -> Result<AgentPayload, AgentError> {
        // Accept-check and destructure in one: only Initial is accepted, and its `now` is threaded
        // onto the terminal result so the response carries the turn's timestamp.
        let (user, now) = match input {
            AgentPayload::Initial(p) => (p.prompt, p.now),
            other => {
                return Err(AgentError::Mismatch {
                    expected: self.accepts(),
                    got: other.kind(),
                })
            }
        };
        Ok(AgentPayload::Final(FinalResult {
            user,
            assistant: "hello world.".to_string(),
            now,
            artifacts: HashMap::new(), // a logic-only agent produces no artifacts
        }))
    }
}

/// Resolves a pipeline's stage references against the built agents, **failing fast** on an
/// unknown reference.
///
/// # Arguments
///
/// - `cfg`: the pipeline whose stage ids are being resolved.
/// - `agents`: the built agents, keyed by id.
///
/// # Returns
///
/// Returns the ordered, runnable stages.
///
/// # Errors
///
/// - [`ResolveError::UnknownAgentRef`] — a stage names an agent id that was never built.
///
/// # References
///
/// - Sub-agent contract §1.4 — unknown stage references fail fast
pub fn resolve_pipeline(
    cfg: &PipelineConfig,
    agents: &HashMap<SubAgentId, Arc<dyn SubAgent>>,
) -> Result<Vec<Arc<dyn SubAgent>>, ResolveError> {
    cfg.stages
        .iter()
        .map(|id| {
            agents
                .get(id)
                .cloned()
                .ok_or_else(|| ResolveError::UnknownAgentRef {
                    pipeline: cfg.id.clone(),
                    agent: id.clone(),
                })
        })
        .collect()
}

/// Holds every resolved pipeline and runs a selected one.
///
/// Kleisli composition: the first stage that falls short-circuits the rest (`?`).
#[derive(Default)]
pub struct Orchestrator {
    pipelines: HashMap<PipelineId, Vec<Arc<dyn SubAgent>>>,
}

impl Orchestrator {
    /// Creates an empty orchestrator.
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers a resolved pipeline under its id, returning `&mut self` for chaining.
    pub fn insert(&mut self, id: PipelineId, stages: Vec<Arc<dyn SubAgent>>) -> &mut Self {
        self.pipelines.insert(id, stages);
        self
    }

    /// Runs the pipeline named `id`, threading the payload through each stage.
    ///
    /// A stage mismatch surfaces as a typed [`AgentError`].
    /// An unknown pipeline id is a caller error.
    ///
    /// # Arguments
    ///
    /// - `id`: the pipeline to run.
    /// - `input`: the initial payload fed to the first stage.
    ///
    /// # Returns
    ///
    /// Returns the final stage's payload.
    ///
    /// # Errors
    ///
    /// - [`AgentError::Mismatch`] — a stage was handed a variant it does not accept.
    /// - [`AgentError::Capability`] — `id` names no registered pipeline, or a stage's capability
    ///   failed.
    ///
    /// # References
    ///
    /// - Sub-agent contract §2.4 — stage mismatch as a typed error
    pub async fn run(
        &self,
        id: &PipelineId,
        input: AgentPayload,
    ) -> Result<AgentPayload, AgentError> {
        let stages = self
            .pipelines
            .get(id)
            .ok_or_else(|| AgentError::Capability(format!("unknown pipeline: {id}")))?;
        let mut acc = input;
        for stage in stages {
            acc = stage.run(acc).await?;
        }
        Ok(acc)
    }

    /// Like [`run`](Self::run) but emits [`AgentEvent`]s for each stage boundary onto `sink` — the
    /// streaming path (plan §8.5, mechanism A).
    ///
    /// Stage-level framing (`StageStarted` / `StageProduced` / `StageFinished`, then a terminal
    /// `Finished` / `Error`) is emitted here, **outside** any [`SubAgent::run`], so the normative
    /// `run(payload) -> Result<payload>` morphism is unchanged. The finer-grained token and tool
    /// events come from the stage's own injected capabilities (a
    /// [`StreamingOpenAiLlm`](crate::agent::llm::StreamingOpenAiLlm) and
    /// [`StreamingTool`](crate::agent::tools::StreamingTool) sharing this same `sink`).
    ///
    /// # Arguments
    ///
    /// - `id`: the pipeline to run.
    /// - `input`: the initial payload fed to the first stage.
    /// - `sink`: the per-turn event sink (the same one the stage capabilities were wired with).
    ///
    /// # Returns
    ///
    /// Returns the final stage's payload (identical to [`run`](Self::run)).
    ///
    /// # Errors
    ///
    /// - [`AgentError::Mismatch`] — a stage was handed a variant it does not accept (also emitted
    ///   as an `Error` event before returning).
    /// - [`AgentError::Capability`] — `id` names no registered pipeline, or a stage failed.
    pub async fn run_emitting(
        &self,
        id: &PipelineId,
        input: AgentPayload,
        sink: &dyn EventSink,
    ) -> Result<AgentPayload, AgentError> {
        let stages = self
            .pipelines
            .get(id)
            .ok_or_else(|| AgentError::Capability(format!("unknown pipeline: {id}")))?;
        let mut acc = input;
        for stage in stages {
            let agent = stage.id().clone();
            // Snapshot the artifact keys before the stage, so `StageProduced` reports only what
            // this stage added (not what it carried forward).
            let before: HashSet<ArtifactKey> = match &acc {
                AgentPayload::Intermediate(d) => d.artifacts.keys().cloned().collect(),
                _ => HashSet::new(),
            };
            sink.emit(AgentEvent::StageStarted {
                agent: agent.clone(),
                input: acc.kind(),
            });

            acc = match stage.run(acc).await {
                Ok(next) => next,
                Err(e) => {
                    // Mark the failed stage (a red dot) before the terminal error, so a UI can
                    // close out its indicator with a failure rather than leaving it spinning.
                    sink.emit(AgentEvent::StageFinished {
                        agent: agent.clone(),
                        outcome: StageOutcome::Failure,
                    });
                    sink.emit(AgentEvent::Error {
                        message: e.to_string(),
                    });
                    return Err(e);
                }
            };

            if let AgentPayload::Intermediate(d) = &acc {
                let mut keys: Vec<ArtifactKey> = d
                    .artifacts
                    .keys()
                    .filter(|k| !before.contains(*k))
                    .cloned()
                    .collect();
                keys.sort(); // deterministic order (HashMap iteration is not stable)
                if !keys.is_empty() {
                    sink.emit(AgentEvent::StageProduced {
                        agent: agent.clone(),
                        keys,
                    });
                }
            }
            sink.emit(AgentEvent::StageFinished {
                agent,
                outcome: StageOutcome::Success,
            });
        }
        if let AgentPayload::Final(f) = &acc {
            sink.emit(AgentEvent::Finished {
                assistant: f.assistant.clone(),
            });
        }
        Ok(acc)
    }
}

// ===========================================================================
// TESTS — capabilities are mocked, so each agent is a pure async unit
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::events::test_support::CollectingSink;
    use crate::agent::payload::{InitialPrompt, ToolCall, ToolOutcome, ToolSchema};
    use crate::agent::tools::StreamingTool;
    use chrono::{DateTime, FixedOffset};
    use std::sync::Mutex;

    /// A pinned turn timestamp — payload B makes `now` deterministic input, so tests fix it here.
    fn fixed_now() -> DateTime<FixedOffset> {
        DateTime::parse_from_rfc3339("2026-07-11T09:30:00+08:00").unwrap()
    }

    /// A scripted LLM: hand it the turns to replay, and an agent becomes deterministic.
    struct ScriptedLlm {
        turns: Mutex<Vec<LlmResponse>>,
    }
    impl ScriptedLlm {
        fn arc(turns: Vec<LlmResponse>) -> Arc<dyn LlmCapability> {
            Arc::new(Self {
                turns: Mutex::new(turns),
            })
        }
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

    /// A stand-in data tool: whatever the fetcher's LLM asks, it returns fixed rows into
    /// `fetcher.records`. No MCP, no network — the fetcher stays a pure unit.
    struct FakeDataTool;
    #[async_trait]
    impl Tool for FakeDataTool {
        fn schema(&self) -> ToolSchema {
            ToolSchema {
                name: "bill_revenue".into(),
                description: "fetch revenue".into(),
                parameters: serde_json::json!({ "type": "object", "properties": {} }),
            }
        }
        fn target(&self) -> ArtifactKey {
            ArtifactKey::fetcher_records()
        }
        async fn call(&self, _args: serde_json::Value) -> Result<ToolOutcome, AgentError> {
            Ok(ToolOutcome::Produced(ArtifactValue::Json(
                serde_json::json!({ "revenue": 12345 }),
            )))
        }
    }

    fn fetcher_config() -> SubAgentConfig {
        SubAgentConfig {
            id: SubAgentId("fetcher".into()),
            instruction: "You fetch data using the available tools.".into(),
            llm: None,
            tools: vec![], // grants are resolved into `Vec<Box<dyn Tool>>` at construction
            accepts: vec![PayloadKind::Initial],
            output: None,
            capture_message: false, // tool-only stage — throwaway note
        }
    }

    fn tool_call(name: &str) -> LlmResponse {
        LlmResponse::ToolCalls(vec![ToolCall {
            id: "c1".into(),
            name: name.into(),
            arguments: serde_json::json!({}),
        }])
    }

    // ── the fetcher as a config-defined agent, unit-tested with mocks ──

    #[tokio::test]
    async fn fetcher_lets_the_llm_pick_a_granted_tool_then_produces_intermediate() {
        let fetcher = ConfiguredAgent::new(
            &fetcher_config(),
            ScriptedLlm::arc(vec![
                tool_call("bill_revenue"),
                LlmResponse::Message("fetched the revenue".into()),
            ]),
            vec![Box::new(FakeDataTool)],
            OutputShape::Intermediate,
        );

        let out = fetcher
            .run(AgentPayload::Initial(InitialPrompt {
                prompt: "revenue please".into(),
                history: vec![],
                now: fixed_now(),
            }))
            .await
            .unwrap();

        match out {
            AgentPayload::Intermediate(d) => {
                assert_eq!(d.prompt, "revenue please");
                let rows = d
                    .artifacts
                    .get(&ArtifactKey::fetcher_records())
                    .expect("fetcher.records must be produced");
                assert!(matches!(rows, ArtifactValue::Json(_)));
            }
            other => panic!("expected Intermediate, got {:?}", other.kind()),
        }
    }

    #[tokio::test]
    async fn fetcher_falls_on_a_variant_it_does_not_accept() {
        let fetcher = ConfiguredAgent::new(
            &fetcher_config(),
            ScriptedLlm::arc(vec![]),
            vec![],
            OutputShape::Intermediate,
        );
        let out = fetcher
            .run(AgentPayload::Final(FinalResult {
                user: "u".into(),
                assistant: "a".into(),
                now: fixed_now(),
                artifacts: HashMap::new(),
            }))
            .await;
        assert!(matches!(
            out,
            Err(AgentError::Mismatch {
                got: PayloadKind::Final,
                ..
            })
        ));
    }

    // ── a no-tool writer degenerates to a single model turn ──

    #[tokio::test]
    async fn writer_with_no_tools_produces_final_from_carried_artifacts() {
        let cfg = SubAgentConfig {
            id: SubAgentId("writer".into()),
            instruction: "Write a report from the material.".into(),
            llm: None,
            tools: vec![],
            accepts: vec![PayloadKind::Intermediate],
            output: None,
            capture_message: true,
        };
        let writer = ConfiguredAgent::new(
            &cfg,
            ScriptedLlm::arc(vec![LlmResponse::Message("THE REPORT".into())]),
            vec![], // no tools → nothing to fetch or invent with
            OutputShape::Final,
        );
        let mut artifacts = HashMap::new();
        artifacts.insert(
            ArtifactKey::fetcher_records(),
            ArtifactValue::Text("revenue=12345".into()),
        );
        let out = writer
            .run(AgentPayload::Intermediate(IntermediateData {
                prompt: "write it".into(),
                artifacts,
                now: fixed_now(),
            }))
            .await
            .unwrap();
        match out {
            AgentPayload::Final(f) => assert_eq!(f.assistant, "THE REPORT"),
            other => panic!("expected Final, got {:?}", other.kind()),
        }
    }

    // ── an Intermediate prose stage captures its message into an artifact ──

    #[tokio::test]
    async fn a_stages_message_is_captured_as_a_first_class_artifact_when_enabled() {
        // The analyst's shape: no tools, Intermediate output, `capture_message` on. Its message is
        // captured under `{id}.message`, so an Intermediate boundary keeps its prose (open-key
        // contract).
        let cfg = SubAgentConfig {
            id: SubAgentId("analyst".into()),
            instruction: "analyse the material".into(),
            llm: None,
            tools: vec![],
            accepts: vec![PayloadKind::Intermediate],
            output: None,
            capture_message: true,
        };
        let analyst = ConfiguredAgent::new(
            &cfg,
            ScriptedLlm::arc(vec![LlmResponse::Message(
                "## Analysis\nRevenue is up.".into(),
            )]),
            vec![],
            OutputShape::Intermediate,
        );

        let mut artifacts = HashMap::new();
        artifacts.insert(
            ArtifactKey::fetcher_records(),
            ArtifactValue::Json(serde_json::json!({ "revenue": 12345 })),
        );
        let out = analyst
            .run(AgentPayload::Intermediate(IntermediateData {
                prompt: "analyse it".into(),
                artifacts,
                now: fixed_now(),
            }))
            .await
            .unwrap();

        match out {
            AgentPayload::Intermediate(d) => {
                // The message landed at `analyst.message` with no capture wiring…
                assert_eq!(
                    d.artifacts.get(&ArtifactKey::message("analyst")),
                    Some(&ArtifactValue::Text("## Analysis\nRevenue is up.".into()))
                );
                // …and the upstream material is carried forward untouched (append-only).
                assert!(d.artifacts.contains_key(&ArtifactKey::fetcher_records()));
            }
            other => panic!("expected Intermediate, got {:?}", other.kind()),
        }
    }

    // ── a stage is time-aware: the payload's `now` lands in the system prompt ──

    /// An LLM that records the system message it was handed, so a test can assert what the stage
    /// actually sent — here, that the `# Current Time` header carried the payload's `now`.
    struct CapturingLlm {
        system: Mutex<Option<String>>,
    }
    impl CapturingLlm {
        fn arc() -> Arc<Self> {
            Arc::new(Self {
                system: Mutex::new(None),
            })
        }
    }
    #[async_trait]
    impl LlmCapability for CapturingLlm {
        async fn chat(
            &self,
            messages: &[LlmMessage],
            _tools: &[ToolSchema],
        ) -> Result<LlmResponse, AgentError> {
            if let Some(LlmMessage::System(s)) = messages.first() {
                *self.system.lock().unwrap() = Some(s.clone());
            }
            Ok(LlmResponse::Message("ok".into()))
        }
    }

    #[tokio::test]
    async fn run_prepends_the_payloads_now_as_a_current_time_header() {
        let llm = CapturingLlm::arc();
        let dyn_llm: Arc<dyn LlmCapability> = llm.clone();
        let agent = ConfiguredAgent::new(&fetcher_config(), dyn_llm, vec![], OutputShape::Final);

        // `now` is deterministic *input* — no ambient clock, no wiring override (payload B).
        agent
            .run(AgentPayload::Initial(InitialPrompt {
                prompt: "revenue".into(),
                history: vec![],
                now: fixed_now(),
            }))
            .await
            .unwrap();

        let system = llm
            .system
            .lock()
            .unwrap()
            .clone()
            .expect("the LLM should have been handed a system message");
        assert!(
            system.starts_with("# Current Time\n2026-07-11 09:30:00 +08:00\n\n"),
            "system prompt must open with the turn's time header, got:\n{system}"
        );
        // the agent's own instruction still follows the header
        assert!(system.contains("You fetch data using the available tools."));
    }

    #[tokio::test]
    async fn now_threads_unchanged_onto_the_terminal_result() {
        // Payload B's auditability guarantee: the turn's `now` reaches the Final result verbatim.
        let agent = ConfiguredAgent::new(
            &fetcher_config(),
            ScriptedLlm::arc(vec![LlmResponse::Message("done".into())]),
            vec![],
            OutputShape::Final,
        );
        let out = agent
            .run(AgentPayload::Initial(InitialPrompt {
                prompt: "hi".into(),
                history: vec![],
                now: fixed_now(),
            }))
            .await
            .unwrap();
        match out {
            AgentPayload::Final(f) => assert_eq!(f.now, fixed_now()),
            other => panic!("expected Final, got {:?}", other.kind()),
        }
    }

    // ── a code-defined agent drops into a pipeline beside config ones ──

    #[tokio::test]
    async fn orchestrator_runs_a_code_defined_agent() {
        let hello: Arc<dyn SubAgent> = Arc::new(HelloWorld::new(SubAgentId("hello".into())));
        let mut agents: HashMap<SubAgentId, Arc<dyn SubAgent>> = HashMap::new();
        agents.insert(SubAgentId("hello".into()), hello);
        let pipe = PipelineConfig {
            id: PipelineId("greet".into()),
            stages: vec![SubAgentId("hello".into())],
        };
        let mut orch = Orchestrator::new();
        orch.insert(pipe.id.clone(), resolve_pipeline(&pipe, &agents).unwrap());

        let out = orch
            .run(
                &PipelineId("greet".into()),
                AgentPayload::Initial(InitialPrompt {
                    prompt: "hi".into(),
                    history: vec![],
                    now: fixed_now(),
                }),
            )
            .await
            .unwrap();
        assert!(matches!(out, AgentPayload::Final(f) if f.assistant == "hello world."));
    }

    #[test]
    fn pipeline_referencing_an_unknown_agent_fails_fast() {
        let agents: HashMap<SubAgentId, Arc<dyn SubAgent>> = HashMap::new();
        let cfg = PipelineConfig {
            id: PipelineId("p".into()),
            stages: vec![SubAgentId("ghost".into())],
        };
        assert!(matches!(
            resolve_pipeline(&cfg, &agents),
            Err(ResolveError::UnknownAgentRef { .. })
        ));
    }

    // ── streaming: stage + tool + content events land on one ordered sink (mechanism A) ──

    /// A scripted LLM that *also* emits like the real streaming adapter: a `ContentDelta` before a
    /// final message, a `ToolCallProposed` before tool calls. It stands in for
    /// [`StreamingOpenAiLlm`](crate::agent::llm::StreamingOpenAiLlm) with no network, so the
    /// wiring (stage + tool + content events, correctly interleaved) is unit-testable.
    struct ScriptedStreamingLlm {
        turns: Mutex<Vec<LlmResponse>>,
        sink: Arc<dyn EventSink>,
    }
    impl ScriptedStreamingLlm {
        fn arc(turns: Vec<LlmResponse>, sink: Arc<dyn EventSink>) -> Arc<dyn LlmCapability> {
            Arc::new(Self {
                turns: Mutex::new(turns),
                sink,
            })
        }
    }
    #[async_trait]
    impl LlmCapability for ScriptedStreamingLlm {
        async fn chat(
            &self,
            _messages: &[LlmMessage],
            _tools: &[ToolSchema],
        ) -> Result<LlmResponse, AgentError> {
            let response = self.turns.lock().unwrap().remove(0);
            match &response {
                LlmResponse::Message(text) => self
                    .sink
                    .emit(AgentEvent::ContentDelta { text: text.clone() }),
                LlmResponse::ToolCalls(calls) => {
                    for c in calls {
                        self.sink.emit(AgentEvent::ToolCallProposed {
                            id: c.id.clone(),
                            name: c.name.clone(),
                        });
                    }
                }
            }
            Ok(response)
        }
    }

    #[tokio::test]
    async fn run_emitting_streams_stage_tool_and_content_events_in_order() {
        let sink: Arc<CollectingSink> = Arc::new(CollectingSink::new());
        let dyn_sink: Arc<dyn EventSink> = sink.clone();

        // Upstream fetcher: a BUFFERED (non-emitting) LLM + a StreamingTool-wrapped data tool.
        // This models a buffered stage that still shows tool activity (plan §8.2).
        let fetcher = ConfiguredAgent::new(
            &fetcher_config(),
            ScriptedLlm::arc(vec![
                tool_call("bill_revenue"),
                LlmResponse::Message("fetched".into()),
            ]),
            StreamingTool::wrap_all(vec![Box::new(FakeDataTool)], dyn_sink.clone()),
            OutputShape::Intermediate,
        );

        // Terminal writer: a STREAMING LLM (emits content deltas) + no tools.
        let writer_cfg = SubAgentConfig {
            id: SubAgentId("writer".into()),
            instruction: "write".into(),
            llm: None,
            tools: vec![],
            accepts: vec![PayloadKind::Intermediate],
            output: None,
            capture_message: true,
        };
        let writer = ConfiguredAgent::new(
            &writer_cfg,
            ScriptedStreamingLlm::arc(
                vec![LlmResponse::Message("THE REPORT".into())],
                dyn_sink.clone(),
            ),
            vec![],
            OutputShape::Final,
        );

        let mut agents: HashMap<SubAgentId, Arc<dyn SubAgent>> = HashMap::new();
        agents.insert(SubAgentId("fetcher".into()), Arc::new(fetcher));
        agents.insert(SubAgentId("writer".into()), Arc::new(writer));
        let pipe = PipelineConfig {
            id: PipelineId("report".into()),
            stages: vec![SubAgentId("fetcher".into()), SubAgentId("writer".into())],
        };
        let mut orch = Orchestrator::new();
        orch.insert(pipe.id.clone(), resolve_pipeline(&pipe, &agents).unwrap());

        let out = orch
            .run_emitting(
                &PipelineId("report".into()),
                AgentPayload::Initial(InitialPrompt {
                    prompt: "revenue".into(),
                    history: vec![],
                    now: fixed_now(),
                }),
                &*sink,
            )
            .await
            .unwrap();

        assert!(matches!(out, AgentPayload::Final(f) if f.assistant == "THE REPORT"));
        assert_eq!(
            sink.events(),
            vec![
                AgentEvent::StageStarted {
                    agent: SubAgentId("fetcher".into()),
                    input: PayloadKind::Initial,
                },
                AgentEvent::ToolStarted {
                    name: "bill_revenue".into(),
                },
                AgentEvent::ToolProduced {
                    name: "bill_revenue".into(),
                    target: ArtifactKey::fetcher_records(),
                },
                AgentEvent::StageProduced {
                    agent: SubAgentId("fetcher".into()),
                    // the fetcher has `capture_message` off (tool-only stage), so it produces only
                    // the tool artifact — its throwaway note is not captured.
                    keys: vec![ArtifactKey::fetcher_records()],
                },
                AgentEvent::StageFinished {
                    agent: SubAgentId("fetcher".into()),
                    outcome: StageOutcome::Success,
                },
                AgentEvent::StageStarted {
                    agent: SubAgentId("writer".into()),
                    input: PayloadKind::Intermediate,
                },
                AgentEvent::ContentDelta {
                    text: "THE REPORT".into(),
                },
                AgentEvent::StageFinished {
                    agent: SubAgentId("writer".into()),
                    outcome: StageOutcome::Success,
                },
                AgentEvent::Finished {
                    assistant: "THE REPORT".into(),
                },
            ]
        );
    }

    #[tokio::test]
    async fn run_emitting_marks_a_failed_stage_then_emits_a_terminal_error() {
        let sink: Arc<CollectingSink> = Arc::new(CollectingSink::new());

        // A stage that only accepts Intermediate, handed an Initial → it falls with `Mismatch`
        // before its LLM is ever consulted.
        let writer_cfg = SubAgentConfig {
            id: SubAgentId("writer".into()),
            instruction: "write".into(),
            llm: None,
            tools: vec![],
            accepts: vec![PayloadKind::Intermediate],
            output: None,
            capture_message: true,
        };
        let writer = ConfiguredAgent::new(
            &writer_cfg,
            ScriptedLlm::arc(vec![]), // never called — the accept-check fails first
            vec![],
            OutputShape::Final,
        );
        let mut agents: HashMap<SubAgentId, Arc<dyn SubAgent>> = HashMap::new();
        agents.insert(SubAgentId("writer".into()), Arc::new(writer));
        let pipe = PipelineConfig {
            id: PipelineId("p".into()),
            stages: vec![SubAgentId("writer".into())],
        };
        let mut orch = Orchestrator::new();
        orch.insert(pipe.id.clone(), resolve_pipeline(&pipe, &agents).unwrap());

        let result = orch
            .run_emitting(
                &PipelineId("p".into()),
                AgentPayload::Initial(InitialPrompt {
                    prompt: "x".into(),
                    history: vec![],
                    now: fixed_now(),
                }),
                &*sink,
            )
            .await;

        assert!(result.is_err());
        // The failed stage is closed out with `Failure` (red dot) *before* the terminal error.
        let events = sink.events();
        assert_eq!(events.len(), 3, "got {events:?}");
        assert!(matches!(
            &events[0],
            AgentEvent::StageStarted {
                input: PayloadKind::Initial,
                ..
            }
        ));
        assert!(matches!(
            &events[1],
            AgentEvent::StageFinished {
                outcome: StageOutcome::Failure,
                ..
            }
        ));
        assert!(matches!(&events[2], AgentEvent::Error { .. }));
    }
}
