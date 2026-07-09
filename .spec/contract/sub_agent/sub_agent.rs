//! # SubAgent — normative config model + suggested resolution/engine
//!
//! This file layers on top of [`agent_payload`](../agent_payload/agent_payload.rs):
//! that contract owns the value flowing between agents and the runtime morphism
//! (`async fn(AgentPayload) -> Result<AgentPayload, AgentError>`); **this** file owns the
//! *configuration surface* an author writes, the *resolution rules* that turn it into a
//! runnable agent, and the *composition* of agents into pipelines. It has two parts, and
//! the distinction is load-bearing:
//!
//! - **PART A — NORMATIVE.** The config data model (`LlmConfig`/`Provider`, `ToolId`,
//!   `SubAgentConfig`, `PipelineConfig`) and the resolution & composition rules
//!   (default-LLM *field* merge, tool-grant isolation resolved at boot / fail-fast,
//!   k8s-style secret binding, and the **self-checking composition rule** that replaces any
//!   static wiring graph). These bind: any loader must honor them or its configs are not
//!   portable.
//! - **PART B — SUGGESTED.** *One* encoding of resolution and execution: `ResolvedLlm`, the
//!   `ToolRegistry`, the generic `ConfiguredAgent`, and an `Orchestrator`. Advisory — swap
//!   any of it as long as PART A holds.
//!
//! ## The sub-agent is abstract; config drives the default implementation
//!
//! A sub-agent is anything satisfying the payload morphism and the *falling convention*
//! (self-check its input, return `AgentError::Mismatch`, never panic). The default way to
//! obtain one is **data**: a [`SubAgentConfig`] fed to one generic [`ConfiguredAgent`], so
//! the payload contract's `DataFetcher` / `ReportWriter` become *configs*, not bespoke
//! types. Hand-written agents remain possible; both are the same abstract [`SubAgent`].
//!
//! ## Why there is no `produces` field
//!
//! Composition safety is a **runtime** property, not a static graph check: each agent
//! self-checks what it `accepts` and falls on a mismatch. Pipelines are therefore just
//! ordered lists, which is what lets a sub-agent be recombined into several pipelines
//! without re-deriving a wiring graph.

#![allow(dead_code)] // skeleton: some example variants/tools are not yet wired up

// The payload contract is the substrate. Included by relative path so this reference
// compiles standalone (`rustc`/a scratch crate) exactly as `agent_payload.rs` does.
#[path = "../agent_payload/agent_payload.rs"]
mod agent_payload;

use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;

use async_trait::async_trait;

// Explicit imports (no glob): we define our own `SubAgent` trait — the payload contract's
// same-named advisory trait is intentionally *not* imported, because we drop `produces`.
use agent_payload::{
    AgentError, AgentPayload, ArtifactKey, ArtifactValue, FinalResult, IntermediateData,
    LlmCapability, PayloadKind, Tool, run_llm_loop,
};

// ===========================================================================
// PART A — NORMATIVE CONFIG MODEL (binding: the surface an author writes)
// ===========================================================================

/// Stable identity of a sub-agent. Namespaces the [`ArtifactKey`]s it produces (payload
/// §2.5) and labels its logs. Author-defined, so a newtype rather than a closed enum.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct SubAgentId(pub String);

impl fmt::Display for SubAgentId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Stable identity of a pipeline (e.g. how an incoming request selects one).
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct PipelineId(pub String);

impl fmt::Display for PipelineId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// A **k8s-style secret reference**: the config carries a *key name*, and the matching
/// entry in the process environment supplies the value. Config files never carry a raw key.
/// Binding happens at boot; a referenced key with no environment entry **fails boot** (§2.3).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct SecretRef(pub String);

/// The LLM provider — a *known enum with a `Custom` escape hatch*. Known variants carry
/// their default base URL and auth style; `Custom` is the generic OpenAI-compatible
/// `(name, endpoint)` for anything else.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Provider {
    /// Hosted OpenRouter. Default base URL; API key via the fixed `OPENROUTER_API_KEY`
    /// secret. (An overridable key name is an implementation-plan item.)
    OpenRouter,
    /// Local Ollama — keyless. `endpoint` defaults to `http://localhost:11434/v1`.
    Ollama { endpoint: Option<String> },
    /// Any other OpenAI-compatible endpoint, named and located by the author.
    Custom {
        name: String,
        endpoint: String,
        api_key: Option<SecretRef>,
    },
}

