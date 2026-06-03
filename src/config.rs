//! Top-level application config.
//!
//! Loads a single TOML manifest (`config/config.toml` by default) that
//! points at the sub-config files (including intent-routing TOML and a set
//! of prompt Markdown files)
//!
//! Every relative path inside the manifest is resolved against the
//! **parent directory of the manifest file itself**, not the process CWD.
//! So container mounting can be much more straightforward.
//!
//! ## Example
//!
//! ```ignore
//! use eomc_agent::config::AppConfig;
//!
//! let app = AppConfig::load("config/config.toml")?;
//! let agent_prompt = app.prompt("agent_system")?;
//! let routing_toml = &app.routing_path;
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
    /// Pointer to the intent-routing TOML.
    routing: RoutingRef,
    /// Prompt id to Markdown file path map.
    #[serde(default)]
    prompts: BTreeMap<String, PromptRef>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct RoutingRef {
    /// Path to the intent-routing TOML (relative to the manifest's
    /// parent directory, or absolute).
    config: PathBuf,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct PromptRef {
    /// Path to a Markdown file containing the prompt body (relative to
    /// the manifest's parent directory, or absolute).
    file: PathBuf,
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
    /// Absolute path to the intent-routing TOML.
    /// Pass to [`crate::data_fetch::Router::from_config_path`].
    pub routing_path: PathBuf,
    /// Loaded prompt bodies KV map.
    pub prompts: BTreeMap<String, String>,
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

        // Resolve the routing path
        let routing_path = resolve_relative(&root, &manifest.routing.config);
        debug!(
            routing = %routing_path.display(),
            "app config: routing path resolved"
        );

        // Load prompts
        let prompts = manifest
            .prompts
            .iter()
            .map(|(id, prompt_ref)| Ok((id.clone(), load_prompt(&root, id, prompt_ref)?)))
            .collect::<Result<BTreeMap<_, _>>>()?;

        // Log the loaded config
        info!(
            root = %root.display(),
            prompts = prompts.len(),
            "app config loaded"
        );

        Ok(Self {
            root,
            routing_path,
            prompts,
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
