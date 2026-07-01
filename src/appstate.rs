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

//! Process-wide state shared by every handler.
//!
//! This module also process ENV variables for the LLM and MCP server.

use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use async_openai::types::chat::ChatCompletionTool;
use reqwest::Client;
use tokio::sync::Mutex;

use crate::config::AppConfig;
use crate::mcp_client::McpHandle;
use crate::model::{GenerationConfig, History};
use crate::runtime::audit::{AuditFailurePolicy, AuditSink};
use crate::runtime::config::RuntimeConfig;
use crate::runtime::guardrails::answer_policy::AnswerPolicy;
use crate::runtime::input::pipeline::InputPipeline;
use crate::runtime::llm_normalizer::LlmInputNormalizer;
use crate::runtime::memory::store::SessionMemoryStore;
use crate::runtime::registry::BuiltinRegistry;

/// Helper to parse optional env vars with a fallback.
fn get_env_with_default<T: std::str::FromStr>(key: &str, default: T) -> T {
    std::env::var(key)
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(default)
}

/// LLM defaults sourced from the environment at startup.
///
/// The per-request handler clones these into a fresh `GenerationConfig`.
#[derive(Debug, Clone)]
pub struct LlmDefaults {
    pub api_key: String,
    pub base_url: String,
    pub model: String,
    pub app_url: Option<String>,
    pub app_title: Option<String>,
    pub temperature: f32,
    pub top_p: f32,
    pub max_tokens: u32,
}

impl LlmDefaults {
    pub fn from_env() -> Result<Self> {
        let api_key = std::env::var("OPENROUTER_API_KEY").context(
            "env_error: OPENROUTER_API_KEY missing, copy .env.example to .env and fill it in",
        )?;
        let model =
            std::env::var("OPENROUTER_MODEL").context("env_error: OPENROUTER_MODEL missing")?;
        let base_url = std::env::var("OPENROUTER_BASE_URL")
            .unwrap_or_else(|_| "https://openrouter.ai/api/v1".into());

        Ok(Self {
            api_key,
            base_url,
            model,
            app_url: std::env::var("OPENROUTER_APP_URL").ok(),
            app_title: std::env::var("OPENROUTER_APP_TITLE").ok(),
            temperature: get_env_with_default("OPENROUTER_TEMPERATURE", 0.2_f32),
            top_p: 0.1,
            max_tokens: get_env_with_default("OPENROUTER_MAX_TOKENS", 4096_u32),
        })
    }
}

/// Runtime-loaded prompts.
///
/// The actual prompt contents are read from the Markdown files declared
/// in `config.toml` under `[prompts.*]`. Every field here has a required `id`.
///
/// Stored in [`AppState`] behind an `Arc` so every handler clone shares
/// the same heap allocation rather than copying per request (each prompt can
/// cost up to kilobytes!).
#[derive(Debug)]
pub struct PromptBank {
    /// System prompt for the analytical `/agent` + `/agent/stream` endpoints.
    pub agent_system: String,
    /// System prompt for the greeting generator.
    pub greeting_system: String,
    /// User-side stub passed alongside `greeting_system`.
    pub greeting_user: String,
}

impl PromptBank {
    /// Pull the three required prompts out of an [`AppConfig`].
    ///
    /// # Errors
    ///
    /// Returns `Err` if any of `agent_system`, `greeting_system`, or
    /// `greeting_user` is missing from the loaded config.
    pub fn from_app_config(cfg: &AppConfig) -> Result<Self> {
        Ok(Self {
            agent_system: cfg.get_prompt_by_id("agent_system")?.to_string(),
            greeting_system: cfg.get_prompt_by_id("greeting_system")?.to_string(),
            greeting_user: cfg.get_prompt_by_id("greeting_user")?.to_string(),
        })
    }
}

