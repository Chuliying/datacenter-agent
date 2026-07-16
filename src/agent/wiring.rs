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

//! Production assembly of the `/insight` pipeline — the seam that turns the boot-discovered MCP
//! tools + the environment's LLM defaults + the **config-driven tool grants** into a runnable
//! [`Orchestrator`].
//!
//! This is the **direct** wiring the server handlers use today: it builds the four-stage
//! `fetcher → analyst → charter → finalizer` pipeline ([`crate::agent::pipeline`]) from live
//! parts and hands back an [`Orchestrator`] ready to [`run`](Orchestrator::run) (buffered) or
//! [`run_emitting`](Orchestrator::run_emitting) (streaming). It deliberately **bypasses the
//! runtime turn** (guardrails / intent / memory / audit) — routing the pipeline *behind* the
//! runtime `AgentPort` is the plan's §9 step, reserved for after the pipeline is proven by hand.
//!
//! **Tool grants come from config, not code** (`[insight.grants]` in `config.toml`, see
//! [`InsightGrants`](crate::config::InsightGrants)). Each stage's grant is a list of wire names,
//! resolved here per name:
//!
//! - the sentinel `"*"` expands to **every tool the MCP server advertised** — the fetcher's usual
//!   grant, so a new datacenter tool is wired by the server offering it, no code change;
//! - a **built-in code-backed** name (`emit_chart`) resolves to its Rust [`SchemaTool`] sink;
//! - anything else is an **MCP tool** whose schema is read from the discovered set (fail-fast if
//!   the server never advertised it — see [`validate_insight_grants`], run at boot).
//!
//! Each MCP tool fills a distinct artifact slot `{stage}.{name}` (e.g. `fetcher.bill_revenue`), so
//! granting several tools never has one clobber another's result.
//!
//! Two shapes from one builder, selected by the `sink` argument:
//!
//! - **buffered** (`sink = None`) — every stage runs on one shared buffered
//!   [`OpenAiLlm`]; nothing is emitted. This backs `/insight`.
//! - **streaming** (`sink = Some(_)`) — every LLM stage runs on a shared [`StreamingOpenAiLlm`], so
//!   each one's tokens stream live, delimited by the orchestrator's `StageStarted` /
//!   `StageFinished` events. This backs `/insight/stream`.
//!
//! The terminal [`Finalizer`] is pure logic and emits no tokens of its own — it assembles the
//! analyst's report and the charter's charts into the final answer.
//!
//! # References
//!
//! - Sub-agent plan §4 — the closed/config tool layer (grants resolved at boot, fail-fast)
//! - Sub-agent plan §9 — the eventual `PipelineAgentPort` behind the runtime (not yet wired)
//! - Sub-agent plan §10 — the endpoint pipelines (the `/agent` → `/insight` conversion)

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use anyhow::{anyhow, bail, Context, Result};
use async_openai::types::chat::ChatCompletionTool;

use crate::agent::config::{
    OutputShape, PipelineConfig, PipelineId, ReasoningEffort, ResolvedLlm, SubAgentConfig,
    SubAgentId,
};
use crate::agent::engine::{resolve_pipeline, ConfiguredAgent, Orchestrator, SubAgent};
use crate::agent::events::EventSink;
use crate::agent::llm::{OpenAiLlm, StreamingOpenAiLlm};
use crate::agent::payload::{ArtifactKey, LlmCapability, PayloadKind, Tool};
use crate::agent::pipeline::{
    agent_pipeline_id, analyst_config, charter_config, fetcher_config, report_analyst_config,
    report_composer_config, report_pipeline_id, Finalizer, Renderer,
};
use crate::agent::tools::{emit_chart_tool, emit_report_tool, McpTool};
use crate::mcp_client::McpHandle;

/// The wildcard grant: "every tool the MCP server advertises".
const ALL_MCP_TOOLS: &str = "*";

/// The report `composer`'s fixed grant: the built-in `emit_report` sink (code-backed, not MCP).
const EMIT_REPORT: &str = "emit_report";

/// The reasoning budget for **mechanical** stages — a data `fetcher` (fetch args, no analysis) and
/// the report `composer` (transcribe fetched numbers into a fixed shape). These tasks need no
/// chain-of-thought, so the smallest budget stops them silently burning the output token limit on
/// reasoning (a `truncated at token limit` with an otherwise empty stream). Reasoning stages (the
/// `analyst`, the `charter`) keep the provider default.
const MECHANICAL_REASONING: ReasoningEffort = ReasoningEffort::Minimal;

