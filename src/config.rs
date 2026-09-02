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

//! Top-level application config.
//!
//! Loads a single TOML manifest (`config/config.toml` by default) that
//! points at the prompt Markdown files used by the agent and greeting
//! generators.
//!
//! Every relative path inside the manifest is resolved against the
//! **parent directory of the manifest file itself**, not the process CWD.
//! So container mounting can be much more straightforward.
//!
//! ## Example
//!
//! ```ignore
//! use datacenter_agent::config::AppConfig;
//!
//! let app = AppConfig::load("config/config.toml")?;
//! let agent_prompt = app.get_prompt_by_id("agent_system")?;
//! ```

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context, Result};
use serde::Deserialize;
use tracing::{debug, info};

/// Schema versions this loader knows how to parse. Bump in lockstep
/// with breaking changes to the on-disk layout so old binaries refuse
/// to load configs they cannot interpret.
const SUPPORTED_VERSION: u32 = 1;

// ──── helpers ────

/// Resolve, read, and validate a single prompt body referenced by the
/// manifest.
///
/// Resolves `prompt_ref.file` against `root`, and reads it.
///
/// If the read prompt file is empty or contains only whitespaces,
/// the prompt will be rejected.
///
/// # Errors
///
/// Returns `Err` if the file cannot be read, or if its body is empty.
fn load_prompt(root: &Path, id: &str, prompt_ref: &PromptRef) -> Result<String> {
    let file = resolve_relative(root, &prompt_ref.file);
    let body = std::fs::read_to_string(&file)
        .with_context(|| format!("read prompt `{id}` from {}", file.display()))?;
    if body.trim().is_empty() {
        return Err(anyhow!(
            "prompt `{id}` at {} is empty — refusing to ship an empty system message",
            file.display()
        ));
    }
    debug!(id, path = %file.display(), bytes = body.len(), "app config: prompt loaded");
    Ok(body)
}

/// Resolve and read the `/report` HTML template body referenced by the manifest.
///
/// Resolves `[report].template` (or [`default_report_template`] when the section is absent) against
/// `root`, and reads it. Placeholder validation is deferred to boot ([`AppState::new`]) so the
/// config layer stays free of pipeline internals.
///
/// # Errors
///
/// Returns `Err` if the file cannot be read, or if its body is empty.
///
/// [`AppState::new`]: crate::appstate::AppState::new
fn load_report_template(root: &Path, report: Option<&ReportManifest>) -> Result<String> {
    let rel = report
        .map(|r| r.template.clone())
        .unwrap_or_else(default_report_template);
    let file = resolve_relative(root, &rel);
    let body = std::fs::read_to_string(&file)
        .with_context(|| format!("read report template from {}", file.display()))?;
    if body.trim().is_empty() {
        return Err(anyhow!(
            "report template at {} is empty — refusing to ship an empty report template",
            file.display()
        ));
    }
    debug!(path = %file.display(), bytes = body.len(), "app config: report template loaded");
    Ok(body)
}

/// Join `p` to `root` if relative, pass through if already absolute.
fn resolve_relative(root: &Path, p: &Path) -> PathBuf {
    if p.is_absolute() {
        p.to_path_buf()
    } else {
        root.join(p)
    }
}

// ──── raw schema ────
//
// `deny_unknown_fields` everywhere so a typo (`routng = ...`,
// `[promts.agent_system]`) fails the boot loudly instead of silently
// falling back to defaults.

/// Raw `config.toml` body, before path resolution / Markdown loading.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    /// Schema version. Currently `1`.
    version: u32,
    /// Prompt id to Markdown file path map.
    #[serde(default)]
    prompts: BTreeMap<String, PromptRef>,
    /// Optional runtime capability-pack references and assembly.
    #[serde(default)]
    runtime: Option<RuntimeManifest>,
    /// Optional `/insight` pipeline tool grants (defaults applied when absent).
    #[serde(default)]
    insight: Option<InsightManifest>,
    /// Optional `/ss-chat` pipeline tool grants (defaults applied when absent).
    #[serde(default)]
    ss_chat: Option<SsChatManifest>,
    /// Optional `/report` pipeline assets (the HTML template; default path applied when absent).
    #[serde(default)]
    report: Option<ReportManifest>,
    /// Optional `[server]` section (ingress policy; defaults applied when absent).
    #[serde(default)]
    server: Option<ServerManifest>,
    /// Optional `[identity]` section (Falcon permissions endpoint policy).
    #[serde(default)]
    identity: Option<IdentityConfig>,
    /// Optional `[authz]` section (permission and intent mapping tables).
    #[serde(default)]
    authz: Option<AuthzConfig>,
}

/// `[authz]`: the two mapping tables the authorization gate intersects
/// (S-RUNTIME-SEC-02 FR-003).
///
/// The gate is `boot grant ∩ permission grant ∩ intent required tools`. Intersecting all
/// three matters: a finance-only user asking a member question still holds a non-empty
/// narrowed set (their finance tools), so a "is the narrowed set non-empty" rule would
/// wave them through. Anything either table fails to cover is denied, and an uncovered
/// intent is a *startup* failure rather than a runtime surprise.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AuthzConfig {
    /// Falcon permission code to the MCP data tools it unlocks. A code mapped to an empty
    /// list grants nothing (e.g. `engproj`, which has no MCP endpoint yet).
    pub permission_tools: BTreeMap<String, Vec<String>>,
    /// Runtime intent to the tools that intent *requires*. Must cover the runtime's whole
    /// intent allowlist, `unknown` included.
    pub intent_tools: BTreeMap<String, Vec<String>>,
    /// Falcon permission codes any one of which unlocks the SS chat pipeline's full grant.
    ///
    /// The SS route cannot use the intent-based gate: it deliberately disables intent filtering
    /// (every SS question resolves to `unknown` under the EV-charging intent pack), so its
    /// authorization keys off these codes instead. Empty means deny-all — fail-safe for a config
    /// that predates the SS route.
    #[serde(default)]
    pub ss_chat_permissions: Vec<String>,
}