#[derive(Clone)]
pub struct AppState {
    /// Handle of the datacenter MCP server (rmcp peer) for tool calls.
    pub mcp: McpHandle,
    /// Tool definitions discovered from the MCP server at startup, ready to
    /// pass to OpenRouter.
    ///
    /// Shared read-only behind an `Arc`.
    pub tools: Arc<Vec<ChatCompletionTool>>,
    /// The MCP server's handshake `instructions` (cross-cutting tool
    /// conventions), appended to every system prompt.
    ///
    /// It's optional, `None` if the server sent none.
    pub instructions: Arc<Option<String>>,
    /// LLM defaults loaded from the environment.
    pub llm: LlmDefaults,
    /// Prompt bodies loaded from `config/prompt_guide/*.md` at startup.
    pub prompts: Arc<PromptBank>,
    /// HTTP client used by `/ready` to probe the LLM base URL.
    pub http: Client,
    /// Bearer token every request must present in `Authorization: Bearer <token>`.
    ///
    /// This is for simple "who calls us?" authentication to prevent weirdos from spamming
    /// our endpoint, or worse, stealing our API key.
    ///
    /// Loaded once from `GLOBAL_TOKEN` at startup, never logged, and should rotate periodically (e.g. weekly).
    pub auth_token: Arc<String>,
    /// Pre-generated greeting strings populated by background tasks at boot.
    ///
    /// `GET /greeting` picks one at random.
    pub greetings: Arc<Mutex<Vec<String>>>,
    /// Runtime wiring. Active by default (cutover); `RUNTIME_ENABLED=false`
    /// rolls a request back to the legacy direct path.
    pub runtime: Option<Arc<AppRuntime>>,
}

/// Runtime dependencies assembled at boot.
pub struct AppRuntime {
    /// Whether runtime route wiring is enabled.
    pub enabled: bool,
    /// Runtime configuration.
    pub config: Arc<RuntimeConfig>,
    /// Deterministic input pipeline.
    pub input_pipeline: InputPipeline,
    /// Answer policy.
    pub answer_policy: Arc<dyn AnswerPolicy>,
    /// Optional LLM-backed input normalizer.
    pub llm_normalizer: Option<Arc<dyn LlmInputNormalizer>>,
    /// Optional server-side session memory store.
    pub sessions: Option<Arc<dyn SessionMemoryStore>>,
    /// Audit sink.
    pub audit_sink: Arc<dyn AuditSink>,
    /// Audit failure policy.
    pub audit_failure_policy: AuditFailurePolicy,
}

impl AppState {
    pub fn new(
        app_config: &AppConfig,
        mcp: McpHandle,
        tools: Arc<Vec<ChatCompletionTool>>,
        instructions: Arc<Option<String>>,
        llm: LlmDefaults,
        prompts: Arc<PromptBank>,
        auth_token: String,
    ) -> Result<Self> {
        let http = Client::builder()
            .timeout(Duration::from_secs(2))
            .build()
            .context("build /ready probe http client")?;
        let runtime = build_runtime(app_config)?;
        Ok(Self {
            mcp,
            tools,
            instructions,
            llm,
            prompts,
            http,
            auth_token: Arc::new(auth_token),
            greetings: Arc::new(Mutex::new(Vec::new())),
            runtime,
        })
    }

    /// Assemble a [`GenerationConfig`] for one tool-calling run.
    ///
    /// The effective system prompt is `base_system` with the MCP server's
    /// conventions appended (when present), mirroring the orchestrator. LLM
    /// parameters are cloned from [`LlmDefaults`]. Shared by `/agent`,
    /// `/agent/stream`, and the greeting generator so the wire contract and
    /// model behaviour stay identical.
    pub fn generation_config(
        &self,
        base_system: &str,
        user_prompt: String,
        history: Vec<History>,
    ) -> GenerationConfig {
        let system_base = match self.instructions.as_deref() {
            Some(instr) if !instr.trim().is_empty() => {
                format!("{base_system}\n\n# MCP server conventions (apply to all tools)\n{instr}")
            }
            _ => base_system.to_string(),
        };

        // Make LLM time-aware
        let now_str = chrono::Local::now()
            .format("%Y-%m-%d %H:%M:%S %:z")
            .to_string();
        let system = format!("# Current Time\n{now_str}\n\n{system_base}");

        GenerationConfig {
            system,
            user_prompt,
            history,
            api_key: self.llm.api_key.clone(),
            model: self.llm.model.clone(),
            base_url: self.llm.base_url.clone(),
            app_url: self.llm.app_url.clone(),
            app_title: self.llm.app_title.clone(),
            temperature: self.llm.temperature,
            top_p: self.llm.top_p,
            max_tokens: self.llm.max_tokens,
        }
    }
}

fn build_runtime(app_config: &AppConfig) -> Result<Option<Arc<AppRuntime>>> {
    let runtime_flag = std::env::var("RUNTIME_ENABLED").ok();
    build_runtime_for_flag(app_config, runtime_flag.as_deref())
}