/// Builds one stage's LLM capability: the streaming adapter when `sink` is `Some` (tokens stream
/// onto it), else the buffered adapter. Factored so a pipeline can build a second, lower-reasoning
/// client for its mechanical stages from a tweaked [`ResolvedLlm`].
fn build_stage_llm(
    resolved: &ResolvedLlm,
    sink: &Option<Arc<dyn EventSink>>,
) -> Result<Arc<dyn LlmCapability>> {
    Ok(match sink {
        Some(sink) => Arc::new(
            StreamingOpenAiLlm::from_resolved(resolved, sink.clone())
                .context("build streaming stage LLM")?,
        ),
        None => Arc::new(OpenAiLlm::from_resolved(resolved).context("build buffered stage LLM")?),
    })
}

/// Builds the `/insight` pipeline as a runnable [`Orchestrator`], registered under
/// [`agent_pipeline_id`].
///
/// One LLM client is built and shared by every stage: the [`StreamingOpenAiLlm`] when `sink` is
/// `Some` (so every stage's tokens stream), else the buffered [`OpenAiLlm`]. Each stage's tools
/// are resolved from its config grant (see the module docs).
///
/// # Arguments
///
/// - `mcp`: the connected datacenter MCP handle the tool calls go through.
/// - `discovered`: the tools the MCP server advertised at boot; each MCP grant's schema is read
///   from here so the model sees the server's real argument shape.
/// - `mcp_instructions`: the MCP server's handshake conventions, appended to the `fetcher`'s
///   instruction (the data-tool-bearing stage) when present — parity with the legacy assembly.
/// - `fetcher_grant` / `charter_grant`: the config-driven tool grants for those stages.
/// - `resolved`: the fully-resolved LLM every stage runs on.
/// - `sink`: `Some` selects the streaming shape; `None` is fully buffered.
///
/// # Returns
///
/// Returns an [`Orchestrator`] holding exactly the `agent_pipeline_id()` pipeline.
///
/// # Errors
///
/// - a granted MCP tool was not advertised by the server (typically already caught at boot by
///   [`validate_insight_grants`]);
/// - the vendor HTTP client could not be built from `resolved`;
/// - a pipeline stage reference failed to resolve (an internal wiring bug).
pub fn build_insight_pipeline(
    mcp: McpHandle,
    discovered: &[ChatCompletionTool],
    mcp_instructions: Option<&str>,
    fetcher_grant: &[String],
    charter_grant: &[String],
    resolved: &ResolvedLlm,
    sink: Option<Arc<dyn EventSink>>,
) -> Result<Orchestrator> {
    // ── resolve each stage's config grant into concrete tools ──
    let fetcher_tools = build_stage_tools("fetcher", fetcher_grant, &mcp, discovered)?;
    let charter_tools = build_stage_tools("charter", charter_grant, &mcp, discovered)?;

    // ── two LLM clients (streaming or buffered per `sink`): the default for the reasoning stages,
    //    and a minimal-reasoning one for the mechanical `fetcher` (plan: cut the hidden reasoning
    //    budget that silently truncated tool-heavy stages) ──
    let llm = build_stage_llm(resolved, &sink).context("build insight LLM")?;
    let llm_low = build_stage_llm(&resolved.with_reasoning_effort(MECHANICAL_REASONING), &sink)
        .context("build minimal-reasoning insight LLM")?;

    // ── the fetcher instruction, composed with the MCP server's conventions when present ──
    let mut fetcher_cfg = fetcher_config();
    fetcher_cfg.instruction = compose_with_mcp(&fetcher_cfg.instruction, mcp_instructions);

    // ── the four stages; upstream shapes are Intermediate, the finalizer is the terminal Final ──
    let fetcher: Arc<dyn SubAgent> = Arc::new(ConfiguredAgent::new(
        &fetcher_cfg,
        llm_low, // mechanical fetch — minimal reasoning
        fetcher_tools,
        OutputShape::Intermediate,
    ));
    let analyst: Arc<dyn SubAgent> = Arc::new(ConfiguredAgent::new(
        &analyst_config(),
        llm.clone(),
        vec![], // the analyst only reasons over provided material
        OutputShape::Intermediate,
    ));
    let charter: Arc<dyn SubAgent> = Arc::new(ConfiguredAgent::new(
        &charter_config(),
        llm, // chart selection is a judgment — keep the default reasoning budget
        charter_tools,
        OutputShape::Intermediate,
    ));
    let finalizer: Arc<dyn SubAgent> = Arc::new(Finalizer::default_stage());

    let agents: HashMap<SubAgentId, Arc<dyn SubAgent>> = [
        (SubAgentId("fetcher".into()), fetcher),
        (SubAgentId("analyst".into()), analyst),
        (SubAgentId("charter".into()), charter),
        (SubAgentId("finalizer".into()), finalizer),
    ]
    .into_iter()
    .collect();

    let pipeline = PipelineConfig {
        id: agent_pipeline_id(),
        stages: vec![
            SubAgentId("fetcher".into()),
            SubAgentId("analyst".into()),
            SubAgentId("charter".into()),
            SubAgentId("finalizer".into()),
        ],
    };
    let stages = resolve_pipeline(&pipeline, &agents)
        .map_err(|e| anyhow!("resolve insight pipeline: {e}"))?;

    let mut orchestrator = Orchestrator::new();
    orchestrator.insert(pipeline.id, stages);
    Ok(orchestrator)
}

