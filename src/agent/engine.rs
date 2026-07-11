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

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;

use crate::agent::config::{
    OutputShape, PipelineConfig, PipelineId, ResolveError, SubAgentConfig, SubAgentId,
};
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
        entries.sort_by_key(|(k, _)| **k);
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

        // Assemble the user turn + carry-forward artifacts (append-only, payload §2.4).
        let (prompt, incoming) = match input {
            AgentPayload::Initial(p) => (p.prompt, HashMap::new()),
            AgentPayload::Intermediate(d) => (d.prompt, d.artifacts),
            // Excluded by the accept-check above unless an agent explicitly accepts Final.
            AgentPayload::Final(f) => (f.user, HashMap::new()),
        };
        let material = Self::render_material(&incoming);
        let user = if material.is_empty() {
            prompt.clone()
        } else {
            format!("{prompt}\n\nMaterial:\n{material}")
        };

        // The LLM chooses among *only* the granted tools; out-of-set calls are rejected at
        // dispatch inside the loop (payload §2.3).
        let (text, produced) =
            run_llm_loop(&self.llm, &self.instruction, &user, &self.tools).await?;

        match self.output {
            OutputShape::Intermediate => {
                let mut artifacts = incoming;
                artifacts.extend(produced); // append-only merge
                Ok(AgentPayload::Intermediate(IntermediateData {
                    prompt,
                    artifacts,
                }))
            }
            OutputShape::Final => Ok(AgentPayload::Final(FinalResult {
                user: prompt,
                assistant: text,
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
        if !self.accepts().contains(&input.kind()) {
            return Err(AgentError::Mismatch {
                expected: self.accepts(),
                got: input.kind(),
            });
        }
        let user = match input {
            AgentPayload::Initial(p) => p.prompt,
            _ => String::new(),
        };
        Ok(AgentPayload::Final(FinalResult {
            user,
            assistant: "hello world.".to_string(),
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
}

// ===========================================================================
// TESTS — capabilities are mocked, so each agent is a pure async unit
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::payload::{InitialPrompt, ToolCall, ToolOutcome, ToolSchema};
    use std::sync::Mutex;

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
            ArtifactKey::FetcherRecords
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
            }))
            .await
            .unwrap();

        match out {
            AgentPayload::Intermediate(d) => {
                assert_eq!(d.prompt, "revenue please");
                let rows = d
                    .artifacts
                    .get(&ArtifactKey::FetcherRecords)
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
        };
        let writer = ConfiguredAgent::new(
            &cfg,
            ScriptedLlm::arc(vec![LlmResponse::Message("THE REPORT".into())]),
            vec![], // no tools → nothing to fetch or invent with
            OutputShape::Final,
        );
        let mut artifacts = HashMap::new();
        artifacts.insert(
            ArtifactKey::FetcherRecords,
            ArtifactValue::Text("revenue=12345".into()),
        );
        let out = writer
            .run(AgentPayload::Intermediate(IntermediateData {
                prompt: "write it".into(),
                artifacts,
            }))
            .await
            .unwrap();
        match out {
            AgentPayload::Final(f) => assert_eq!(f.assistant, "THE REPORT"),
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
}