fn build_runtime_for_flag(
    app_config: &AppConfig,
    runtime_flag: Option<&str>,
) -> Result<Option<Arc<AppRuntime>>> {
    let enabled = runtime_enabled_from_env(runtime_flag);
    if !enabled {
        return Ok(None);
    }
    let refs = app_config
        .runtime
        .as_ref()
        .context("runtime enabled but [runtime] config missing")?;
    let registry = BuiltinRegistry::default();
    let config = RuntimeConfig::load(refs, &registry).context("load runtime config")?;
    let answer_policy = registry
        .build_answer_policy(&config)
        .context("build runtime answer policy")?;
    let llm_normalizer = registry
        .build_llm_normalizer(&config)
        .context("build runtime LLM normalizer")?;
    let sessions = registry
        .build_memory(&config)
        .context("build runtime session memory")?;
    let audit_sink = registry
        .build_audit(&config)
        .context("build runtime audit sink")?;
    Ok(Some(Arc::new(AppRuntime {
        enabled,
        audit_failure_policy: config.assembly.audit_failure_policy,
        config: Arc::new(config),
        input_pipeline: InputPipeline::default(),
        answer_policy,
        llm_normalizer,
        sessions,
        audit_sink,
    })))
}

/// Resolve the runtime route flag with cutover semantics.
///
/// The Rust runtime is now the default streaming authority, so an unset (or
/// otherwise non-falsey) `RUNTIME_ENABLED` keeps it on. The flag is a rollback
/// escape hatch: only an explicit `false`/`0` (case- and whitespace-insensitive)
/// reverts a request to the legacy direct path.
fn runtime_enabled_from_env(value: Option<&str>) -> bool {
    match value.map(str::trim) {
        Some("0") => false,
        Some(rollback) if rollback.eq_ignore_ascii_case("false") => false,
        _ => true,
    }
}

/// Read `GLOBAL_TOKEN` from the environment.
///
/// Errors out on missing or empty so misconfiguration never silently accepts traffic.
pub fn load_global_token() -> Result<String> {
    let token = std::env::var("GLOBAL_TOKEN")
        .context("env_error: GLOBAL_TOKEN missing — set it in your environment / compose file")?;
    if token.trim().is_empty() {
        anyhow::bail!("env_error: GLOBAL_TOKEN is empty");
    }
    Ok(token)
}

/// Read `DATACENTER_MCP_URL`.
pub fn load_mcp_url() -> Result<String> {
    let url = std::env::var("DATACENTER_MCP_URL").context(
        "env_error: DATACENTER_MCP_URL missing — start datacenter MCP server and point DATACENTER_MCP_URL at its /mcp endpoint",
    )?;
    if url.trim().is_empty() {
        anyhow::bail!("env_error: DATACENTER_MCP_URL is empty");
    }
    Ok(url)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runtime_enabled_env_defaults_on_with_explicit_rollback() {
        // Cutover default: unset, empty, or any non-falsey value runs the runtime.
        for value in [
            None,
            Some(""),
            Some("true"),
            Some("TRUE"),
            Some("1"),
            Some("TRUE "),
        ] {
            assert!(runtime_enabled_from_env(value));
        }
        // Rollback escape hatch: only explicit false/0 reverts to the legacy path.
        for value in [Some("false"), Some("FALSE"), Some(" false "), Some("0")] {
            assert!(!runtime_enabled_from_env(value));
        }
    }

    #[test]
    fn explicit_rollback_skips_invalid_runtime_config() {
        let mut app_config = AppConfig::load("config/config.toml").expect("app config should load");
        app_config
            .runtime
            .as_mut()
            .expect("runtime refs should exist")
            .intents = app_config.root.join("runtime/missing-intents.toml");
        let result = build_runtime_for_flag(&app_config, Some("false"));

        assert!(matches!(result, Ok(None)));
    }

    #[test]
    fn default_cutover_requires_runtime_config() {
        let mut app_config = AppConfig::load("config/config.toml").expect("app config should load");
        app_config.runtime = None;

        let result = build_runtime_for_flag(&app_config, None);

        match result {
            Err(err) => assert!(err
                .to_string()
                .contains("runtime enabled but [runtime] config missing")),
            Ok(_) => panic!("default cutover must not silently fall back to legacy"),
        }
    }
}
