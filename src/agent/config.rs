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

//! The authored config surface (PART A, normative) and the boot resolution rules.
//!
//! Port of the sub-agent contract, PART A, plus resolution: the [`SubAgentConfig`] an author
//! writes, the [`Provider`] / [`LlmConfig`] model, and the rules that turn config into runnable
//! parts.
//!
//! Those rules are the default-LLM field merge ([`resolve_llm`]), k8s-style secret binding, and
//! position-derived output shaping ([`effective_output`]).
//! Every resolution failure surfaces at boot, before a single LLM call.
//!
//! # References
//!
//! - Sub-agent contract, PART A — `.spec/contract/sub_agent/sub_agent.rs`

#![allow(dead_code)] // groundwork: not every resolution path is wired into boot yet.

use std::collections::HashMap;
use std::fmt;

use crate::agent::payload::PayloadKind;
use crate::agent::tools::ToolId;

// ===========================================================================
// PART A — the authored config model (normative)
// ===========================================================================

/// Stable identity of a sub-agent.
///
/// Namespaces the [`ArtifactKey`](crate::agent::payload::ArtifactKey)s it produces and labels
/// its logs.
/// Author-defined, so a newtype rather than a closed enum.
///
/// # References
///
/// - Payload contract §2.5 — producer namespacing
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

/// A **k8s-style secret reference**: the config carries a *key name*, and the matching entry in
/// the process environment supplies the value.
///
/// Config files never carry a raw key.
/// Binding happens at boot; a referenced key with no environment entry **fails boot**.
///
/// # References
///
/// - Sub-agent contract §2.3 — secret binding
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct SecretRef(pub String);

/// The LLM provider — a *known enum with a `Custom` escape hatch*.
///
/// Known variants carry their default base URL and auth style.
/// `Custom` is the generic OpenAI-compatible `(name, endpoint)` for anything else.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Provider {
    /// Hosted OpenRouter. Default base URL; API key via the fixed `OPENROUTER_API_KEY` secret.
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
    /// The default base URL for known providers.
    ///
    /// # Returns
    ///
    /// Returns `Some(url)` for a known provider, or `None` when the variant already carries its
    /// own `endpoint`.
    pub fn default_base_url(&self) -> Option<&'static str> {
        match self {
            Provider::OpenRouter => Some("https://openrouter.ai/api/v1"),
            Provider::Ollama { .. } => Some("http://localhost:11434/v1"),
            Provider::Custom { .. } => None,
        }
    }

    /// The secret key this provider authenticates with, if any.
    ///
    /// # Returns
    ///
    /// Returns `Some(SecretRef)` for a keyed provider, or `None` for keyless providers (Ollama).
    pub fn secret_ref(&self) -> Option<SecretRef> {
        match self {
            Provider::OpenRouter => Some(SecretRef("OPENROUTER_API_KEY".to_string())),
            Provider::Ollama { .. } => None,
            Provider::Custom { api_key, .. } => api_key.clone(),
        }
    }
}

/// Optional generation parameters.
///
/// Each field merges *independently* over the default LLM, so an agent can nudge `temperature`
/// alone and inherit everything else.
///
/// # References
///
/// - Sub-agent contract §2.1 — per-field merge
#[derive(Clone, PartialEq, Debug, Default)]
pub struct GenerationParams {
    pub temperature: Option<f32>,
    pub top_p: Option<f32>,
    pub max_tokens: Option<u32>,
}

/// A sub-agent's LLM config — **all-optional**, so it states only what differs from the default
/// LLM.
///
/// `provider` is *atomic* (overridden or inherited whole).
/// `model` and each [`GenerationParams`] field merge independently.
#[derive(Clone, PartialEq, Debug, Default)]
pub struct LlmConfig {
    pub provider: Option<Provider>,
    pub model: Option<String>,
    pub params: GenerationParams,
}