impl Provider {
    /// The default base URL for known providers (`None` ⇒ the variant already carries its
    /// own `endpoint`).
    pub fn default_base_url(&self) -> Option<&'static str> {
        match self {
            Provider::OpenRouter => Some("https://openrouter.ai/api/v1"),
            Provider::Ollama { .. } => Some("http://localhost:11434/v1"),
            Provider::Custom { .. } => None,
        }
    }

    /// The secret key this provider authenticates with, if any. Keyless providers (Ollama)
    /// return `None`.
    pub fn secret_ref(&self) -> Option<SecretRef> {
        match self {
            Provider::OpenRouter => Some(SecretRef("OPENROUTER_API_KEY".to_string())),
            Provider::Ollama { .. } => None,
            Provider::Custom { api_key, .. } => api_key.clone(),
        }
    }
}

/// Optional generation parameters. Each field merges *independently* over the default LLM
/// (§2.1), so an agent can nudge `temperature` alone and inherit everything else.
#[derive(Clone, PartialEq, Debug, Default)]
pub struct GenerationParams {
    pub temperature: Option<f32>,
    pub top_p: Option<f32>,
    pub max_tokens: Option<u32>,
}

/// A sub-agent's LLM config — **all-optional**, so it states only what differs from the
/// default LLM. `provider` is *atomic* (overridden or inherited whole); `model` and each
/// [`GenerationParams`] field merge independently.
#[derive(Clone, PartialEq, Debug, Default)]
pub struct LlmConfig {
    pub provider: Option<Provider>,
    pub model: Option<String>,
    pub params: GenerationParams,
}

/// The logical tool identifier — a **closed enum** (parity with [`ArtifactKey`]: a typo is a
/// compile/parse error), decoupled from any backend. The [`ToolRegistry`] maps it to a
/// concrete [`Tool`]; MCP is one backend among possible others. `ToolId` (which *grants* a
/// tool) is distinct from [`ArtifactKey`] (the *slot* a tool's result fills).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum ToolId {
    BillRevenue,
    StationRevenueRanking,
    BillCharge,
    MemberAnalysis,
    // EXTEND: one variant per logical tool the orchestration designer offers.
}

impl fmt::Display for ToolId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            ToolId::BillRevenue => "bill_revenue",
            ToolId::StationRevenueRanking => "station_revenue_ranking",
            ToolId::BillCharge => "bill_charge",
            ToolId::MemberAnalysis => "member_analysis",
        })
    }
}

impl std::str::FromStr for ToolId {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "bill_revenue" => Ok(ToolId::BillRevenue),
            "station_revenue_ranking" => Ok(ToolId::StationRevenueRanking),
            "bill_charge" => Ok(ToolId::BillCharge),
            "member_analysis" => Ok(ToolId::MemberAnalysis),
            other => Err(format!("unknown ToolId: {other}")),
        }
    }
}

/// The output variant a [`ConfiguredAgent`] shapes its result into. This is *execution-time
/// output shaping* used to build the outgoing payload — it is **not** the removed static
/// `produces` field and participates in no graph validation (§2.4).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum OutputShape {
    /// Working data for a downstream agent.
    Intermediate,
    /// The user-facing terminal result.
    Final,
}

/// The authored unit: a sub-agent as *data*. Fed to the generic [`ConfiguredAgent`] engine
/// at resolution.
#[derive(Clone, Debug)]
pub struct SubAgentConfig {
    pub id: SubAgentId,
    /// The system prompt this agent carries (payload §1: each agent carries its own).
    pub instruction: String,
    /// Per-agent LLM; unset fields inherit the default LLM. `None` ⇒ the default verbatim.
    pub llm: Option<LlmConfig>,
    /// The granted tool set — the isolation boundary (payload §2.3). Resolved at boot.
    pub tools: Vec<ToolId>,
    /// Which payload variants this agent consumes; drives its self-check (§2.4).
    pub accepts: Vec<PayloadKind>,
    /// How to shape the outgoing payload (see [`OutputShape`]). `None` (the common case)
    /// derives the shape from the agent's position across the pipelines that reference it —
    /// see [`effective_output`]. Set explicitly only when the agent's shape must diverge from
    /// its structural position (e.g. reused as both a non-terminal and a terminal stage).
    pub output: Option<OutputShape>,
}