impl AuthzConfig {
    /// Boot-time validation of both tables.
    ///
    /// # Errors
    ///
    /// Returns `Err` when the intent allowlist is not fully covered, or when either table
    /// names a tool the MCP server does not advertise.
    pub fn validate(&self, intent_allowlist: &[String], advertised: &[String]) -> Result<()> {
        for intent in intent_allowlist {
            if !self.intent_tools.contains_key(intent) {
                anyhow::bail!(
                    "config_error: intent `{intent}` has no row in [authz.intent_tools]; \
                     every intent in the runtime allowlist needs one, so a gap fails boot \
                     instead of becoming a silent default-deny at request time"
                );
            }
        }
        for (intent, tools) in &self.intent_tools {
            reject_unadvertised(tools, advertised, &format!("[authz.intent_tools].{intent}"))?;
        }
        for (code, tools) in &self.permission_tools {
            reject_unadvertised(
                tools,
                advertised,
                &format!("[authz.permission_tools].{code}"),
            )?;
        }
        Ok(())
    }
}

/// `[identity]`: how the runtime reaches Falcon's permissions endpoint
/// (S-RUNTIME-SEC-02 FR-001).
///
/// There is deliberately **no** `enabled` switch. D3 forbids a feature flag, so the
/// identity layer is unconditional in the delivered binary (spec D-011); forward
/// compatibility during rollout is provided by the *previous* binary, which ignores the
/// unknown header. `deny_unknown_fields` therefore rejects `enabled = ...` outright
/// rather than letting a default-config deploy silently ship with RBAC off.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct IdentityConfig {
    /// Base URL of the Falcon API, e.g. `https://falcon.example.com`. The runtime only
    /// ever appends `/api/auth/me/permissions` to it.
    ///
    /// Deliberately **optional and without a checked-in default**: a shipped default would let a
    /// deployment that forgets `FALCON_API_BASE_URL` silently verify production users' tokens
    /// against whatever host the repo happened to carry. It must be supplied by config here or by
    /// the `FALCON_API_BASE_URL` env var; absence is a boot failure (finding #1), the same
    /// treatment a missing pepper gets.
    #[serde(default)]
    pub base_url: Option<String>,
    /// How long a successful permission lookup is reused, in milliseconds. Also the upper
    /// bound on how long a Falcon-side permission revocation takes to take effect.
    pub positive_ttl_ms: std::num::NonZeroU64,
    /// How long a failed lookup is replayed from cache, in milliseconds. Bounds the
    /// amplification from a consumer stuck resending one dead token.
    pub negative_ttl_ms: std::num::NonZeroU64,
    /// Per-request timeout for the permissions call, in milliseconds.
    pub request_timeout_ms: std::num::NonZeroU64,
}

/// `IdentityConfig` after boot resolution: `base_url` is guaranteed present (env or config,
/// else boot already failed), so nothing downstream has to handle its absence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedIdentityConfig {
    /// Resolved Falcon API base URL.
    pub base_url: String,
    /// See [`IdentityConfig::positive_ttl_ms`].
    pub positive_ttl_ms: std::num::NonZeroU64,
    /// See [`IdentityConfig::negative_ttl_ms`].
    pub negative_ttl_ms: std::num::NonZeroU64,
    /// See [`IdentityConfig::request_timeout_ms`].
    pub request_timeout_ms: std::num::NonZeroU64,
}

/// Optional `[server]` section: HTTP ingress policy.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct ServerManifest {
    /// Optional `[server.rate_limit]` global burst-admission policy.
    #[serde(default)]
    rate_limit: Option<RateLimitConfig>,
}

/// `[server.rate_limit]`: opt-in process-local global burst limiter for the
/// expensive routes (S-RUNTIME-SEC-01 FR-005). Disabled unless the section is
/// present and `enabled = true`; when the section is present, the policy must
/// be explicit — there is no undocumented crate preset (FR-004).
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RateLimitConfig {
    /// Whether the limiter is attached at all. Default: disabled.
    #[serde(default)]
    pub enabled: bool,
    /// Token-bucket burst size (admissions available at once).
    pub burst_size: std::num::NonZeroU32,
    /// One admission token refills every this many milliseconds.
    pub refill_period_ms: std::num::NonZeroU64,
    /// `[server.rate_limit.per_actor]`: the inner, identity-keyed layer
    /// (S-RUNTIME-SEC-02 FR-005). Absent until the identity layer ships.
    #[serde(default)]
    pub per_actor: Option<PerActorRateLimitConfig>,
}

