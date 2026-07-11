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

//! The `/agent` analytics pipeline: **fetcher → analyst → charter → finalizer**.
//!
//! The monolith's single `/agent` turn (fetch + analyse + chart in one prompt) is decomposed here
//! into four cooperating sub-agents, proving the sub-agent approach by construction — each stage is
//! a pure async function of its payload, unit-testable in isolation, and the four compose into the
//! same end-to-end behaviour:
//!
//! | Stage | Kind | Reads | Produces | Shape |
//! |---|---|---|---|---|
//! | `fetcher` | [`ConfiguredAgent`] + MCP tools | the user prompt | `fetcher.records` | Intermediate |
//! | `analyst` | [`ConfiguredAgent`], no tools | `fetcher.records` | `analyst.message` (its prose) | Intermediate |
//! | `charter` | [`ConfiguredAgent`] + `emit_chart` sink | the report + data | `charts.spec` (or nothing) | Intermediate |
//! | `finalizer` | [`Finalizer`] — pure logic, no LLM | `analyst.message` + `charts.spec` | the answer | Final |
//!
//! The **`finalizer` is pure logic** ([`render_report`]): it plain-concatenates the analysis report
//! with each chart wrapped in a ```` ```falcon-chart ```` fenced block. No model, no tools — the
//! empty grant is the isolation guarantee made concrete (it can neither fetch nor invent), and its
//! output is deterministic given the upstream artifacts.
//!
//! The `analyst`'s report is prose, but nothing special is needed to keep it: every stage's model
//! message is captured as a first-class artifact under `{id}.message` (open-key contract), so an
//! `Intermediate` boundary is lossless. The `finalizer` reads the `analyst`'s message as the report
//! body and carries the whole artifact map onto the `Final` result as provenance.
//!
//! # References
//!
//! - Sub-agent plan §10 — the endpoint pipelines (the `/agent` conversion)
//! - `config/prompt_guide/{fetcher,analyst,charter}_system.md` — the authored stage instructions

#![allow(dead_code)] // groundwork: the pipeline is assembled + tested, not yet wired behind AgentPort.

use std::collections::HashMap;

use async_trait::async_trait;

use crate::agent::config::{PipelineId, SubAgentConfig, SubAgentId};
use crate::agent::engine::SubAgent;
use crate::agent::payload::{
    AgentError, AgentPayload, ArtifactKey, ArtifactValue, FinalResult, PayloadKind,
};
use crate::agent::tools::ToolId;

// ===========================================================================
// Authored stage instructions (compiled in; superseded by the §6 TOML loader later)
// ===========================================================================

/// The `fetcher`'s system instruction — fetch the exact data, don't analyse.
pub const FETCHER_INSTRUCTION: &str = include_str!("../../config/prompt_guide/fetcher_system.md");
/// The `analyst`'s system instruction — analyse the fetched material into a markdown report.
pub const ANALYST_INSTRUCTION: &str = include_str!("../../config/prompt_guide/analyst_system.md");
/// The `charter`'s system instruction — decide on 0–2 charts, emit via `emit_chart`.
pub const CHARTER_INSTRUCTION: &str = include_str!("../../config/prompt_guide/charter_system.md");

// ===========================================================================
// Stage configs — the authored `SubAgentConfig` for each LLM-driven stage
// ===========================================================================

/// The `fetcher` config: MCP data tools, consumes the initial prompt, produces `fetcher.records`.
///
/// Non-terminal ⇒ its output shape derives to `Intermediate`
/// ([`effective_output`](crate::agent::config::effective_output)).
pub fn fetcher_config() -> SubAgentConfig {
    SubAgentConfig {
        id: SubAgentId("fetcher".into()),
        instruction: FETCHER_INSTRUCTION.to_string(),
        llm: None,
        tools: vec![ToolId::BillRevenue], // the real datacenter grant (extends with the closed set)
        accepts: vec![PayloadKind::Initial],
        output: None,
    }
}