/// The output variant a config-defined agent shapes its result into.
///
/// This is *execution-time output shaping* used to build the outgoing payload.
/// It is **not** a static `produces` and participates in no graph validation.
///
/// # References
///
/// - Sub-agent contract §2.4 — output shaping is runtime, not static
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum OutputShape {
    /// Working data for a downstream agent.
    Intermediate,
    /// The user-facing terminal result.
    Final,
}

/// The authored unit: a config-defined sub-agent as *data*.
///
/// Fed to the generic [`ConfiguredAgent`](crate::agent::engine::ConfiguredAgent) engine at
/// resolution.
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
    /// Whether this stage's model **message** is captured as a first-class artifact keyed
    /// `{id}.message` (open-key contract). **Default on** (`true`): a prose stage's report survives
    /// the boundary and the terminal result gains provenance. Set `false` for a tool-only stage
    /// whose message is a throwaway note (e.g. the `fetcher`'s "已取得營收"), so it does not clutter
    /// downstream material or the provenance map.
    pub capture_message: bool,
}

/// A pipeline: a first-class, *ordered* list of sub-agent references.
///
/// A deployment may declare several, and the same sub-agent may appear in more than one —
/// compatibility is a runtime guarantee.
///
/// # References
///
/// - Sub-agent contract §2.4 — compatibility is a runtime guarantee
#[derive(Clone, Debug)]
pub struct PipelineConfig {
    pub id: PipelineId,
    pub stages: Vec<SubAgentId>,
}

// ===========================================================================
// PART B — resolution (advisory: one way to turn config into runnable parts)
// ===========================================================================

/// A boot/resolution failure.
///
/// Every variant is a *fail-fast* condition surfaced before any LLM call, so a deployment never
/// limps along with a half-wired pipeline.
#[derive(Debug, PartialEq, Eq)]
pub enum ResolveError {
    /// A granted [`ToolId`] has no registry entry (§2.2). Mapped from the tool layer's
    /// [`ToolError`](crate::agent::tools::ToolError) during grant resolution.
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
    /// `output` was left unset and the agent's position is ambiguous across the pipelines that
    /// reference it — terminal in one, non-terminal in another (§2.4). The author must set
    /// `output` explicitly to disambiguate.
    AmbiguousOutput(SubAgentId),
}

impl fmt::Display for ResolveError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ResolveError::UnknownTool(t) => write!(f, "no registry entry for tool `{t}`"),
            ResolveError::MissingSecret(k) => write!(f, "secret `{k}` not present in environment"),
            ResolveError::MissingModel(a) => write!(
                f,
                "sub-agent `{a}` has no model and the default LLM sets none"
            ),
            ResolveError::UnknownAgentRef { pipeline, agent } => {
                write!(
                    f,
                    "pipeline `{pipeline}` references unknown sub-agent `{agent}`"
                )
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

/// How hard the model should reason before answering, mapped onto the provider's `reasoning_effort`
/// control.
///
/// Reasoning tokens are billed inside the completion budget but never streamed as content, so a
/// heavy reasoner can silently exhaust `max_tokens` on chain-of-thought a task doesn't need. The
/// mechanical pipeline stages (a data `fetcher`, a transcribing `composer`) run at
/// [`Minimal`](Self::Minimal), while stages that genuinely reason (the `analyst`) keep the
/// provider default.
///
/// The ladder mirrors the vendor's `reasoning_effort` levels. `Option::None` on
/// [`ResolvedLlm::reasoning_effort`] means "send nothing — use the provider default".
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ReasoningEffort {
    /// The smallest reasoning budget the provider offers — for mechanical, no-deliberation stages.
    Minimal,
    /// A low reasoning budget.
    Low,
    /// The provider's medium reasoning budget (typically its default).
    Medium,
    /// A high reasoning budget.
    High,
}

/// The option-free product of resolution: a fully-resolved LLM ready to construct an
/// [`LlmCapability`](crate::agent::payload::LlmCapability).
///
/// See [`OpenAiLlm`](crate::agent::llm::OpenAiLlm) for the concrete capability built from it.
///
/// # References
///
/// - Sub-agent contract §2.1 — LLM resolution
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
    /// The reasoning budget to request, or `None` to leave the provider default. Lowered to
    /// [`ReasoningEffort::Minimal`] for mechanical stages so they don't burn the output budget on
    /// reasoning the task doesn't need (see [`with_reasoning_effort`](Self::with_reasoning_effort)).
    pub reasoning_effort: Option<ReasoningEffort>,
    /// OpenRouter app-attribution URL (`HTTP-Referer` header), or `None`. Without it, requests show
    /// as "Unknown" on the OpenRouter dashboard.
    pub app_url: Option<String>,
    /// OpenRouter app-attribution title (`X-Title` header), or `None`.
    pub app_title: Option<String>,
}