/// A pipeline is a first-class, *ordered* list of sub-agent references. A deployment may
/// declare several, and the same sub-agent may appear in more than one (compatibility is a
/// runtime guarantee, §2.4).
#[derive(Clone, Debug)]
pub struct PipelineConfig {
    pub id: PipelineId,
    pub stages: Vec<SubAgentId>,
}

// ===========================================================================
// PART B — SUGGESTED RESOLUTION & ENGINE (advisory: one way to run the above)
// ===========================================================================

/// A boot/resolution failure. Every variant is a *fail-fast* condition surfaced before any
/// LLM call (§2), so a deployment never limps along with a half-wired pipeline.
#[derive(Debug, PartialEq, Eq)]
pub enum ResolveError {
    /// A granted [`ToolId`] has no registry entry (§2.2).
    UnknownTool(ToolId),
    /// A referenced secret key has no matching environment entry (§2.3).
    MissingSecret(String),
    /// No `model` in the agent config *or* the default LLM (§2.1).
    MissingModel(SubAgentId),
    /// A pipeline names a sub-agent id that was never defined (§1.4).
    UnknownAgentRef {
        pipeline: PipelineId,
        agent: SubAgentId,
    },
    /// `output` was left unset and the agent's position is ambiguous across the pipelines
    /// that reference it — terminal in one, non-terminal in another (§2.4). The author must
    /// set `output` explicitly to disambiguate.
    AmbiguousOutput(SubAgentId),
}

impl fmt::Display for ResolveError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ResolveError::UnknownTool(t) => write!(f, "no registry entry for tool `{t}`"),
            ResolveError::MissingSecret(k) => write!(f, "secret `{k}` not present in environment"),
            ResolveError::MissingModel(a) => write!(f, "sub-agent `{a}` has no model and the default LLM sets none"),
            ResolveError::UnknownAgentRef { pipeline, agent } => {
                write!(f, "pipeline `{pipeline}` references unknown sub-agent `{agent}`")
            }
            ResolveError::AmbiguousOutput(agent) => write!(
                f,
                "sub-agent `{agent}` has no explicit `output` and its position is ambiguous \
                 across pipelines (terminal in one, non-terminal in another) — set `output` \
                 explicitly"
            ),
        }
    }
}

impl std::error::Error for ResolveError {}

/// The option-free product of §2.1: a fully-resolved LLM ready to construct an
/// [`LlmCapability`]. The `ResolvedLlm` → capability factory (which needs the vendor SDK) is
/// an implementation-plan item; here the capability is injected directly (see
/// [`ConfiguredAgent`]) so the engine unit-tests without a network.
#[derive(Clone, PartialEq, Debug)]
pub struct ResolvedLlm {
    pub provider: Provider,
    pub base_url: String,
    pub model: String,
    pub temperature: f32,
    pub top_p: f32,
    pub max_tokens: u32,
    /// The bound secret value, or `None` for keyless providers (Ollama).
    pub api_key: Option<String>,
}

/// Read a secret value for `provider` from `env` (a k8s-style key→value map). Keyless
/// providers bind to `None`; a referenced-but-absent key **fails** (§2.3).
fn bind_secret(
    provider: &Provider,
    env: &HashMap<String, String>,
) -> Result<Option<String>, ResolveError> {
    match provider.secret_ref() {
        None => Ok(None),
        Some(SecretRef(key)) => env
            .get(&key)
            .cloned()
            .map(Some)
            .ok_or(ResolveError::MissingSecret(key)),
    }
}