/// The `analyst` config: no tools, consumes the fetcher's data, writes the prose report.
///
/// Its report is its model message, captured automatically as the `analyst.message` artifact (the
/// open-key contract captures every stage's message), so it survives the `Intermediate` boundary
/// with no per-agent wiring.
pub fn analyst_config() -> SubAgentConfig {
    SubAgentConfig {
        id: SubAgentId("analyst".into()),
        instruction: ANALYST_INSTRUCTION.to_string(),
        llm: None,
        tools: vec![],
        accepts: vec![PayloadKind::Intermediate],
        output: None,
    }
}

/// The `charter` config: the `emit_chart` sink only, consumes the report + data, produces
/// `charts.spec` (or nothing, for chit-chat).
pub fn charter_config() -> SubAgentConfig {
    SubAgentConfig {
        id: SubAgentId("charter".into()),
        instruction: CHARTER_INSTRUCTION.to_string(),
        llm: None,
        tools: vec![ToolId::EmitChart],
        accepts: vec![PayloadKind::Intermediate],
        output: None,
    }
}

/// The pipeline id that selects this `/agent` pipeline in the [`Orchestrator`](crate::agent::engine::Orchestrator).
pub fn agent_pipeline_id() -> PipelineId {
    PipelineId("agent".into())
}

// ===========================================================================
// render_report — the finalizer's pure, deterministic assembly
// ===========================================================================

/// Assembles the terminal answer: the analysis report, then **each chart as its own
/// ```` ```falcon-chart ```` fenced block**.
///
/// Pure and total — this *is* the finalizer's whole logic, factored out so it is directly testable
/// without constructing a payload. Charts are appended in order; each is pretty-printed JSON so the
/// rendered block matches the frontend contract in `agent_system.md`.
///
/// # Arguments
///
/// - `report`: the analyst's markdown prose (trailing whitespace is trimmed).
/// - `charts`: the falcon-chart JSON objects to append (empty ⇒ the report is returned as-is).
pub fn render_report(report: &str, charts: &[serde_json::Value]) -> String {
    charts
        .iter()
        .fold(report.trim_end().to_string(), |mut acc, chart| {
            let block =
                serde_json::to_string_pretty(chart).unwrap_or_else(|_| chart.to_string());
            acc.push_str("\n\n```falcon-chart\n");
            acc.push_str(&block);
            acc.push_str("\n```");
            acc
        })
}

/// Extracts the chart objects from a `charts.spec` artifact (a serialized
/// [`ChartBatch`](crate::agent::chart::ChartBatch)), or an empty vec when the charter produced none.
fn charts_of(artifacts: &HashMap<ArtifactKey, ArtifactValue>) -> Vec<serde_json::Value> {
    match artifacts.get(&ArtifactKey::charts_spec()) {
        Some(ArtifactValue::Json(v)) => v
            .get("charts")
            .and_then(serde_json::Value::as_array)
            .cloned()
            .unwrap_or_default(),
        _ => Vec::new(),
    }
}

// ===========================================================================
// Finalizer — the pure-logic terminal stage (no LLM, no tools)
// ===========================================================================

/// The payload variants the [`Finalizer`] consumes: it reads only the merged artifact map.
const FINALIZER_ACCEPTS: &[PayloadKind] = &[PayloadKind::Intermediate];

/// The **combine-everything** terminal stage: a code-defined [`SubAgent`] with **no LLM and no
/// tools** whose entire behaviour is [`render_report`].
///
/// It reads the analysis report — the message of its `report_source` stage, keyed
/// `{report_source}.message` (required; a missing report is a wiring error surfaced as
/// [`AgentError::MissingArtifact`], not a panic) — plus the optional `charts.spec`, then emits the
/// `Final` answer. The full upstream artifact map rides along on the result as provenance. Being
/// pure logic, it can neither fetch nor invent — the isolation boundary made concrete — and its
/// output is fully determined by the upstream artifacts.
///
/// # References
///
/// - Sub-agent plan §10 — the `finalizer` combines upstream artifacts into the answer
/// - Sub-agent contract §1.1 — Logic-only agents
pub struct Finalizer {
    id: SubAgentId,
    /// The stage whose `{id}.message` artifact is the analysis report to render.
    report_source: SubAgentId,
}

impl Finalizer {
    /// Builds a finalizer that renders `report_source`'s message as the report body.
    pub fn new(id: SubAgentId, report_source: SubAgentId) -> Self {
        Self { id, report_source }
    }