impl ResolvedLlm {
    /// Returns a copy with `reasoning_effort` set — used to give a mechanical stage (fetch,
    /// transcribe) a lower reasoning budget than a reasoning stage built from the same resolution.
    pub fn with_reasoning_effort(&self, effort: ReasoningEffort) -> Self {
        Self {
            reasoning_effort: Some(effort),
            ..self.clone()
        }
    }
}

/// Reads a secret value for `provider` from `env` (a k8s-style key→value map).
///
/// # Arguments
///
/// - `provider`: the provider whose [`SecretRef`] names the key to read.
/// - `env`: the key→value environment map.
///
/// # Returns
///
/// Returns `Ok(Some(value))` when the key is present, or `Ok(None)` for keyless providers.
///
/// # Errors
///
/// - [`ResolveError::MissingSecret`] — a referenced key is absent from `env`.
///
/// # References
///
/// - Sub-agent contract §2.3 — secret binding fails fast
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

/// Resolves a sub-agent's LLM by merging its overrides over the default LLM (the field merge).
///
/// `provider` is atomic — taken whole from the override or inherited.
/// `model` and each generation param merge independently over `default`.
/// Secrets are bound from `env` against the *resolved* provider.
///
/// # Arguments
///
/// - `agent`: the agent being resolved (names the error on a missing model).
/// - `default`: the deployment's default LLM every field falls back to.
/// - `over`: the agent's optional overrides; `None` inherits the default verbatim.
/// - `env`: the key→value environment used to bind the provider's secret.
///
/// # Returns
///
/// Returns a fully-resolved [`ResolvedLlm`] with no optional fields left.
///
/// # Errors
///
/// - [`ResolveError::MissingModel`] — neither the override nor the default sets a model.
/// - [`ResolveError::MissingSecret`] — the resolved provider's secret key is absent from `env`.
///
/// # References
///
/// - Sub-agent contract §2.1 — default-LLM field merge
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
        reasoning_effort: None, // provider default; lowered per mechanical stage in `wiring`
        // The TOML sub-agent config path carries no app attribution; the live pipelines build
        // their `ResolvedLlm` from `LlmDefaults::resolved`, which does.
        app_url: None,
        app_title: None,
    })
}

/// Derives the output shape an agent produces, defaulting from its position across pipelines.
///
/// `configured` (the config's explicit value) always wins.
/// When unset, the shape is derived from `id`'s position across every declared pipeline that
/// references it: terminal in *all* of them ⇒ [`OutputShape::Final`]; non-terminal in *all* of
/// them ⇒ [`OutputShape::Intermediate`].
///
/// An agent that is terminal in one pipeline and non-terminal in another (the
/// `quick_fetch`-style reuse case) has no unambiguous default and **fails resolution**,
/// demanding an explicit `output`.
///
/// An agent referenced by no pipeline is unreachable; its shape is never observed, so it
/// defaults to [`OutputShape::Intermediate`] rather than erroring.
///
/// # Arguments
///
/// - `id`: the agent whose output shape is being resolved.
/// - `configured`: the config's explicit shape, if any — it always wins.
/// - `pipelines`: every declared pipeline, scanned for `id`'s position.
///
/// # Returns
///
/// Returns the resolved [`OutputShape`].
///
/// # Errors
///
/// - [`ResolveError::AmbiguousOutput`] — `id` is terminal in one pipeline and non-terminal in
///   another, with no explicit `output` to disambiguate.
///
/// # References
///
/// - Sub-agent contract §2.4 — position-derived output shaping
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