/// **Default-LLM field merge (§2.1).** `provider` is atomic; `model` and each param merge
/// independently over `default`. Secrets are bound from `env` against the *resolved*
/// provider. A missing model fails fast, attributed to `agent`.
pub fn resolve_llm(
    agent: &SubAgentId,
    default: &ResolvedLlm,
    over: Option<&LlmConfig>,
    env: &HashMap<String, String>,
) -> Result<ResolvedLlm, ResolveError> {
    let over = over.cloned().unwrap_or_default();

    // provider is atomic: take the override whole, or inherit the default's.
    let provider = over.provider.unwrap_or_else(|| default.provider.clone());
    let base_url = provider
        .default_base_url()
        .map(str::to_string)
        .unwrap_or_else(|| match &provider {
            Provider::Custom { endpoint, .. } => endpoint.clone(),
            Provider::Ollama { endpoint: Some(e) } => e.clone(),
            _ => default.base_url.clone(),
        });

    let model = over.model.or_else(|| Some(default.model.clone()));
    let model = model.ok_or_else(|| ResolveError::MissingModel(agent.clone()))?;

    let api_key = bind_secret(&provider, env)?;

    Ok(ResolvedLlm {
        provider,
        base_url,
        model,
        temperature: over.params.temperature.unwrap_or(default.temperature),
        top_p: over.params.top_p.unwrap_or(default.top_p),
        max_tokens: over.params.max_tokens.unwrap_or(default.max_tokens),
        api_key,
    })
}

/// **Output-shape default derivation (§2.4).** `configured` (the config's explicit value)
/// always wins. When unset, the shape is derived from `id`'s position across every declared
/// pipeline that references it: terminal in *all* of them ⇒ [`OutputShape::Final`];
/// non-terminal in *all* of them ⇒ [`OutputShape::Intermediate`]. An agent that is terminal in
/// one pipeline and non-terminal in another (the `quick_fetch`-style reuse case) has no
/// unambiguous default and **fails resolution**, demanding an explicit `output`. An agent
/// referenced by no pipeline is unreachable; its shape is never observed, so it defaults to
/// `Intermediate` rather than erroring.
pub fn effective_output(
    id: &SubAgentId,
    configured: Option<OutputShape>,
    pipelines: &[PipelineConfig],
) -> Result<OutputShape, ResolveError> {
    if let Some(shape) = configured {
        return Ok(shape);
    }

    let mut any_terminal = false;
    let mut any_non_terminal = false;
    for pipeline in pipelines {
        for (i, stage) in pipeline.stages.iter().enumerate() {
            if stage != id {
                continue;
            }
            if i == pipeline.stages.len() - 1 {
                any_terminal = true;
            } else {
                any_non_terminal = true;
            }
        }
    }

    match (any_terminal, any_non_terminal) {
        (true, true) => Err(ResolveError::AmbiguousOutput(id.clone())),
        (true, false) => Ok(OutputShape::Final),
        (false, true) => Ok(OutputShape::Intermediate),
        (false, false) => Ok(OutputShape::Intermediate), // unreferenced; never observed
    }
}

// --- The tool registry: the abstract seam between a grant and a backend ------

/// Builds a fresh boxed [`Tool`] on demand. Capturing e.g. an `McpHandle`, it lets the same
/// logical [`ToolId`] be re-backed (MCP → HTTP → mock) without touching any agent config.
pub type ToolFactory = Arc<dyn Fn() -> Box<dyn Tool> + Send + Sync>;

/// Designer-owned map from logical [`ToolId`] to a concrete tool backend. A **closed** set:
/// every grant is resolved against it at boot, and an unresolvable id fails fast.
#[derive(Clone, Default)]
pub struct ToolRegistry {
    factories: HashMap<ToolId, ToolFactory>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a backend for a logical tool.
    pub fn register(&mut self, id: ToolId, factory: ToolFactory) -> &mut Self {
        self.factories.insert(id, factory);
        self
    }

    /// Resolve a grant into concrete tools, **failing fast** on the first unknown id (§2.2).
    pub fn resolve(&self, grants: &[ToolId]) -> Result<Vec<Box<dyn Tool>>, ResolveError> {
        grants
            .iter()
            .map(|id| {
                self.factories
                    .get(id)
                    .map(|f| f())
                    .ok_or(ResolveError::UnknownTool(*id))
            })
            .collect()
    }
}

// --- The abstract SubAgent (no `produces`) and the generic engine -----------

