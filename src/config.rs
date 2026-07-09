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
        assert_eq!(runtime.audit_sink, "stdout");
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
}