/// Builds the `/report` pipeline as a runnable [`Orchestrator`], registered under
/// [`report_pipeline_id`].
///
/// A four-stage `fetcher → analyst → composer → renderer` chain. The `fetcher` pulls the datacenter
/// data (its config grant, shared with `/insight`), the `analyst` writes the executive insight
/// narrative, the `composer` maps the data + narrative into a schema-validated
/// [`ReportData`](crate::agent::report::ReportData) via its built-in `emit_report` sink, and the
/// **terminal** pure-logic [`Renderer`] injects that `report.data` into the boot-loaded HTML
/// `template`, producing the `falcon-report` `Final` answer. The LLM never writes HTML.
///
/// Buffered/streaming is selected by `sink`, exactly as [`build_insight_pipeline`]: the three LLM
/// stages share one client, streaming their tokens onto `sink` when it is `Some`; the renderer is
/// pure logic and emits none of its own.
///
/// # Arguments
///
/// - `mcp` / `discovered` / `mcp_instructions`: as for [`build_insight_pipeline`].
/// - `fetcher_grant`: the datacenter tools the report fetcher may call (shares the `/insight`
///   fetcher's grant — the same broad snapshot).
/// - `resolved`: the fully-resolved LLM every LLM stage runs on.
/// - `template`: the boot-loaded HTML template the renderer fills (its `__REPORT_DATA_JSON__`
///   placeholder is validated present at boot, see [`AppState::new`](crate::appstate::AppState)).
/// - `sink`: `Some` selects the streaming shape; `None` is fully buffered.
///
/// # Errors
///
/// As for [`build_insight_pipeline`]: a granted MCP tool absent from the server, an LLM client that
/// fails to build, or an unresolvable stage reference.
pub fn build_report_pipeline(
    mcp: McpHandle,
    discovered: &[ChatCompletionTool],
    mcp_instructions: Option<&str>,
    fetcher_grant: &[String],
    resolved: &ResolvedLlm,
    template: Arc<String>,
    sink: Option<Arc<dyn EventSink>>,
) -> Result<Orchestrator> {
    // ── resolve each tool-bearing stage's grant into concrete tools ──
    let fetcher_tools = build_stage_tools("fetcher", fetcher_grant, &mcp, discovered)?;
    // The composer's only tool is the built-in `emit_report` sink (code-backed, no MCP).
    let composer_tools =
        build_stage_tools("composer", &[EMIT_REPORT.to_string()], &mcp, discovered)?;

    // ── two LLM clients (streaming or buffered per `sink`): the default for the reasoning `analyst`,
    //    and a minimal-reasoning one for the mechanical `fetcher` + `composer`. The composer only
    //    transcribes fetched numbers into a fixed shape, so full reasoning was ~86% wasted output
    //    and the cause of the token-limit truncations. ──
    let llm = build_stage_llm(resolved, &sink).context("build report LLM")?;
    let llm_low = build_stage_llm(&resolved.with_reasoning_effort(MECHANICAL_REASONING), &sink)
        .context("build minimal-reasoning report LLM")?;

    // ── the fetcher instruction, composed with the MCP server's conventions when present ──
    let mut fetcher_cfg = fetcher_config();
    fetcher_cfg.instruction = compose_with_mcp(&fetcher_cfg.instruction, mcp_instructions);

    // ── the four stages; upstream shapes are Intermediate, the renderer is the terminal Final ──
    let fetcher: Arc<dyn SubAgent> = Arc::new(ConfiguredAgent::new(
        &fetcher_cfg,
        llm_low.clone(), // mechanical fetch — minimal reasoning
        fetcher_tools,
        OutputShape::Intermediate,
    ));
    let analyst: Arc<dyn SubAgent> = Arc::new(ConfiguredAgent::new(
        &report_analyst_config(),
        llm,    // the analyst genuinely reasons — keep the default budget
        vec![], // the analyst only reasons over provided material
        OutputShape::Intermediate,
    ));
    let composer: Arc<dyn SubAgent> = Arc::new(ConfiguredAgent::new(
        &report_composer_config(),
        llm_low, // mechanical transcription — minimal reasoning
        composer_tools,
        OutputShape::Intermediate,
    ));
    let renderer: Arc<dyn SubAgent> = Arc::new(Renderer::with_template(template));

    let agents: HashMap<SubAgentId, Arc<dyn SubAgent>> = [
        (SubAgentId("fetcher".into()), fetcher),
        (SubAgentId("analyst".into()), analyst),
        (SubAgentId("composer".into()), composer),
        (SubAgentId("renderer".into()), renderer),
    ]
    .into_iter()
    .collect();

    let pipeline = PipelineConfig {
        id: report_pipeline_id(),
        stages: vec![
            SubAgentId("fetcher".into()),
            SubAgentId("analyst".into()),
            SubAgentId("composer".into()),
            SubAgentId("renderer".into()),
        ],
    };
    let stages = resolve_pipeline(&pipeline, &agents)
        .map_err(|e| anyhow!("resolve report pipeline: {e}"))?;

    let mut orchestrator = Orchestrator::new();
    orchestrator.insert(pipeline.id, stages);
    Ok(orchestrator)
}