/// `[server.rate_limit.per_actor]`: the inner token bucket, keyed by `actor_key`.
///
/// Sits *inside* the outer global bucket and *after* identity resolution, because it
/// needs the `actor_key`. The outer layer therefore stays where it is — it is the only
/// thing bounding traffic whose identity never resolves (ERR-002/003/004/008 all finish
/// before this layer is reached).
///
/// Like the outer policy, every value is explicit: there is no crate preset, and there is
/// no `enabled` switch — an opt-out here would be a bypass of D3.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct PerActorRateLimitConfig {
    /// Token-bucket burst size per actor.
    pub burst_size: std::num::NonZeroU32,
    /// One admission token refills every this many milliseconds, per actor.
    pub refill_period_ms: std::num::NonZeroU64,
    /// Upper bound on tracked actors. Reaching it evicts the least recently used bucket;
    /// eviction only returns that actor to a full allowance and never widens the outer cap.
    pub max_tracked_actors: std::num::NonZeroU32,
}

impl Default for RateLimitConfig {
    /// The absent-section shape: disabled, with placeholder policy values
    /// that are never consulted while `enabled` is false.
    fn default() -> Self {
        Self {
            enabled: false,
            burst_size: std::num::NonZeroU32::MIN,
            refill_period_ms: std::num::NonZeroU64::new(1000).expect("1000 is non-zero"),
            per_actor: None,
        }
    }
}

/// Optional `[report]` section: assets for the `/report` HTML pipeline.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReportManifest {
    /// Path to the HTML report template the `renderer` fills (relative to the manifest's parent
    /// directory, or absolute). Must contain the `__REPORT_DATA_JSON__` placeholder.
    #[serde(default = "default_report_template")]
    template: PathBuf,
    /// Optional `[report.grants]`: the report pipeline's own tool ceiling
    /// (S-RUNTIME-SEC-02 D-010). Absent means the legacy behavior of sharing
    /// `[insight.grants].fetcher`.
    #[serde(default)]
    grants: Option<ReportGrants>,
}

/// `[report.grants]`: the report pipeline's own tool ceiling.
///
/// Separate from `[insight.grants].fetcher` on purpose. The shared grant lists five tools
/// and its comment deliberately excludes `bill_member_analysis`, while the report prompt
/// instructs the model to call six — so the sixth was never advertised to it. Giving the
/// report its own ceiling fixes that mismatch without widening insight.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ReportGrants {
    /// The report fetcher's granted data tools, by MCP wire name.
    pub fetcher: Vec<String>,
}

impl ReportGrants {
    /// Boot-time validation that every granted name is actually advertised.
    ///
    /// # Errors
    ///
    /// Returns `Err` naming the first tool the MCP server does not advertise.
    pub fn validate(&self, advertised: &[String]) -> Result<()> {
        reject_unadvertised(&self.fetcher, advertised, "[report.grants].fetcher")
    }
}

/// Fail on the first granted name the MCP server does not advertise.
///
/// A typo here would otherwise become a permanent, invisible default-deny for that topic:
/// the tool simply never resolves, and the user is told they lack permission.
fn reject_unadvertised(tools: &[String], advertised: &[String], where_: &str) -> Result<()> {
    for tool in tools {
        if !advertised.iter().any(|name| name == tool) {
            anyhow::bail!(
                "config_error: {where_} names `{tool}`, which the MCP server does not \
                 advertise; advertised tools are: {}",
                advertised.join(", ")
            );
        }
    }
    Ok(())
}

/// The default report template path, used when `[report]` (or its `template`) is omitted.
fn default_report_template() -> PathBuf {
    PathBuf::from("report_template/report.html")
}

/// Optional `[insight]` section: per-sub-agent tool grants for the `/insight` pipeline.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct InsightManifest {
    #[serde(default = "default_insight_grants")]
    grants: InsightGrantsManifest,
}

/// `[insight.grants]`: the tools each `/insight` sub-agent exposes to its LLM, by wire name.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct InsightGrantsManifest {
    #[serde(default = "default_fetcher_grant")]
    fetcher: Vec<String>,
    #[serde(default = "default_charter_grant")]
    charter: Vec<String>,
}

/// The fetcher's default grant: `["*"]` — every tool the MCP server advertises, so new datacenter
/// tools are wired without a code change.
fn default_fetcher_grant() -> Vec<String> {
    vec!["*".to_string()]
}

/// The charter's default grant: the built-in `emit_chart` sink only.
fn default_charter_grant() -> Vec<String> {
    vec!["emit_chart".to_string()]
}

fn default_insight_grants() -> InsightGrantsManifest {
    InsightGrantsManifest {
        fetcher: default_fetcher_grant(),
        charter: default_charter_grant(),
    }
}

/// Optional `[ss_chat]` section: per-sub-agent tool grants for the `/ss-chat` pipeline.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct SsChatManifest {
    #[serde(default = "default_ss_chat_grants")]
    grants: SsChatGrantsManifest,
}

/// `[ss_chat.grants]`: the tools each `/ss-chat` sub-agent exposes to its LLM, by wire name.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct SsChatGrantsManifest {
    #[serde(default = "default_ss_fetcher_grant")]
    fetcher: Vec<String>,
    #[serde(default = "default_charter_grant")]
    charter: Vec<String>,
}

/// The six `ss_*` investor-platform tools, the SS fetcher's default grant.
///
/// Unlike the `/insight` fetcher this is an **explicit list, never `["*"]`**: the same MCP server
/// advertises the EV-charging tools too, and a wildcard would hand the SS fetcher tools its prompt
/// never describes. A new `ss_*` tool is therefore a one-line config edit, by design.
const SS_FETCHER_TOOLS: [&str; 6] = [
    "ss_sunshine_hours",
    "ss_energy_storage",
    "ss_power_wheeling",
    "ss_power_pipeline",
    "ss_btm_projects",
    "ss_ftm_projects",
];