    /// Builds the finalizer with its canonical `finalizer` id, rendering the `analyst`'s message.
    pub fn default_stage() -> Self {
        Self::new(SubAgentId("finalizer".into()), SubAgentId("analyst".into()))
    }
}

#[async_trait]
impl SubAgent for Finalizer {
    fn id(&self) -> &SubAgentId {
        &self.id
    }

    fn accepts(&self) -> &'static [PayloadKind] {
        FINALIZER_ACCEPTS
    }

    async fn run(&self, input: AgentPayload) -> Result<AgentPayload, AgentError> {
        // The finalizer consumes only Intermediate; anything else falls (payload §2.4). The match
        // is both the accept-check and the destructure — total, never a panic.
        let data = match input {
            AgentPayload::Intermediate(d) => d,
            other => {
                return Err(AgentError::Mismatch {
                    expected: self.accepts(),
                    got: other.kind(),
                })
            }
        };

        // The report is required — the finalizer's whole job is to render the analyst's message.
        let report_key = ArtifactKey::message(&self.report_source.0);
        let report = match data.artifacts.get(&report_key) {
            Some(value) => value.to_string(),
            None => return Err(AgentError::MissingArtifact(report_key)),
        };
        let charts = charts_of(&data.artifacts);

        Ok(AgentPayload::Final(FinalResult {
            user: data.prompt,
            assistant: render_report(&report, &charts),
            now: data.now,          // carry the turn's timestamp onto the terminal result
            artifacts: data.artifacts, // full provenance rides along on the result
        }))
    }
}