/// The pipeline id the greeting pipeline is registered under.
pub fn greeting_pipeline_id() -> PipelineId {
    PipelineId("greeting".into())
}

/// Builds the greeting pipeline as a runnable [`Orchestrator`], registered under
/// [`greeting_pipeline_id`].
///
/// A two-stage `fetcher → analyst` chain, always **buffered** (greetings are produced by a
/// boot-time background task, not streamed): the fetcher pulls a broad datacenter snapshot with its
/// granted tools, and the **terminal** analyst turns that material into one short C-suite greeting
/// — its model message *is* the `Final` answer. There is no charter/finalizer: a greeting has no
/// charts.
///
/// # Arguments
///
/// - `mcp` / `discovered` / `mcp_instructions`: as for [`build_insight_pipeline`].
/// - `fetcher_grant`: the datacenter tools the greeting fetcher may call (shares the `/insight`
///   fetcher's grant — the same broad snapshot).
/// - `fetcher_instruction` / `analyst_instruction`: the two greeting stage prompts.
/// - `resolved`: the fully-resolved LLM both stages run on.
///
/// # Errors
///
/// As for [`build_insight_pipeline`]: a granted MCP tool absent from the server, an LLM client that
/// fails to build, or an unresolvable stage reference.
pub fn build_greeting_pipeline(
    mcp: McpHandle,
    discovered: &[ChatCompletionTool],
    mcp_instructions: Option<&str>,
    fetcher_grant: &[String],
    fetcher_instruction: &str,
    analyst_instruction: &str,
    resolved: &ResolvedLlm,
) -> Result<Orchestrator> {
    let fetcher_tools = build_stage_tools("fetcher", fetcher_grant, &mcp, discovered)?;
    // Buffered (greetings are boot-time, not streamed): the default client for the reasoning
    // analyst, a minimal-reasoning one for the mechanical fetcher.
    let llm: Arc<dyn LlmCapability> =
        Arc::new(OpenAiLlm::from_resolved(resolved).context("build greeting LLM")?);
    let llm_low: Arc<dyn LlmCapability> = Arc::new(
        OpenAiLlm::from_resolved(&resolved.with_reasoning_effort(MECHANICAL_REASONING))
            .context("build minimal-reasoning greeting LLM")?,
    );

    let fetcher_cfg = SubAgentConfig {
        id: SubAgentId("fetcher".into()),
        instruction: compose_with_mcp(fetcher_instruction, mcp_instructions),
        llm: None,
        tools: vec![],
        accepts: vec![PayloadKind::Initial],
        output: None,
        capture_message: false, // tool-only stage — its note is throwaway
    };
    let analyst_cfg = SubAgentConfig {
        id: SubAgentId("analyst".into()),
        instruction: analyst_instruction.to_string(),
        llm: None,
        tools: vec![],
        accepts: vec![PayloadKind::Intermediate],
        output: None,
        capture_message: false, // terminal — its message *is* the greeting (the Final answer)
    };

    let fetcher: Arc<dyn SubAgent> = Arc::new(ConfiguredAgent::new(
        &fetcher_cfg,
        llm_low, // mechanical fetch — minimal reasoning
        fetcher_tools,
        OutputShape::Intermediate,
    ));
    let analyst: Arc<dyn SubAgent> = Arc::new(ConfiguredAgent::new(
        &analyst_cfg,
        llm,    // the greeting writer reasons over the snapshot — keep the default budget
        vec![], // the analyst writes from provided material; no tools
        OutputShape::Final,
    ));

    let agents: HashMap<SubAgentId, Arc<dyn SubAgent>> = [
        (SubAgentId("fetcher".into()), fetcher),
        (SubAgentId("analyst".into()), analyst),
    ]
    .into_iter()
    .collect();

    let pipeline = PipelineConfig {
        id: greeting_pipeline_id(),
        stages: vec![SubAgentId("fetcher".into()), SubAgentId("analyst".into())],
    };
    let stages = resolve_pipeline(&pipeline, &agents)
        .map_err(|e| anyhow!("resolve greeting pipeline: {e}"))?;

    let mut orchestrator = Orchestrator::new();
    orchestrator.insert(pipeline.id, stages);
    Ok(orchestrator)
}