#[cfg(test)]
mod tests {
    use super::*;

    fn default_llm() -> ResolvedLlm {
        ResolvedLlm {
            provider: Provider::OpenRouter,
            base_url: "https://openrouter.ai/api/v1".into(),
            model: "google/gemini-flash".into(),
            temperature: 0.7,
            top_p: 0.9,
            max_tokens: 1024,
            api_key: Some("default-key".into()),
            reasoning_effort: None,
            app_url: None,
            app_title: None,
        }
    }

    fn env(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    // ── §2.1 default-LLM field merge ──

    #[test]
    fn merge_overrides_only_model_and_inherits_the_rest() {
        let over = LlmConfig {
            model: Some("anthropic/claude-sonnet-5".into()),
            ..Default::default()
        };
        let e = env(&[("OPENROUTER_API_KEY", "sk-xyz")]);
        let r = resolve_llm(
            &SubAgentId("writer".into()),
            &default_llm(),
            Some(&over),
            &e,
        )
        .unwrap();

        assert_eq!(r.model, "anthropic/claude-sonnet-5"); // overridden
        assert_eq!(r.provider, Provider::OpenRouter); // inherited (atomic)
        assert_eq!(r.temperature, 0.7); // inherited per-field
        assert_eq!(r.api_key.as_deref(), Some("sk-xyz")); // bound from env
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
        let r = resolve_llm(
            &SubAgentId("local".into()),
            &default_llm(),
            Some(&over),
            &env(&[]),
        )
        .unwrap();
        assert_eq!(r.base_url, "http://localhost:11434/v1");
        assert_eq!(r.api_key, None);
        assert_eq!(r.model, "google/gemini-flash"); // model still inherited
    }

    // ── §2.3 secrets ──

    #[test]
    fn missing_secret_fails_fast() {
        let err =
            resolve_llm(&SubAgentId("a".into()), &default_llm(), None, &env(&[])).unwrap_err();
        assert_eq!(
            err,
            ResolveError::MissingSecret("OPENROUTER_API_KEY".into())
        );
    }

    // ── §2.4 output-shape default: derived from position, ambiguous ⇒ explicit required ──

    #[test]
    fn output_derives_from_position_and_ambiguity_requires_an_explicit_value() {
        let fetcher = SubAgentId("fetcher".into());
        let writer = SubAgentId("writer".into());
        let report = PipelineConfig {
            id: PipelineId("report".into()),
            stages: vec![fetcher.clone(), writer.clone()],
        };
        // terminal everywhere ⇒ Final; non-terminal everywhere ⇒ Intermediate.
        assert_eq!(
            effective_output(&writer, None, std::slice::from_ref(&report)),
            Ok(OutputShape::Final)
        );
        assert_eq!(
            effective_output(&fetcher, None, std::slice::from_ref(&report)),
            Ok(OutputShape::Intermediate)
        );
        // reused terminally elsewhere ⇒ ambiguous ⇒ must be explicit.
        let quick = PipelineConfig {
            id: PipelineId("quick_fetch".into()),
            stages: vec![fetcher.clone()],
        };
        assert_eq!(
            effective_output(&fetcher, None, &[report, quick]),
            Err(ResolveError::AmbiguousOutput(fetcher))
        );
    }

    #[test]
    fn explicit_output_overrides_position() {
        let writer = SubAgentId("writer".into());
        let report = PipelineConfig {
            id: PipelineId("report".into()),
            stages: vec![SubAgentId("fetcher".into()), writer.clone()],
        };
        // writer is terminal here, yet the author forces Intermediate — explicit wins.
        assert_eq!(
            effective_output(&writer, Some(OutputShape::Intermediate), &[report]),
            Ok(OutputShape::Intermediate)
        );
    }
}