// ===========================================================================
// TESTS — every stage is a pure async unit (scripted LLM + mock tools), and
// the four compose into the full `/agent` procedure end to end.
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::config::OutputShape;
    use crate::agent::engine::{resolve_pipeline, ConfiguredAgent, Orchestrator};
    use crate::agent::payload::{
        InitialPrompt, IntermediateData, LlmCapability, LlmMessage, LlmResponse, Tool, ToolCall,
        ToolOutcome, ToolSchema,
    };
    use crate::agent::tools::emit_chart_tool;
    use std::sync::{Arc, Mutex};

    // ── mocks: a scripted LLM and a fixed data tool, so no network, no config ──

    /// A scripted LLM: hand it the turns to replay and a stage becomes deterministic.
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

    /// A stand-in `bill_revenue`: returns fixed rows into `fetcher.records`, no MCP.
    struct FakeFetchTool;
    #[async_trait]
    impl Tool for FakeFetchTool {
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
            Ok(ToolOutcome::Produced(ArtifactValue::Json(serde_json::json!({
                "months": [{ "month": "2026-05", "revenue": 120 }, { "month": "2026-06", "revenue": 180 }]
            }))))
        }
    }

    fn message(text: &str) -> LlmResponse {
        LlmResponse::Message(text.into())
    }
    fn call(name: &str, arguments: serde_json::Value) -> LlmResponse {
        LlmResponse::ToolCalls(vec![ToolCall {
            id: "c1".into(),
            name: name.into(),
            arguments,
        }])
    }

    /// A well-formed two-month bar chart, as the model would pass it to `emit_chart`.
    fn chart_args() -> serde_json::Value {
        serde_json::json!({
            "charts": [{
                "version": 1, "chartType": "bar", "title": "近兩月營收",
                "data": [{ "name": "5月", "value": 120 }, { "name": "6月", "value": 180 }]
            }]
        })
    }

    /// A pinned turn timestamp — payload B makes `now` deterministic input, so tests fix it here.
    fn fixed_now() -> chrono::DateTime<chrono::FixedOffset> {
        chrono::DateTime::parse_from_rfc3339("2026-07-11T09:30:00+08:00").unwrap()
    }

    fn intermediate(prompt: &str, artifacts: HashMap<ArtifactKey, ArtifactValue>) -> AgentPayload {
        AgentPayload::Intermediate(IntermediateData {
            prompt: prompt.into(),
            artifacts,
            now: fixed_now(),
        })
    }

    // ── analyst: captures its prose report from the fetched material ──

    #[tokio::test]
    async fn analyst_writes_a_report_from_the_material_and_carries_data_forward() {
        let analyst = ConfiguredAgent::new(
            &analyst_config(),
            ScriptedLlm::arc(vec![message("## 營收分析\n5月 120，6月 180，月增 50%。")]),
            vec![], // no tools — the analyst only reasons over provided material
            OutputShape::Intermediate,
        );

        let mut artifacts = HashMap::new();
        artifacts.insert(
            ArtifactKey::fetcher_records(),
            ArtifactValue::Json(serde_json::json!({ "revenue": 180 })),
        );
        let out = analyst
            .run(intermediate("近兩月營收", artifacts))
            .await
            .unwrap();

        match out {
            AgentPayload::Intermediate(d) => {
                assert_eq!(
                    d.artifacts.get(&ArtifactKey::message("analyst")),
                    Some(&ArtifactValue::Text("## 營收分析\n5月 120，6月 180，月增 50%。".into()))
                );
                // the fetcher's data is still present for the charter downstream (append-only)
                assert!(d.artifacts.contains_key(&ArtifactKey::fetcher_records()));
            }
            other => panic!("expected Intermediate, got {:?}", other.kind()),
        }
    }

    // ── charter: emits a schema-validated chart batch into charts.spec ──

    #[tokio::test]
    async fn charter_emits_a_validated_charts_spec_and_carries_the_report_forward() {
        let charter = ConfiguredAgent::new(
            &charter_config(),
            ScriptedLlm::arc(vec![call("emit_chart", chart_args()), message("已產生圖表")]),
            vec![Box::new(emit_chart_tool())],
            OutputShape::Intermediate,
        );

        let mut artifacts = HashMap::new();
        artifacts.insert(
            ArtifactKey::message("analyst"),
            ArtifactValue::Text("## 營收分析\n近兩月成長。".into()),
        );
        artifacts.insert(
            ArtifactKey::fetcher_records(),
            ArtifactValue::Json(serde_json::json!({ "revenue": 180 })),
        );
        let out = charter
            .run(intermediate("近兩月營收", artifacts))
            .await
            .unwrap();

        match out {
            AgentPayload::Intermediate(d) => {
                match d.artifacts.get(&ArtifactKey::charts_spec()) {
                    Some(ArtifactValue::Json(v)) => {
                        assert_eq!(v["charts"][0]["chartType"], "bar");
                        assert_eq!(v["charts"][0]["title"], "近兩月營收");
                    }
                    other => panic!("expected charts.spec Json, got {other:?}"),
                }
                // the analyst's report survives for the finalizer
                assert!(d.artifacts.contains_key(&ArtifactKey::message("analyst")));
            }
            other => panic!("expected Intermediate, got {:?}", other.kind()),
        }
    }

    #[tokio::test]
    async fn charter_rejects_a_malformed_chart_then_produces_on_retry() {
        // The first call is a bad chart type; the SchemaTool rejects it (fed back), the model
        // corrects, and the second call produces charts.spec — "loop until valid" for free.
        let bad = serde_json::json!({ "charts": [{ "chartType": "donut", "title": "x", "data": [] }] });
        let charter = ConfiguredAgent::new(
            &charter_config(),
            ScriptedLlm::arc(vec![
                call("emit_chart", bad),
                call("emit_chart", chart_args()),
                message("已修正並產生圖表"),
            ]),
            vec![Box::new(emit_chart_tool())],
            OutputShape::Intermediate,
        );

        let mut artifacts = HashMap::new();
        artifacts.insert(
            ArtifactKey::message("analyst"),
            ArtifactValue::Text("## 報告".into()),
        );
        let out = charter
            .run(intermediate("近兩月營收", artifacts))
            .await
            .unwrap();

        match out {
            AgentPayload::Intermediate(d) => match d.artifacts.get(&ArtifactKey::charts_spec()) {
                Some(ArtifactValue::Json(v)) => assert_eq!(v["charts"][0]["chartType"], "bar"),
                other => panic!("expected charts.spec after retry, got {other:?}"),
            },
            other => panic!("expected Intermediate, got {:?}", other.kind()),
        }
    }

    #[tokio::test]
    async fn charter_skips_charts_for_chit_chat() {
        // No emit_chart call — the model judged the turn needs no chart. charts.spec stays absent.
        let charter = ConfiguredAgent::new(
            &charter_config(),
            ScriptedLlm::arc(vec![message("這是閒聊，不需要圖表。")]),
            vec![Box::new(emit_chart_tool())],
            OutputShape::Intermediate,
        );

        let mut artifacts = HashMap::new();
        artifacts.insert(
            ArtifactKey::message("analyst"),
            ArtifactValue::Text("你好！我是 EOMC 助理。".into()),
        );
        let out = charter
            .run(intermediate("你是誰", artifacts))
            .await
            .unwrap();

        match out {
            AgentPayload::Intermediate(d) => {
                assert!(!d.artifacts.contains_key(&ArtifactKey::charts_spec()));
                assert!(d.artifacts.contains_key(&ArtifactKey::message("analyst")));
            }
            other => panic!("expected Intermediate, got {:?}", other.kind()),
        }
    }

    // ── finalizer: pure logic — render, combine, and the falling convention ──

    #[test]
    fn render_report_appends_each_chart_as_a_falcon_block() {
        let charts = vec![
            serde_json::json!({ "version": 1, "chartType": "bar", "title": "A", "data": [] }),
            serde_json::json!({ "version": 1, "chartType": "line", "title": "B", "data": [] }),
        ];
        let out = render_report("## Analysis\nGood.", &charts);
        assert!(out.starts_with("## Analysis\nGood."));
        assert_eq!(out.matches("```falcon-chart").count(), 2);
        assert!(out.contains("\"title\": \"A\""));
        assert!(out.contains("\"title\": \"B\""));
        // every fence is closed
        assert_eq!(out.matches("```").count(), 4);
    }

    #[test]
    fn render_report_without_charts_is_just_the_report() {
        assert_eq!(render_report("hello\n\n", &[]), "hello");
        assert!(!render_report("hello", &[]).contains("falcon-chart"));
    }

    #[tokio::test]
    async fn finalizer_combines_report_and_charts_into_the_final_answer() {
        let mut artifacts = HashMap::new();
        artifacts.insert(
            ArtifactKey::message("analyst"),
            ArtifactValue::Text("## 營收分析\n近兩月成長。".into()),
        );
        artifacts.insert(
            ArtifactKey::charts_spec(),
            ArtifactValue::Json(serde_json::json!({
                "charts": [{ "version": 1, "chartType": "bar", "title": "近兩月營收", "data": [] }]
            })),
        );
        // an unrelated upstream artifact the finalizer must simply ignore
        artifacts.insert(
            ArtifactKey::fetcher_records(),
            ArtifactValue::Json(serde_json::json!({ "revenue": 180 })),
        );

        let out = Finalizer::default_stage()
            .run(intermediate("近兩月營收", artifacts))
            .await
            .unwrap();

        match out {
            AgentPayload::Final(f) => {
                assert_eq!(f.user, "近兩月營收");
                assert!(f.assistant.starts_with("## 營收分析"));
                assert!(f.assistant.contains("```falcon-chart"));
                assert!(f.assistant.contains("\"title\": \"近兩月營收\""));
            }
            other => panic!("expected Final, got {:?}", other.kind()),
        }
    }

    #[tokio::test]
    async fn finalizer_falls_on_a_missing_report() {
        // No analyst.message ⇒ a wiring error, surfaced typed (never a panic).
        let out = Finalizer::default_stage()
            .run(intermediate("x", HashMap::new()))
            .await;
        match out {
            Err(AgentError::MissingArtifact(k)) => {
                assert_eq!(k, ArtifactKey::message("analyst"))
            }
            other => panic!("expected MissingArtifact(analyst.message), got {other:?}"),
        }
    }

    #[tokio::test]
    async fn finalizer_falls_on_a_variant_it_does_not_accept() {
        let out = Finalizer::default_stage()
            .run(AgentPayload::Initial(InitialPrompt {
                prompt: "x".into(),
                history: vec![],
                now: fixed_now(),
            }))
            .await;
        assert!(matches!(
            out,
            Err(AgentError::Mismatch {
                got: PayloadKind::Initial,
                ..
            })
        ));
    }

    // ── the whole procedure: fetch → analyse → chart → finalize, end to end ──

    #[tokio::test]
    async fn agent_pipeline_runs_fetch_analyse_chart_finalize() {
        // Each stage gets its own scripted LLM; the fetcher and charter also get mock/real tools.
        let fetcher = ConfiguredAgent::new(
            &fetcher_config(),
            ScriptedLlm::arc(vec![
                call("bill_revenue", serde_json::json!({})),
                message("已取得近兩月營收"),
            ]),
            vec![Box::new(FakeFetchTool)],
            OutputShape::Intermediate,
        );
        let analyst = ConfiguredAgent::new(
            &analyst_config(),
            ScriptedLlm::arc(vec![message(
                "## 營收分析\n5月 NT$120，6月 NT$180，月增約 50%。",
            )]),
            vec![],
            OutputShape::Intermediate,
        );
        let charter = ConfiguredAgent::new(
            &charter_config(),
            ScriptedLlm::arc(vec![call("emit_chart", chart_args()), message("已產生圖表")]),
            vec![Box::new(emit_chart_tool())],
            OutputShape::Intermediate,
        );

        let mut agents: HashMap<SubAgentId, Arc<dyn SubAgent>> = HashMap::new();
        agents.insert(SubAgentId("fetcher".into()), Arc::new(fetcher));
        agents.insert(SubAgentId("analyst".into()), Arc::new(analyst));
        agents.insert(SubAgentId("charter".into()), Arc::new(charter));
        agents.insert(
            SubAgentId("finalizer".into()),
            Arc::new(Finalizer::default_stage()),
        );

        let pipe = crate::agent::config::PipelineConfig {
            id: agent_pipeline_id(),
            stages: vec![
                SubAgentId("fetcher".into()),
                SubAgentId("analyst".into()),
                SubAgentId("charter".into()),
                SubAgentId("finalizer".into()),
            ],
        };
        let mut orch = Orchestrator::new();
        orch.insert(
            agent_pipeline_id(),
            resolve_pipeline(&pipe, &agents).unwrap(),
        );

        let out = orch
            .run(
                &agent_pipeline_id(),
                AgentPayload::Initial(InitialPrompt {
                    prompt: "近兩月營收".into(),
                    history: vec![],
                    now: fixed_now(),
                }),
            )
            .await
            .unwrap();

        match out {
            AgentPayload::Final(f) => {
                // the turn's timestamp threads intact from the boundary to the terminal result
                assert_eq!(f.now, fixed_now());
                // the original question threads all the way to the terminal result
                assert_eq!(f.user, "近兩月營收");
                // the analyst's prose is the body…
                assert!(f.assistant.contains("## 營收分析"));
                assert!(f.assistant.contains("月增約 50%"));
                // …and the charter's validated chart is appended as a falcon-chart block
                assert!(f.assistant.contains("```falcon-chart"));
                assert!(f.assistant.contains("\"title\": \"近兩月營收\""));

                // the appended block is real, parseable falcon-chart JSON
                let block = f
                    .assistant
                    .split("```falcon-chart")
                    .nth(1)
                    .and_then(|s| s.split("```").next())
                    .expect("a falcon-chart block");
                let parsed: serde_json::Value =
                    serde_json::from_str(block.trim()).expect("valid chart JSON");
                assert_eq!(parsed["chartType"], "bar");
                assert_eq!(parsed["data"][1]["value"], 180.0);

                // provenance (open-key contract): the Final result carries the *whole* lossless
                // artifact map — every stage's tool outputs and messages — for audit, not just the
                // rendered answer.
                assert!(f.artifacts.contains_key(&ArtifactKey::fetcher_records()));
                assert!(f.artifacts.contains_key(&ArtifactKey::message("fetcher")));
                assert!(f.artifacts.contains_key(&ArtifactKey::message("analyst")));
                assert!(f.artifacts.contains_key(&ArtifactKey::message("charter")));
                assert!(f.artifacts.contains_key(&ArtifactKey::charts_spec()));
            }
            other => panic!("expected Final, got {:?}", other.kind()),
        }
    }
}