/// Appends the MCP server's handshake conventions to a tool-bearing stage's instruction (parity
/// with the legacy prompt assembly). Returns the instruction unchanged when there are none.
fn compose_with_mcp(instruction: &str, mcp_instructions: Option<&str>) -> String {
    match mcp_instructions.filter(|s| !s.trim().is_empty()) {
        Some(instr) => {
            format!("{instruction}\n\n# MCP server conventions (apply to all tools)\n{instr}")
        }
        None => instruction.to_string(),
    }
}

/// Validates the `/insight` tool grants against the discovered set — the **fail-fast at boot**
/// check (plan §4/§2.2) so a typo or an unavailable tool aborts startup, not a live request.
///
/// # Errors
///
/// Returns `Err` naming the offending `(stage, tool)` when a granted name is neither the `"*"`
/// wildcard, a built-in code-backed tool, nor a tool the server advertised.
pub fn validate_insight_grants(
    discovered: &[ChatCompletionTool],
    fetcher_grant: &[String],
    charter_grant: &[String],
) -> Result<()> {
    validate_grant("fetcher", fetcher_grant, discovered)?;
    validate_grant("charter", charter_grant, discovered)?;
    Ok(())
}

/// Checks every name in one stage's grant resolves to a real tool.
fn validate_grant(stage: &str, grant: &[String], discovered: &[ChatCompletionTool]) -> Result<()> {
    for name in grant {
        let known = name == ALL_MCP_TOOLS
            || code_tool(name).is_some()
            || discovered.iter().any(|t| t.function.name == *name);
        if !known {
            bail!(
                "insight `{stage}` grant names tool `{name}`, which the MCP server did not \
                 advertise and is not a built-in tool"
            );
        }
    }
    Ok(())
}

/// Resolves one stage's config grant into concrete [`Tool`]s.
///
/// `"*"` first expands to every advertised MCP tool name; each resulting name then resolves to a
/// code-backed tool or an MCP tool (§ module docs).
fn build_stage_tools(
    stage: &str,
    grant: &[String],
    mcp: &McpHandle,
    discovered: &[ChatCompletionTool],
) -> Result<Vec<Box<dyn Tool>>> {
    let advertised: Vec<String> = discovered.iter().map(|t| t.function.name.clone()).collect();
    expand_grant(grant, &advertised)
        .iter()
        .map(|name| build_tool(stage, name, mcp, discovered))
        .collect()
}

/// Expands the `"*"` wildcard into every advertised MCP tool name, passing other names through,
/// de-duplicated in first-seen order.
fn expand_grant(grant: &[String], advertised: &[String]) -> Vec<String> {
    let mut seen = HashSet::new();
    grant
        .iter()
        .flat_map(|name| {
            if name == ALL_MCP_TOOLS {
                advertised.to_vec()
            } else {
                vec![name.clone()]
            }
        })
        .filter(|name| seen.insert(name.clone()))
        .collect()
}