/// The SS fetcher's default grant: the six `ss_*` tools ([`SS_FETCHER_TOOLS`]).
fn default_ss_fetcher_grant() -> Vec<String> {
    SS_FETCHER_TOOLS.iter().map(|t| (*t).to_string()).collect()
}

fn default_ss_chat_grants() -> SsChatGrantsManifest {
    SsChatGrantsManifest {
        fetcher: default_ss_fetcher_grant(),
        charter: default_charter_grant(),
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct PromptRef {
    /// Path to a Markdown file containing the prompt body (relative to
    /// the manifest's parent directory, or absolute).
    file: PathBuf,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct RuntimeManifest {
    /// Runtime intent config path.
    intents: PathBuf,
    /// Runtime lexicon config path.
    lexicon: PathBuf,
    /// Runtime thresholds config path.
    thresholds: PathBuf,
    /// Runtime injection config path.
    injection: PathBuf,
    /// Runtime input pipeline assembly.
    input: RuntimeInputManifest,
    /// Runtime answer policy assembly.
    answer_policy: RuntimeAnswerPolicyManifest,
    /// Optional LLM normalizer assembly.
    llm_normalizer: RuntimeLlmNormalizerManifest,
    /// Runtime memory assembly.
    memory: RuntimeMemoryManifest,
    /// Runtime audit assembly.
    audit: RuntimeAuditManifest,
    /// Runtime guardrail assembly.
    guardrails: RuntimeGuardrailsManifest,
    /// Runtime slot extractor assembly.
    slots: RuntimeSlotsManifest,
    /// Runtime eval assembly.
    eval: RuntimeEvalManifest,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct RuntimeInputManifest {
    /// Ordered input pipeline stage ids.
    input_stages: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct RuntimeAnswerPolicyManifest {
    /// Answer policy backend id.
    backend: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct RuntimeLlmNormalizerManifest {
    /// Whether the LLM normalizer is enabled.
    enabled: bool,
    /// LLM normalizer backend id.
    backend: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct RuntimeMemoryManifest {
    /// Whether server memory is enabled.
    enabled: bool,
    /// Memory backend id.
    backend: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct RuntimeAuditManifest {
    /// Audit sink id.
    sink: String,
    /// Audit failure policy id.
    failure_policy: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct RuntimeGuardrailsManifest {
    /// Enabled guardrail ids.
    enabled: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct RuntimeSlotsManifest {
    /// Enabled slot extractor ids.
    extractors: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct RuntimeEvalManifest {
    /// Enabled pipeline evaluator ids.
    pipeline_evaluators: Vec<String>,
    /// Enabled response evaluator ids.
    response_evaluators: Vec<String>,
    /// Eval fixture path.
    fixtures: PathBuf,
    /// Response baseline path.
    baseline: PathBuf,
}

// ──── resolved, runtime-ready config ────

/// Fully-resolved application config.
///
/// All paths are converted to absolute (resolved against the manifest's directory)
/// and every referenced Markdown body has been read into memory.
#[derive(Debug, Clone)]
pub struct AppConfig {
    /// Directory the manifest was loaded from.
    /// Useful for diagnostics and for resolving any further relative paths added later.
    pub root: PathBuf,
    /// Loaded prompt bodies KV map.
    pub prompts: BTreeMap<String, String>,
    /// Optional runtime config references and assembly.
    pub runtime: Option<RuntimeRefs>,
    /// Resolved `/insight` pipeline tool grants (defaults when `[insight]` is absent).
    pub insight_grants: InsightGrants,
    /// Resolved `/ss-chat` pipeline tool grants (defaults when `[ss_chat]` is absent).
    pub ss_chat_grants: SsChatGrants,
    /// The `/report` HTML template body, read at load from `[report].template` (or its default
    /// path). Holds the `__REPORT_DATA_JSON__` placeholder the `renderer` fills.
    pub report_template: String,
    /// Resolved `[server.rate_limit]` policy (disabled default when absent).
    pub rate_limit: RateLimitConfig,
    /// Resolved `[identity]` policy. `None` when the section is absent.
    pub identity: Option<IdentityConfig>,
    /// Resolved `[authz]` mapping tables. `None` when the section is absent.
    pub authz: Option<AuthzConfig>,
    /// Resolved `[report.grants]` ceiling. `None` when absent (legacy shared-grant behavior).
    pub report_grants: Option<ReportGrants>,
}

/// Resolved `/insight` pipeline tool grants — which tools each sub-agent exposes to its LLM.
///
/// Names are matched at boot against the MCP server's advertised set (fail-fast on a typo) or the
/// built-in code-backed tools (e.g. `emit_chart`). A grant of `["*"]` means "every tool the MCP
/// server advertises", so new datacenter tools are wired without a code change.
#[derive(Debug, Clone)]
pub struct InsightGrants {
    /// The fetcher's granted data tools (MCP wire names, or `["*"]` for all discovered).
    pub fetcher: Vec<String>,
    /// The charter's granted tools (normally the built-in `emit_chart` sink).
    pub charter: Vec<String>,
}

impl Default for InsightGrants {
    fn default() -> Self {
        Self {
            fetcher: default_fetcher_grant(),
            charter: default_charter_grant(),
        }
    }
}

/// Resolved `/ss-chat` pipeline tool grants — which tools each 星星電力 sub-agent exposes to its
/// LLM.
///
/// Resolved and boot-validated exactly like [`InsightGrants`]; only the defaults differ. The
/// fetcher's default is the six `ss_*` investor-platform tools ([`SS_FETCHER_TOOLS`]) rather than
/// the `["*"]` wildcard, because the same MCP server also advertises the EV-charging tools.
#[derive(Debug, Clone)]
pub struct SsChatGrants {
    /// The SS fetcher's granted data tools (MCP wire names; normally the six `ss_*` tools).
    pub fetcher: Vec<String>,
    /// The SS charter's granted tools (normally the built-in `emit_chart` sink).
    pub charter: Vec<String>,
}

impl Default for SsChatGrants {
    fn default() -> Self {
        Self {
            fetcher: default_ss_fetcher_grant(),
            charter: default_charter_grant(),
        }
    }
}

/// Resolved runtime references and assembly from the host config.
#[derive(Debug, Clone)]
pub struct RuntimeRefs {
    /// Runtime intent config path.
    pub intents: PathBuf,
    /// Runtime lexicon config path.
    pub lexicon: PathBuf,
    /// Runtime thresholds config path.
    pub thresholds: PathBuf,
    /// Runtime injection config path.
    pub injection: PathBuf,
    /// Ordered input pipeline stage ids.
    pub input_stages: Vec<String>,
    /// Answer policy backend id.
    pub answer_policy_backend: String,
    /// Whether the LLM normalizer is enabled.
    pub llm_normalizer_enabled: bool,
    /// LLM normalizer backend id.
    pub llm_normalizer_backend: String,
    /// Whether server memory is enabled.
    pub memory_enabled: bool,
    /// Memory backend id.
    pub memory_backend: String,
    /// Audit sink id.
    pub audit_sink: String,
    /// Audit failure policy id.
    pub audit_failure_policy: String,
    /// Enabled guardrail ids.
    pub guardrails: Vec<String>,
    /// Enabled slot extractor ids.
    pub slot_extractors: Vec<String>,
    /// Enabled pipeline evaluator ids.
    pub pipeline_evaluators: Vec<String>,
    /// Enabled response evaluator ids.
    pub response_evaluators: Vec<String>,
    /// Eval fixture path.
    pub eval_fixtures: PathBuf,
    /// Response baseline path.
    pub response_baseline: PathBuf,
}

impl RuntimeRefs {
    fn resolve(root: &Path, manifest: RuntimeManifest) -> Self {
        Self {
            intents: resolve_relative(root, &manifest.intents),
            lexicon: resolve_relative(root, &manifest.lexicon),
            thresholds: resolve_relative(root, &manifest.thresholds),
            injection: resolve_relative(root, &manifest.injection),
            input_stages: manifest.input.input_stages,
            answer_policy_backend: manifest.answer_policy.backend,
            llm_normalizer_enabled: manifest.llm_normalizer.enabled,
            llm_normalizer_backend: manifest.llm_normalizer.backend,
            memory_enabled: manifest.memory.enabled,
            memory_backend: manifest.memory.backend,
            audit_sink: manifest.audit.sink,
            audit_failure_policy: manifest.audit.failure_policy,
            guardrails: manifest.guardrails.enabled,
            slot_extractors: manifest.slots.extractors,
            pipeline_evaluators: manifest.eval.pipeline_evaluators,
            response_evaluators: manifest.eval.response_evaluators,
            eval_fixtures: resolve_relative(root, &manifest.eval.fixtures),
            response_baseline: resolve_relative(root, &manifest.eval.baseline),
        }
    }
}

impl AppConfig {
    /// Parse the manifest at `path` and eagerly read every referenced
    /// Markdown body so the rest of the binary never has to touch the
    /// filesystem for prompts again.
    ///
    /// # Errors
    ///
    /// Returns `Err` if:
    /// - the manifest file cannot be read or parsed as TOML
    /// - referenced Markdown file cannot be read
    pub fn load(path: impl AsRef<Path>) -> Result<Self> {
        // `path` is now `Path`
        let path = path.as_ref();

        // Read the app config TOML file
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("read app config {}", path.display()))?;

        // Parse the app config TOML file
        let manifest: Manifest = toml::from_str(&text)
            .with_context(|| format!("parse app config {}", path.display()))?;

        // Check the version
        if manifest.version != SUPPORTED_VERSION {
            return Err(anyhow!(
                "{}: unsupported config version {} (this binary expects {})",
                path.display(),
                manifest.version,
                SUPPORTED_VERSION
            ));
        }

        // `root` is the parent directory of the manifest file
        let root = path
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."));

        // Load prompts
        let prompts = manifest
            .prompts
            .iter()
            .map(|(id, prompt_ref)| Ok((id.clone(), load_prompt(&root, id, prompt_ref)?)))
            .collect::<Result<BTreeMap<_, _>>>()?;

        let runtime = manifest
            .runtime
            .map(|runtime| RuntimeRefs::resolve(&root, runtime));

        let insight_grants = manifest
            .insight
            .map(|insight| InsightGrants {
                fetcher: insight.grants.fetcher,
                charter: insight.grants.charter,
            })
            .unwrap_or_default();

        let ss_chat_grants = manifest
            .ss_chat
            .map(|ss_chat| SsChatGrants {
                fetcher: ss_chat.grants.fetcher,
                charter: ss_chat.grants.charter,
            })
            .unwrap_or_default();

        let report_template = load_report_template(&root, manifest.report.as_ref())?;

        let report_grants = manifest.report.as_ref().and_then(|r| r.grants.clone());

        let rate_limit = manifest
            .server
            .and_then(|server| server.rate_limit)
            .unwrap_or_default();

        let identity = manifest.identity;

        let authz = manifest.authz;

        // Log the loaded config
        info!(
            root = %root.display(),
            prompts = prompts.len(),
            "app config loaded"
        );

        Ok(Self {
            root,
            prompts,
            runtime,
            insight_grants,
            ss_chat_grants,
            report_template,
            rate_limit,
            identity,
            authz,
            report_grants,
        })
    }

    /// Look up a loaded prompt body by id.
    ///
    /// # Errors
    ///
    /// Returns `Err` with a contextual message if the id is missing —
    /// the caller can `.with_context()` further if more detail is
    /// useful at the call site.
    pub fn get_prompt_by_id(&self, id: &str) -> Result<&str> {
        self.prompts
            .get(id)
            .map(String::as_str)
            .with_context(|| format!("prompt `{id}` missing from app config"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_loads_runtime_refs_from_default_manifest() {
        let cfg = AppConfig::load("config/config.toml").expect("config should load");
        let runtime = cfg.runtime.expect("runtime refs should be configured");

        assert!(runtime.intents.ends_with("runtime/intents.toml"));
        assert!(runtime.lexicon.ends_with("runtime/lexicon.toml"));
        assert!(runtime.thresholds.ends_with("runtime/thresholds.toml"));
        assert!(runtime.injection.ends_with("runtime/injection.toml"));
        assert_eq!(
            runtime.input_stages,
            ["normalize", "input_guard", "injection", "intent", "slots"]
        );
        assert_eq!(runtime.answer_policy_backend, "rule");
        assert!(!runtime.llm_normalizer_enabled);
        assert_eq!(runtime.llm_normalizer_backend, "disabled");
        assert!(runtime.memory_enabled);
        assert_eq!(runtime.memory_backend, "in-memory");
        assert_eq!(runtime.audit_sink, "tracing");
        assert_eq!(runtime.audit_failure_policy, "fail-open");
        assert_eq!(
            runtime.guardrails,
            ["injection", "input_guard", "answer_policy"]
        );
        assert_eq!(
            runtime.slot_extractors,
            ["time_range", "metric", "asset", "rank_limit"]
        );
        assert_eq!(runtime.pipeline_evaluators, ["pipeline-deterministic"]);
        assert_eq!(
            runtime.response_evaluators,
            ["response-baseline", "llm-judge"]
        );
        assert!(runtime.eval_fixtures.ends_with("runtime/evals/inputs.json"));
        assert!(runtime
            .response_baseline
            .ends_with("runtime/evals/response-baseline.json"));
    }

    #[test]
    fn config_loads_insight_grants() {
        let cfg = AppConfig::load("config/config.toml").expect("config should load");
        // The fetcher enumerates the datacenter analytics tools explicitly; the charter gets the
        // code-backed `emit_chart` sink.
        assert_eq!(
            cfg.insight_grants.fetcher,
            [
                "bill_revenue",
                "bill_charge",
                "member_analysis",
                "business_metrics",
                "station_revenue_ranking",
            ]
        );
        assert_eq!(cfg.insight_grants.charter, ["emit_chart"]);
    }

    #[test]
    fn config_loads_the_report_template_with_the_data_placeholder() {
        let cfg = AppConfig::load("config/config.toml").expect("config should load");
        // The template body is read eagerly and carries the one placeholder the renderer fills.
        assert!(cfg.report_template.contains("__REPORT_DATA_JSON__"));
        // The design tokens are baked in (no leftover token placeholder).
        assert!(!cfg.report_template.contains("__REPORT_TOKENS__"));
    }

    /// T02 / spec S1: the `[identity]` section carries the Falcon permissions
    /// endpoint policy. It deliberately has **no** `enabled` switch — D3 forbids a
    /// feature flag, so the identity layer is unconditional in this binary (D-011).
    #[test]
    fn identity_section_parses_its_policy_and_rejects_an_enabled_switch() {
        let manifest: Manifest = toml::from_str(
            r#"
version = 1

[identity]
base_url = "https://falcon.example.com"
positive_ttl_ms = 60000
negative_ttl_ms = 10000
request_timeout_ms = 5000
"#,
        )
        .expect("identity section should parse");

        let identity = manifest
            .identity
            .expect("inline manifest declares the section");
        assert_eq!(
            identity.base_url.as_deref(),
            Some("https://falcon.example.com")
        );
        assert_eq!(identity.positive_ttl_ms.get(), 60_000);
        assert_eq!(identity.negative_ttl_ms.get(), 10_000);
        assert_eq!(identity.request_timeout_ms.get(), 5_000);

        // D-011: an `enabled` key is not part of the schema, so `deny_unknown_fields`
        // rejects it rather than silently letting a deploy ship RBAC turned off.
        let with_switch = toml::from_str::<Manifest>(
            r#"
version = 1

[identity]
enabled = false
base_url = "https://falcon.example.com"
positive_ttl_ms = 60000
negative_ttl_ms = 10000
request_timeout_ms = 5000
"#,
        );
        assert!(
            with_switch.is_err(),
            "`[identity].enabled` must be rejected: D3 forbids a feature flag"
        );
    }

    /// T02 / spec S1: the shipped `config/config.toml` must declare `[identity]`, and its
    /// values are the ones the spec pins (60s positive TTL, 10s negative TTL, 5s timeout).
    #[test]
    fn shipped_config_declares_the_identity_policy() {
        let cfg = AppConfig::load("config/config.toml").expect("config should load");
        let identity = cfg
            .identity
            .expect("config/config.toml must declare an [identity] section");

        // No checked-in default (finding #1): base_url comes from FALCON_API_BASE_URL at boot.
        assert_eq!(identity.base_url, None);
        assert_eq!(identity.positive_ttl_ms.get(), 60_000);
        assert_eq!(identity.negative_ttl_ms.get(), 10_000);
        assert_eq!(identity.request_timeout_ms.get(), 5_000);
    }

    /// T02 / spec S1: the shipped config must enable the outer limiter and declare an
    /// explicit inner per-actor policy. ERR-009 makes the outer limiter a *required*
    /// deployment prerequisite once the identity layer exists, and the inner layer has no
    /// opt-out (an opt-out would be a bypass of D3).
    #[test]
    fn shipped_config_enables_both_rate_limit_layers_explicitly() {
        let cfg = AppConfig::load("config/config.toml").expect("config should load");

        assert!(
            cfg.rate_limit.enabled,
            "the outer global limiter is a prerequisite of the identity layer (ERR-009)"
        );

        let per_actor = cfg
            .rate_limit
            .per_actor
            .expect("[server.rate_limit.per_actor] must be declared explicitly");
        assert!(per_actor.burst_size.get() >= 1);
        assert!(per_actor.refill_period_ms.get() >= 1);
        assert!(
            per_actor.max_tracked_actors.get() >= 1,
            "per-actor buckets need a cardinality bound with LRU eviction"
        );
    }

    /// T02 / spec S1 + FR-003: the two authorization mapping tables. `permission_tools`
    /// maps a Falcon permission code to the MCP data tools it unlocks; `intent_tools` maps
    /// a runtime intent to the tools it *requires*. The gate is the three-way intersection
    /// of boot grant, permission grant, and intent requirement — checking merely that the
    /// narrowed set is non-empty would let a finance-only user's member question through.
    #[test]
    fn shipped_config_declares_both_authorization_mapping_tables() {
        let cfg = AppConfig::load("config/config.toml").expect("config should load");
        let authz = cfg
            .authz
            .expect("config/config.toml must declare an [authz] section");

        // Permission side: the four starcharger sub-pages. engproj maps to nothing —
        // no MCP endpoint serves engineering-project data yet (PRD FU-003).
        let finance = authz
            .permission_tools
            .get("hdrenewables/elecsvc/starcharger/finance")
            .expect("finance permission must be mapped");
        assert!(finance.contains(&"bill_revenue".to_string()));
        assert_eq!(
            authz
                .permission_tools
                .get("hdrenewables/elecsvc/starcharger/engproj")
                .map(Vec::len),
            Some(0),
            "engproj has no corresponding MCP endpoint yet (FU-003)"
        );

        // Intent side: every intent in the runtime allowlist needs a row, including
        // `unknown`, so an unmapped intent is a startup failure rather than a runtime
        // default-deny discovered in production.
        for intent in [
            "unknown",
            "revenue",
            "charging",
            "member",
            "site-build",
            "report",
        ] {
            assert!(
                authz.intent_tools.contains_key(intent),
                "intent `{intent}` must have a required-tools row"
            );
        }
        assert_eq!(authz.intent_tools["unknown"].len(), 0);
        assert_eq!(authz.intent_tools["site-build"].len(), 0);
        assert_eq!(
            authz.intent_tools["report"].len(),
            6,
            "a full report spans every data tool"
        );
    }

    /// T02 / spec S1 + D-010: the report pipeline gets its own grant ceiling instead of
    /// sharing `[insight.grants].fetcher`. The shared one lists five tools and its comment
    /// deliberately excludes `bill_member_analysis`, while the report prompt asks the model
    /// to call six — so the sixth was never advertised to it. That pre-existing mismatch is
    /// what this row fixes.
    #[test]
    fn shipped_config_gives_report_its_own_six_tool_grant() {
        let cfg = AppConfig::load("config/config.toml").expect("config should load");

        let report_grant = cfg
            .report_grants
            .expect("config/config.toml must declare [report.grants]");
        assert_eq!(report_grant.fetcher.len(), 6);
        assert!(
            report_grant
                .fetcher
                .contains(&"bill_member_analysis".to_string()),
            "the report grant must include the tool the shared insight grant omits"
        );

        // The insight grant is untouched: narrowing report must not widen insight.
        assert_eq!(cfg.insight_grants.fetcher.len(), 5);
        assert!(!cfg
            .insight_grants
            .fetcher
            .contains(&"bill_member_analysis".to_string()));
    }

    fn authz_fixture() -> AuthzConfig {
        AppConfig::load("config/config.toml")
            .expect("config should load")
            .authz
            .expect("shipped config declares [authz]")
    }

    /// T02 / spec S1 + FR-003: both mapping tables are validated at boot, not at request
    /// time. A gap discovered on a live request would surface as a silent default-deny —
    /// a user simply told "no" — which is far harder to diagnose than a refused startup.
    #[test]
    fn authz_validation_rejects_an_uncovered_intent_at_boot() {
        let authz = authz_fixture();
        let advertised: Vec<String> = ADVERTISED.iter().map(|t| t.to_string()).collect();

        // The runtime's real allowlist is covered.
        let allowlist: Vec<String> = [
            "unknown",
            "revenue",
            "charging",
            "member",
            "site-build",
            "report",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        authz
            .validate(&allowlist, &advertised)
            .expect("the shipped tables must cover the shipped allowlist");

        // A new intent with no row fails startup, naming the intent.
        let mut extended = allowlist.clone();
        extended.push("forecast".to_string());
        let err = authz
            .validate(&extended, &advertised)
            .expect_err("an uncovered intent must fail startup");
        assert!(
            err.to_string().contains("forecast"),
            "the error must name the uncovered intent, got: {err}"
        );
    }

    /// A mapped tool the MCP server never advertises is a typo that would otherwise become
    /// a permanent, invisible default-deny for that topic.
    #[test]
    fn authz_validation_rejects_a_tool_the_server_never_advertises() {
        let mut authz = authz_fixture();
        authz
            .intent_tools
            .insert("revenue".to_string(), vec!["bil_revenue".to_string()]);

        let advertised: Vec<String> = ADVERTISED.iter().map(|t| t.to_string()).collect();
        let allowlist: Vec<String> = [
            "unknown",
            "revenue",
            "charging",
            "member",
            "site-build",
            "report",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();

        let err = authz
            .validate(&allowlist, &advertised)
            .expect_err("an unadvertised tool must fail startup");
        assert!(
            err.to_string().contains("bil_revenue"),
            "the error must name the offending tool, got: {err}"
        );
    }

    /// The report ceiling gets the same treatment.
    #[test]
    fn report_grant_validation_rejects_an_unadvertised_tool() {
        let advertised: Vec<String> = ADVERTISED.iter().map(|t| t.to_string()).collect();

        let good = AppConfig::load("config/config.toml")
            .expect("config should load")
            .report_grants
            .expect("shipped config declares [report.grants]");
        good.validate(&advertised)
            .expect("the shipped report grant must only name advertised tools");

        let bad = ReportGrants {
            fetcher: vec!["nope".to_string()],
        };
        let err = bad
            .validate(&advertised)
            .expect_err("an unadvertised tool must fail startup");
        assert!(err.to_string().contains("nope"), "got: {err}");
    }

    /// The six datacenter tools `config/mcp-config.toml` maps.
    const ADVERTISED: [&str; 6] = [
        "bill_revenue",
        "station_revenue_ranking",
        "bill_charge",
        "business_metrics",
        "member_analysis",
        "bill_member_analysis",
    ];

    #[test]
    fn insight_grants_default_to_wildcard_when_section_absent() {
        // The in-code default (no `[insight]` section) still grants the fetcher every discovered
        // MCP tool, so a minimal config keeps working.
        assert_eq!(InsightGrants::default().fetcher, ["*"]);
        assert_eq!(InsightGrants::default().charter, ["emit_chart"]);
    }

    #[test]
    fn config_loads_ss_chat_grants() {
        let cfg = AppConfig::load("config/config.toml").expect("config should load");
        assert_eq!(cfg.ss_chat_grants.fetcher, SS_FETCHER_TOOLS);
        assert_eq!(cfg.ss_chat_grants.charter, ["emit_chart"]);
    }

    #[test]
    fn ss_chat_fetcher_grant_never_uses_the_wildcard() {
        // The invariant that keeps the two chat pipelines apart: one MCP server advertises BOTH
        // the EV-charging tools and the `ss_*` ones, so a `"*"` here would hand the SS fetcher
        // datacenter tools its prompt never describes — and the SS analyst would then be asked to
        // reason over EV-charging rows under 星星電力 branding. Checked on the shipped config and
        // on the in-code default, since either could reintroduce it.
        let cfg = AppConfig::load("config/config.toml").expect("config should load");
        for (source, grant) in [
            ("config/config.toml", &cfg.ss_chat_grants.fetcher),
            ("SsChatGrants::default()", &SsChatGrants::default().fetcher),
        ] {
            assert!(
                !grant.iter().any(|name| name == "*"),
                "{source} grants the SS fetcher the `*` wildcard, which would expose the \
                 EV-charging tools to the 星星電力 pipeline"
            );
            assert!(
                grant.iter().all(|name| name.starts_with("ss_")),
                "{source} grants the SS fetcher a non-`ss_` tool: {grant:?}"
            );
        }
    }

    #[test]
    fn ss_chat_grants_default_to_the_six_ss_tools_when_section_absent() {
        assert_eq!(SsChatGrants::default().fetcher, SS_FETCHER_TOOLS);
        assert_eq!(SsChatGrants::default().charter, ["emit_chart"]);
    }

    #[test]
    fn config_loads_the_ss_stage_prompts() {
        let cfg = AppConfig::load("config/config.toml").expect("config should load");
        // Every id `PromptBank::from_app_config` requires for the /ss-chat pipeline must resolve,
        // or boot fails — pin them here so a renamed file is caught without booting.
        for id in [
            "ss_fetcher_system",
            "ss_analyst_system",
            "ss_charter_system",
        ] {
            let body = cfg
                .get_prompt_by_id(id)
                .unwrap_or_else(|e| panic!("prompt `{id}` should load: {e}"));
            assert!(!body.trim().is_empty(), "prompt `{id}` is empty");
        }
    }
}