/// The sub-agent abstraction *of this contract*: a self-checking morphism. It deliberately
/// omits the payload contract's advisory `produces` — composition safety is the runtime
/// falling convention (§2.4), not a static graph.
#[async_trait]
pub trait SubAgent: Send + Sync {
    fn id(&self) -> &SubAgentId;
    fn accepts(&self) -> &'static [PayloadKind];
    async fn run(&self, input: AgentPayload) -> Result<AgentPayload, AgentError>;
}

// So a `ConfiguredAgent` can hold a type-erased LLM (`Arc<dyn LlmCapability>`) yet still call
// `run_llm_loop`, whose type parameter is `Sized`. The trait is local, so this impl is legal.
#[async_trait]
impl LlmCapability for Arc<dyn LlmCapability> {
    async fn chat(
        &self,
        messages: &[agent_payload::LlmMessage],
        tools: &[agent_payload::ToolSchema],
    ) -> Result<agent_payload::LlmResponse, AgentError> {
        (**self).chat(messages, tools).await
    }
}

/// Intern a runtime accept-set into a `'static` slice (there are only 2³ subsets of the
/// three [`PayloadKind`]s), so a config-driven agent can satisfy the payload contract's
/// `&'static` `Mismatch` without leaking.
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
/// This *is* "the sub-agent is abstract, config drives it".
pub struct ConfiguredAgent {
    id: SubAgentId,
    instruction: String,
    llm: Arc<dyn LlmCapability>,
    tools: Vec<Box<dyn Tool>>,
    accepts: &'static [PayloadKind],
    output: OutputShape,
}

impl ConfiguredAgent {
    /// Assemble a runnable agent from resolved parts. The LLM capability is injected (built
    /// from a [`ResolvedLlm`] by a factory, elsewhere), which is what makes the agent a pure
    /// async function of its payload in tests. `output` is the **resolved** shape — the
    /// config's explicit value, or [`effective_output`]'s position-derived default — never
    /// `cfg.output` read directly, since the default can only be computed once every pipeline
    /// referencing this agent is known.
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

    /// Render granted artifacts into a deterministic material block (HashMap order is not
    /// stable), mirroring the payload contract's `ReportWriter`.
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
        let (text, produced) = run_llm_loop(&self.llm, &self.instruction, &user, &self.tools).await?;

        match self.output {
            OutputShape::Intermediate => {
                let mut artifacts = incoming;
                artifacts.extend(produced); // append-only merge
                Ok(AgentPayload::Intermediate(IntermediateData { prompt, artifacts }))
            }
            OutputShape::Final => Ok(AgentPayload::Final(FinalResult {
                user: prompt,
                assistant: text,
            })),
        }
    }
}

// --- Orchestration over many pipelines (first-class) ------------------------

/// Resolve a pipeline's stage references against the built agents, **failing fast** on an
/// unknown reference (§1.4). Returns the ordered, runnable stages.
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

/// Holds every resolved pipeline and runs a selected one. Kleisli composition: the first
/// stage that falls short-circuits the rest (`?`).
#[derive(Default)]
pub struct Orchestrator {
    pipelines: HashMap<PipelineId, Vec<Arc<dyn SubAgent>>>,
}