/// Builds one granted tool: a built-in code-backed tool if the name names one, else an MCP tool
/// filling the `{stage}.{name}` slot.
fn build_tool(
    stage: &str,
    name: &str,
    mcp: &McpHandle,
    discovered: &[ChatCompletionTool],
) -> Result<Box<dyn Tool>> {
    if let Some(tool) = code_tool(name) {
        return Ok(tool);
    }
    let (description, parameters) = tool_schema(discovered, name)?;
    Ok(Box::new(McpTool::from_name(
        mcp.clone(),
        name, // advertised, LLM-facing name
        name, // raw MCP wire name (coincide for the datacenter)
        description,
        parameters,
        ArtifactKey::new(stage, name), // a distinct slot per tool: `{stage}.{name}`
    )))
}

/// Resolves a built-in, code-backed (non-MCP) tool by its advertised name, or `None` for an MCP
/// name. This closed set is the only place a config grant reaches Rust-defined tools.
fn code_tool(name: &str) -> Option<Box<dyn Tool>> {
    match name {
        "emit_chart" => Some(Box::new(emit_chart_tool())),
        EMIT_REPORT => Some(Box::new(emit_report_tool())),
        _ => None,
    }
}

/// Reads a tool's `(description, parameters)` from the boot-discovered set, matching by wire name.
///
/// A missing tool is a **fail-fast** error, not a silent degrade. Absent parameters default to the
/// empty-object schema (a no-argument tool).
///
/// # Errors
///
/// Returns `Err` when the server advertised no tool whose name equals `name`.
fn tool_schema(
    discovered: &[ChatCompletionTool],
    name: &str,
) -> Result<(String, serde_json::Value)> {
    let found = discovered
        .iter()
        .find(|t| t.function.name == name)
        .ok_or_else(|| {
            anyhow!("insight grant names MCP tool `{name}`, which the server did not advertise")
        })?;
    let description = found.function.description.clone().unwrap_or_default();
    let parameters = found
        .function
        .parameters
        .clone()
        .unwrap_or_else(|| serde_json::json!({ "type": "object", "properties": {} }));
    Ok((description, parameters))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_schema_fails_fast_when_the_tool_is_absent() {
        // A grant naming a tool the server never advertised must error, not silently degrade.
        let err = tool_schema(&[], "bill_revenue").expect_err("missing tool must error");
        assert!(err.to_string().contains("bill_revenue"));
    }

    #[test]
    fn code_tool_resolves_only_the_built_in_sinks() {
        assert!(code_tool("emit_chart").is_some());
        assert!(code_tool("emit_report").is_some());
        assert!(code_tool("bill_revenue").is_none()); // an MCP name, not code-backed
        assert!(code_tool("nope").is_none());
    }

    #[test]
    fn expand_grant_expands_the_wildcard_and_dedups() {
        let advertised = vec!["bill_revenue".to_string(), "member_analysis".to_string()];
        // "*" becomes every advertised tool.
        assert_eq!(
            expand_grant(&["*".to_string()], &advertised),
            vec!["bill_revenue".to_string(), "member_analysis".to_string()]
        );
        // Explicit names pass through unchanged.
        assert_eq!(
            expand_grant(&["emit_chart".to_string()], &advertised),
            vec!["emit_chart".to_string()]
        );
        // "*" plus an overlapping explicit name de-duplicates in first-seen order.
        assert_eq!(
            expand_grant(&["*".to_string(), "bill_revenue".to_string()], &advertised),
            vec!["bill_revenue".to_string(), "member_analysis".to_string()]
        );
    }

    #[test]
    fn validate_grant_accepts_wildcard_and_code_tools_but_rejects_unknown_mcp_names() {
        // No discovered tools, yet these resolve without touching the server.
        assert!(validate_grant("fetcher", &["*".to_string()], &[]).is_ok());
        assert!(validate_grant("charter", &["emit_chart".to_string()], &[]).is_ok());
        // A concrete MCP name the server never advertised fails, naming the stage and tool.
        let err = validate_grant("fetcher", &["ghost_tool".to_string()], &[])
            .expect_err("unknown MCP tool must fail validation");
        let msg = err.to_string();
        assert!(msg.contains("fetcher"));
        assert!(msg.contains("ghost_tool"));
    }
}