impl Orchestrator {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&mut self, id: PipelineId, stages: Vec<Arc<dyn SubAgent>>) -> &mut Self {
        self.pipelines.insert(id, stages);
        self
    }

    /// Run the pipeline named `id`. Threads the payload through each stage; a stage mismatch
    /// (§2.4) surfaces as a typed `AgentError`. An unknown pipeline id is a caller error.
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
    use agent_payload::{InitialPrompt, LlmMessage, LlmResponse, ToolCall, ToolSchema};
    use std::sync::Mutex;

    // ── test doubles ──

    /// An LLM whose responses are scripted, so agents become deterministic units.
    struct ScriptedLlm {
        queue: Mutex<Vec<LlmResponse>>,
    }
    impl ScriptedLlm {
        fn arc(responses: Vec<LlmResponse>) -> Arc<dyn LlmCapability> {
            Arc::new(Self { queue: Mutex::new(responses) })
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
        async fn call(&self, _args: serde_json::Value) -> Result<ArtifactValue, AgentError> {
            Ok(ArtifactValue::Json(serde_json::json!([{ "id": 1 }])))
        }
    }

    fn tool_call(name: &str) -> LlmResponse {
        LlmResponse::ToolCalls(vec![ToolCall {
            id: "c1".into(),
            name: name.into(),
            arguments: serde_json::json!({}),
        }])
    }

    fn default_llm() -> ResolvedLlm {
        ResolvedLlm {
            provider: Provider::OpenRouter,
            base_url: "https://openrouter.ai/api/v1".into(),
            model: "google/gemini-flash".into(),
            temperature: 0.7,
            top_p: 0.9,
            max_tokens: 1024,
            api_key: Some("default-key".into()),
        }
    }

    fn env(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    // ── §2.1 default-LLM field merge ──

    #[test]
    fn merge_overrides_only_model_and_inherits_the_rest() {
        let over = LlmConfig {
            model: Some("anthropic/claude-sonnet-5".into()),
            ..Default::default()
        };
        let e = env(&[("OPENROUTER_API_KEY", "sk-xyz")]);
        let r = resolve_llm(&SubAgentId("writer".into()), &default_llm(), Some(&over), &e).unwrap();

        assert_eq!(r.model, "anthropic/claude-sonnet-5"); // overridden
        assert_eq!(r.provider, Provider::OpenRouter); // inherited (atomic)
        assert_eq!(r.temperature, 0.7); // inherited per-field
        assert_eq!(r.api_key.as_deref(), Some("sk-xyz")); // bound from env, not the default's key
    }

    #[test]
    fn omitting_llm_entirely_yields_the_default() {
        let e = env(&[("OPENROUTER_API_KEY", "sk-xyz")]);
        let r = resolve_llm(&SubAgentId("fetcher".into()), &default_llm(), None, &e).unwrap();
        assert_eq!(r.model, "google/gemini-flash");
        assert_eq!(r.temperature, 0.7);
    }

    #[test]
    fn atomic_provider_override_switches_base_url_and_auth() {
        let over = LlmConfig {
            provider: Some(Provider::Ollama { endpoint: None }),
            ..Default::default()
        };
        // Ollama is keyless, so an empty env still resolves.
        let r = resolve_llm(&SubAgentId("local".into()), &default_llm(), Some(&over), &env(&[])).unwrap();
        assert_eq!(r.base_url, "http://localhost:11434/v1");
        assert_eq!(r.api_key, None);
        assert_eq!(r.model, "google/gemini-flash"); // model still inherited
    }

    // ── §2.3 secrets ──

    #[test]
    fn missing_secret_fails_fast() {
        let err = resolve_llm(&SubAgentId("a".into()), &default_llm(), None, &env(&[])).unwrap_err();
        assert_eq!(err, ResolveError::MissingSecret("OPENROUTER_API_KEY".into()));
    }

    // ── §2.2 tool grant, closed set, boot-resolved ──

    #[test]
    fn registry_resolves_a_grant() {
        let mut reg = ToolRegistry::new();
        reg.register(ToolId::BillRevenue, Arc::new(|| Box::new(FakeFetchTool)));
        let tools = reg.resolve(&[ToolId::BillRevenue]).unwrap();
        assert_eq!(tools.len(), 1);
    }

    #[test]
    fn registry_unknown_tool_fails_fast() {
        let reg = ToolRegistry::new(); // nothing registered
        // `.err()` drops the (non-Debug) `Ok` tool vec, unlike `unwrap_err()`.
        let err = reg.resolve(&[ToolId::MemberAnalysis]).err();
        assert_eq!(err, Some(ResolveError::UnknownTool(ToolId::MemberAnalysis)));
    }

    // ── §2.4 self-checking composition ──

    #[tokio::test]
    async fn agent_falls_on_a_variant_it_does_not_accept() {
        let writer = ConfiguredAgent::new(
            &SubAgentConfig {
                id: SubAgentId("writer".into()),
                instruction: "write".into(),
                llm: None,
                tools: vec![],
                accepts: vec![PayloadKind::Intermediate],
                output: None,
            },
            ScriptedLlm::arc(vec![]),
            vec![],
            OutputShape::Final, // resolved shape; irrelevant here — the mismatch short-circuits first
        );
        let out = writer
            .run(AgentPayload::Final(FinalResult { user: "u".into(), assistant: "a".into() }))
            .await;
        assert!(matches!(out, Err(AgentError::Mismatch { got: PayloadKind::Final, .. })));
    }

    #[tokio::test]
    async fn fetcher_lets_llm_pick_a_granted_tool_then_produces_intermediate() {
        let fetcher = ConfiguredAgent::new(
            &SubAgentConfig {
                id: SubAgentId("fetcher".into()),
                instruction: "fetch".into(),
                llm: None,
                tools: vec![ToolId::BillRevenue],
                accepts: vec![PayloadKind::Initial],
                output: None,
            },
            ScriptedLlm::arc(vec![tool_call("query_records"), LlmResponse::Message("done".into())]),
            vec![Box::new(FakeFetchTool)],
            OutputShape::Intermediate,
        );
        let out = fetcher
            .run(AgentPayload::Initial(InitialPrompt { prompt: "get it".into(), history: vec![] }))
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
    async fn writer_with_no_tools_produces_final() {
        let writer = ConfiguredAgent::new(
            &SubAgentConfig {
                id: SubAgentId("writer".into()),
                instruction: "write".into(),
                llm: None,
                tools: vec![],
                accepts: vec![PayloadKind::Intermediate],
                output: None,
            },
            ScriptedLlm::arc(vec![LlmResponse::Message("REPORT".into())]),
            vec![],
            OutputShape::Final,
        );
        let mut artifacts = HashMap::new();
        artifacts.insert(ArtifactKey::FetcherRecords, ArtifactValue::Text("rows".into()));
        let out = writer
            .run(AgentPayload::Intermediate(IntermediateData { prompt: "write it".into(), artifacts }))
            .await
            .unwrap();
        match out {
            AgentPayload::Final(f) => assert_eq!(f.assistant, "REPORT"),
            other => panic!("expected Final, got {:?}", other.kind()),
        }
    }

    // ── §2.4 output-shape default: derived from position, ambiguous ⇒ explicit required ──

    #[test]
    fn output_derives_final_when_terminal_in_every_referencing_pipeline() {
        let writer = SubAgentId("writer".into());
        let full = PipelineConfig {
            id: PipelineId("revenue_report".into()),
            stages: vec![SubAgentId("fetcher".into()), writer.clone()],
        };
        assert_eq!(effective_output(&writer, None, &[full]), Ok(OutputShape::Final));
    }

    #[test]
    fn output_derives_intermediate_when_non_terminal_in_every_referencing_pipeline() {
        let fetcher = SubAgentId("fetcher".into());
        let full = PipelineConfig {
            id: PipelineId("revenue_report".into()),
            stages: vec![fetcher.clone(), SubAgentId("writer".into())],
        };
        assert_eq!(effective_output(&fetcher, None, &[full]), Ok(OutputShape::Intermediate));
    }

    #[test]
    fn output_position_ambiguous_across_pipelines_requires_explicit_value() {
        let fetcher = SubAgentId("fetcher".into());
        let full = PipelineConfig {
            id: PipelineId("revenue_report".into()),
            stages: vec![fetcher.clone(), SubAgentId("writer".into())],
        };
        let quick = PipelineConfig {
            id: PipelineId("quick_fetch".into()),
            stages: vec![fetcher.clone()], // terminal here, non-terminal above
        };
        assert_eq!(
            effective_output(&fetcher, None, &[full, quick]),
            Err(ResolveError::AmbiguousOutput(fetcher))
        );
    }

    #[test]
    fn explicit_output_overrides_position_even_when_it_contradicts_it() {
        let writer = SubAgentId("writer".into());
        // writer is terminal here, yet the author forces `Intermediate` — explicit always wins.
        let full = PipelineConfig {
            id: PipelineId("revenue_report".into()),
            stages: vec![SubAgentId("fetcher".into()), writer.clone()],
        };
        assert_eq!(
            effective_output(&writer, Some(OutputShape::Intermediate), &[full]),
            Ok(OutputShape::Intermediate)
        );
    }

    #[test]
    fn unreferenced_agent_gets_an_inert_default() {
        let ghost = SubAgentId("ghost".into());
        assert_eq!(effective_output(&ghost, None, &[]), Ok(OutputShape::Intermediate));
    }

    // ── §1.4 multiple pipelines reusing one sub-agent ──

    #[tokio::test]
    async fn orchestrator_runs_multiple_pipelines_reusing_a_sub_agent() {
        let fetcher_id = SubAgentId("fetcher".into());
        let writer_id = SubAgentId("writer".into());

        let full = PipelineConfig {
            id: PipelineId("revenue_report".into()),
            stages: vec![fetcher_id.clone(), writer_id.clone()],
        };
        let quick = PipelineConfig {
            id: PipelineId("quick_fetch".into()),
            stages: vec![fetcher_id.clone()], // same fetcher, different pipeline
        };
        let all_pipelines = [full.clone(), quick.clone()];

        // `fetcher` is non-terminal in `revenue_report` but terminal in `quick_fetch` — its
        // position is ambiguous, so an unset `output` fails resolution (§2.4) instead of
        // silently picking one.
        assert_eq!(
            effective_output(&fetcher_id, None, &all_pipelines),
            Err(ResolveError::AmbiguousOutput(fetcher_id.clone()))
        );
        // The author disambiguates with an explicit value.
        let fetcher_output = OutputShape::Intermediate;

        // `writer` is terminal in every pipeline that references it (only `revenue_report`),
        // so the default derives cleanly with no `output` set at all.
        let writer_output = effective_output(&writer_id, None, &all_pipelines).unwrap();
        assert_eq!(writer_output, OutputShape::Final);

        let fetcher: Arc<dyn SubAgent> = Arc::new(ConfiguredAgent::new(
            &SubAgentConfig {
                id: fetcher_id.clone(),
                instruction: "fetch".into(),
                llm: None,
                tools: vec![ToolId::BillRevenue],
                accepts: vec![PayloadKind::Initial],
                output: Some(fetcher_output),
            },
            ScriptedLlm::arc(vec![
                tool_call("query_records"),
                LlmResponse::Message("f1".into()),
                tool_call("query_records"),
                LlmResponse::Message("f2".into()),
            ]),
            vec![Box::new(FakeFetchTool)],
            fetcher_output,
        ));
        let writer: Arc<dyn SubAgent> = Arc::new(ConfiguredAgent::new(
            &SubAgentConfig {
                id: writer_id.clone(),
                instruction: "write".into(),
                llm: None,
                tools: vec![],
                accepts: vec![PayloadKind::Intermediate],
                output: None,
            },
            ScriptedLlm::arc(vec![LlmResponse::Message("SUMMARY".into())]),
            vec![],
            writer_output,
        ));

        let mut agents: HashMap<SubAgentId, Arc<dyn SubAgent>> = HashMap::new();
        agents.insert(fetcher_id, fetcher);
        agents.insert(writer_id, writer);

        let mut orch = Orchestrator::new();
        orch.insert(full.id.clone(), resolve_pipeline(&full, &agents).unwrap());
        orch.insert(quick.id.clone(), resolve_pipeline(&quick, &agents).unwrap());

        let seed = || AgentPayload::Initial(InitialPrompt { prompt: "go".into(), history: vec![] });

        let a = orch.run(&PipelineId("revenue_report".into()), seed()).await.unwrap();
        assert!(matches!(a, AgentPayload::Final(_)));

        let b = orch.run(&PipelineId("quick_fetch".into()), seed()).await.unwrap();
        assert!(matches!(b, AgentPayload::Intermediate(_)));
    }

    #[test]
    fn pipeline_referencing_an_unknown_agent_fails_fast() {
        let agents: HashMap<SubAgentId, Arc<dyn SubAgent>> = HashMap::new();
        let cfg = PipelineConfig {
            id: PipelineId("p".into()),
            stages: vec![SubAgentId("ghost".into())],
        };
        // `matches!` avoids needing `Debug` on the (non-Debug) `Ok` stage vec.
        assert!(matches!(
            resolve_pipeline(&cfg, &agents),
            Err(ResolveError::UnknownAgentRef { .. })
        ));
    }
}
