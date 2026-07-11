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

//! The tool layer: the closed logical [`ToolId`], the backend-agnostic [`ToolRegistry`], the
//! generic validating [`SchemaTool<T>`], and the MCP-backed [`McpTool`].
//!
//! Port of the tool contract, plus the concrete [`McpTool`] backend (which the reference leaves
//! to the implementation plan).
//!
//! A tool is one abstraction over three kinds of backend — an MCP data fetch, a code-backed
//! output *sink*, or a code-backed *validator / compute*.
//! All three are dispatched identically by the tool-use loop and isolated identically by an
//! agent's grant.
//!
//! # References
//!
//! - Tool contract — `.spec/contract/tool/tool.rs`

#![allow(dead_code)] // groundwork: the closed set is seeded ahead of full registry wiring.

use std::collections::HashMap;
use std::fmt;
use std::marker::PhantomData;
use std::sync::Arc;

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::de::DeserializeOwned;

use crate::agent::chart::ChartBatch;
use crate::agent::events::{AgentEvent, EventSink};
use crate::agent::payload::{
    AgentError, ArtifactKey, ArtifactValue, Tool, ToolOutcome, ToolSchema,
};
use crate::mcp_client::McpHandle;

// ===========================================================================
// ToolId — the closed logical name (a typo is a parse error; unlike the now-open ArtifactKey,
// the tool set stays closed so every grant is boot-checked)
// ===========================================================================

/// The logical identifier of a tool, decoupled from any backend.
///
/// Closed on purpose: every grant and every registration is checked against it at boot.
/// The set grows as tools are added.
///
/// It is seeded from the tool contract's running examples.
/// The real datacenter set is authored in a later step by reading `main.rs`'s
/// `discovered MCP tools` boot log; `BillRevenue` is already a real datacenter endpoint, which
/// is why the fetcher can touch it today.
///
/// # References
///
/// - Tool contract §2 — the closed `ToolId` set
/// - Sub-agent plan §4 — authoring the real datacenter set from the boot log
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum ToolId {
    /// MCP-backed data fetch: bill revenue (a real datacenter endpoint).
    BillRevenue,
    /// Output sink: validate + emit a chart spec (code-backed, for the report `charter`).
    EmitChart,
    /// Validator/compute: a calculator (code-backed).
    Calculate,
    // EXTEND: one variant per logical tool the orchestration designer offers.
}

impl fmt::Display for ToolId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            ToolId::BillRevenue => "bill_revenue",
            ToolId::EmitChart => "emit_chart",
            ToolId::Calculate => "calculate",
        })
    }
}

impl std::str::FromStr for ToolId {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "bill_revenue" => Ok(ToolId::BillRevenue),
            "emit_chart" => Ok(ToolId::EmitChart),
            "calculate" => Ok(ToolId::Calculate),
            other => Err(format!("unknown ToolId: {other}")),
        }
    }
}

/// Every [`ToolId`] variant, so the registry can assert completeness at boot.
///
/// Auto-collection (a `#[tool]` macro plus `inventory`) would still be validated against this
/// list: the closed set stays the source of truth, auto-collection is only convenience.
///
/// # References
///
/// - Tool contract §3 — auto-collection reconciled with the closed set
pub const ALL_TOOL_IDS: &[ToolId] = &[ToolId::BillRevenue, ToolId::EmitChart, ToolId::Calculate];

// ===========================================================================
// Registry — backend-agnostic, boot-resolved, fail-fast
// ===========================================================================

/// A boot/resolution failure at the tool layer.
///
/// The sub-agent layer's grant resolution maps these onto its own
/// [`ResolveError`](crate::agent::config::ResolveError).
#[derive(Debug, PartialEq, Eq)]
pub enum ToolError {
    /// A granted/looked-up id has no registered backend.
    Unknown(ToolId),
    /// The closed set has an id with no backend at boot (completeness check).
    Unbacked(ToolId),
    /// Two backends registered for one id.
    Duplicate(ToolId),
}

impl fmt::Display for ToolError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ToolError::Unknown(id) => write!(f, "no registry entry for tool `{id}`"),
            ToolError::Unbacked(id) => {
                write!(f, "tool `{id}` is in the closed set but has no backend")
            }
            ToolError::Duplicate(id) => write!(f, "tool `{id}` registered more than once"),
        }
    }
}

impl std::error::Error for ToolError {}

/// Builds a fresh boxed [`Tool`] on demand — capturing an [`McpHandle`], a validator, etc.
///
/// Lets one logical [`ToolId`] be re-backed (MCP → HTTP → sink → mock) without touching any
/// grant.
pub type ToolFactory = Arc<dyn Fn() -> Box<dyn Tool> + Send + Sync>;

/// Designer-owned map from logical [`ToolId`] to a concrete backend.
///
/// A **closed** set: every grant is resolved against it at boot, and an unresolvable id fails
/// fast.
#[derive(Clone, Default)]
pub struct ToolRegistry {
    factories: HashMap<ToolId, ToolFactory>,
}

impl ToolRegistry {
    /// Creates an empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers a backend for a logical tool.
    ///
    /// The closed set has exactly one backend per id, so a duplicate registration fails fast.
    ///
    /// # Arguments
    ///
    /// - `id`: the logical tool being backed.
    /// - `factory`: builds a fresh boxed [`Tool`] each time the id is resolved.
    ///
    /// # Returns
    ///
    /// Returns `Ok(&mut self)` for chaining on success.
    ///
    /// # Errors
    ///
    /// - [`ToolError::Duplicate`] — the id already has a backend.
    pub fn register(&mut self, id: ToolId, factory: ToolFactory) -> Result<&mut Self, ToolError> {
        if self.factories.contains_key(&id) {
            return Err(ToolError::Duplicate(id));
        }
        self.factories.insert(id, factory);
        Ok(self)
    }

    /// Asserts every id in the closed set has a backend.
    ///
    /// This reconciles ergonomic auto-registration with the fail-fast / closed-set guarantee:
    /// however backends are collected, boot still verifies the map covers exactly the closed set.
    ///
    /// # Arguments
    ///
    /// - `all`: the closed set to check against (normally [`ALL_TOOL_IDS`]).
    ///
    /// # Errors
    ///
    /// - [`ToolError::Unbacked`] — an id in `all` has no registered backend.
    pub fn assert_complete(&self, all: &[ToolId]) -> Result<(), ToolError> {
        all.iter()
            .find(|id| !self.factories.contains_key(id))
            .map_or(Ok(()), |id| Err(ToolError::Unbacked(*id)))
    }

    /// Resolves a grant into concrete tools, **failing fast** on the first unknown id.
    ///
    /// # Arguments
    ///
    /// - `grants`: the [`ToolId`]s an agent is granted.
    ///
    /// # Returns
    ///
    /// Returns the boxed tools in grant order.
    ///
    /// # Errors
    ///
    /// - [`ToolError::Unknown`] — a granted id has no registered backend.
    pub fn resolve(&self, grants: &[ToolId]) -> Result<Vec<Box<dyn Tool>>, ToolError> {
        grants
            .iter()
            .map(|id| {
                self.factories
                    .get(id)
                    .map(|f| f())
                    .ok_or(ToolError::Unknown(*id))
            })
            .collect()
    }
}

// ===========================================================================
// SchemaTool<T> — the generic validating / sink adapter (code backend)
// ===========================================================================

/// Turns any `serde` + `schemars` type `T` into a [`Tool`].
///
/// The advertised schema is derived from `T`, and `call` **validates** the model's arguments by
/// deserializing into `T`.
/// A deserialization failure becomes a [`ToolOutcome::Rejected`] (fed back so the model
/// self-corrects), never a crash.
///
/// The `on_valid` step is the variable part:
///
/// - a **sink** ([`SchemaTool::sink`]) is the identity — the validated `T` *is* the artifact;
/// - a **validator / compute** ([`SchemaTool::new`]) transforms the validated `T` into some
///   [`ArtifactValue`], and may itself `Reject` on a domain rule (e.g. divide-by-zero).
pub struct SchemaTool<T> {
    name: String,
    description: String,
    target: ArtifactKey,
    on_valid: Arc<dyn Fn(T) -> Result<ArtifactValue, String> + Send + Sync>,
    _marker: PhantomData<fn(T)>,
}

impl<T> SchemaTool<T>
where
    T: JsonSchema + DeserializeOwned + Send + Sync + 'static,
{
    /// Builds a validator / compute tool.
    ///
    /// # Arguments
    ///
    /// - `name`: the advertised tool name.
    /// - `description`: the advertised description shown to the model.
    /// - `target`: the artifact slot a produced value fills.
    /// - `on_valid`: maps the validated `T` to an artifact, or returns `Err(reason)` for a
    ///   domain-level rejection (fed back to the model).
    pub fn new(
        name: impl Into<String>,
        description: impl Into<String>,
        target: ArtifactKey,
        on_valid: impl Fn(T) -> Result<ArtifactValue, String> + Send + Sync + 'static,
    ) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            target,
            on_valid: Arc::new(on_valid),
            _marker: PhantomData,
        }
    }
}

impl<T> SchemaTool<T>
where
    T: JsonSchema + DeserializeOwned + serde::Serialize + Send + Sync + 'static,
{
    /// Builds a pure **sink**: the validated `T`, serialized, *is* the artifact.
    ///
    /// This is the chart case — schema-enforced structured output, keyed to `target`.
    ///
    /// # Arguments
    ///
    /// - `name`: the advertised tool name.
    /// - `description`: the advertised description shown to the model.
    /// - `target`: the artifact slot the serialized `T` fills.
    pub fn sink(
        name: impl Into<String>,
        description: impl Into<String>,
        target: ArtifactKey,
    ) -> Self {
        Self::new(name, description, target, |value: T| {
            serde_json::to_value(&value)
                .map(ArtifactValue::Json)
                .map_err(|e| format!("serialize: {e}"))
        })
    }
}

#[async_trait]
impl<T> Tool for SchemaTool<T>
where
    T: JsonSchema + DeserializeOwned + Send + Sync + 'static,
{
    fn schema(&self) -> ToolSchema {
        let root = schemars::gen::SchemaGenerator::default().into_root_schema_for::<T>();
        ToolSchema {
            name: self.name.clone(),
            description: self.description.clone(),
            parameters: serde_json::to_value(root).unwrap_or_else(|_| serde_json::json!({})),
        }
    }

    fn target(&self) -> ArtifactKey {
        self.target.clone()
    }

    async fn call(&self, arguments: serde_json::Value) -> Result<ToolOutcome, AgentError> {
        // Schema enforcement: a shape the protocol type can't accept is REJECTED, not fatal —
        // the loop feeds the reason back and the model corrects (loop-until-valid).
        let parsed: T = match serde_json::from_value(arguments) {
            Ok(value) => value,
            Err(e) => {
                return Ok(ToolOutcome::Rejected {
                    reason: format!("schema: {e}"),
                })
            }
        };
        Ok(match (self.on_valid)(parsed) {
            Ok(value) => ToolOutcome::Produced(value),
            Err(reason) => ToolOutcome::Rejected { reason },
        })
    }
}

/// Builds the `emit_chart` sink: a [`SchemaTool`] over [`ChartBatch`] whose validated value lands
/// at [`ArtifactKey::charts_spec()`].
///
/// This is the report `charter`'s only tool and is **code-backed, not MCP** — the model calls it
/// with one or two falcon charts, a malformed shape is `Rejected` and fed back until valid
/// ("loop until valid" for free, tool contract), and the serialized [`ChartBatch`] becomes the
/// `charts.spec` artifact the `finalizer` renders.
///
/// The advertised name is the canonical [`ToolId::EmitChart`] string, keeping the LLM-facing
/// vocabulary in one place.
///
/// # References
///
/// - Sub-agent plan §10 — `emit_chart` is a code-registered `SchemaTool` sink
pub fn emit_chart_tool() -> SchemaTool<ChartBatch> {
    SchemaTool::<ChartBatch>::sink(
        ToolId::EmitChart.to_string(),
        "Emit the report's charts. Call once, passing one or two falcon charts (bar/line/pie) \
         built strictly from the fetched numbers. Skip it entirely — call no tool — for \
         chit-chat, greetings, or single-value answers.",
        ArtifactKey::charts_spec(),
    )
}

// ===========================================================================
// McpTool — the MCP data-fetch backend
// ===========================================================================

/// Coerces the model's tool arguments into the `serde_json::Map` shape MCP wants.
///
/// A missing / null argument object is the provider's way of encoding "no arguments" for a
/// zero-parameter tool, so it maps to an empty object.
/// Any *other* non-object shape is a malformed call.
///
/// # Returns
///
/// Returns `Ok(map)` with the coerced arguments, or `Err(reason)` describing the malformed
/// shape — which the caller turns into a retryable [`ToolOutcome::Rejected`], never a crash.
fn arg_object(
    arguments: serde_json::Value,
) -> Result<serde_json::Map<String, serde_json::Value>, String> {
    match arguments {
        serde_json::Value::Object(map) => Ok(map),
        serde_json::Value::Null => Ok(serde_json::Map::new()),
        other => Err(format!(
            "expected a JSON object for tool arguments, got {other}"
        )),
    }
}

/// A [`Tool`] backed by an MCP server.
///
/// The **advertised** name is the canonical [`ToolId`] string, so two servers exposing the same
/// raw name never collide within an agent's exposed set.
/// `call` sends `mcp_name` — the server's own raw name, which may differ — to that server.
///
/// `description` and `parameters` are the server's advertised schema, so the model knows how to
/// call it.
///
/// # References
///
/// - Tool contract §2.3 — advertised name is the canonical `ToolId` string
pub struct McpTool {
    handle: McpHandle,
    id: ToolId,
    mcp_name: String,
    description: String,
    parameters: serde_json::Value,
    target: ArtifactKey,
}

impl McpTool {
    /// Builds an MCP-backed tool.
    ///
    /// # Arguments
    ///
    /// - `handle`: the connected MCP client the call is sent through.
    /// - `id`: the logical tool id, whose canonical string is the LLM-facing name.
    /// - `mcp_name`: the raw name the server is actually asked for (may differ from `id`).
    /// - `description`: the server's advertised description.
    /// - `parameters`: the tool's JSON-Schema argument spec (typically the `input_schema`
    ///   discovered via [`McpHandle::list_openrouter_tools`]).
    /// - `target`: the artifact slot the tool's result fills.
    pub fn new(
        handle: McpHandle,
        id: ToolId,
        mcp_name: impl Into<String>,
        description: impl Into<String>,
        parameters: serde_json::Value,
        target: ArtifactKey,
    ) -> Self {
        Self {
            handle,
            id,
            mcp_name: mcp_name.into(),
            description: description.into(),
            parameters,
            target,
        }
    }
}

#[async_trait]
impl Tool for McpTool {
    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: self.id.to_string(), // §2.3: advertised name is the canonical ToolId string
            description: self.description.clone(),
            parameters: self.parameters.clone(),
        }
    }

    fn target(&self) -> ArtifactKey {
        self.target.clone()
    }

    async fn call(&self, arguments: serde_json::Value) -> Result<ToolOutcome, AgentError> {
        // A malformed argument shape is retryable (Rejected), not fatal.
        let args = match arg_object(arguments) {
            Ok(map) => map,
            Err(reason) => return Ok(ToolOutcome::Rejected { reason }),
        };
        // Transport / wiring failure is fatal (`Err`). Note `call_tool_text` already absorbs a
        // *tool-reported* error into `Ok(text)` (logging a warning), so the model still sees it
        // as data it can react to; only a genuine transport failure reaches this `Err` arm.
        match self.handle.call_tool_text(&self.mcp_name, args).await {
            Ok(text) => Ok(ToolOutcome::Produced(ArtifactValue::Text(text))),
            Err(e) => Err(AgentError::Capability(format!(
                "mcp tool `{}`: {e:#}",
                self.mcp_name
            ))),
        }
    }
}

// ===========================================================================
// StreamingTool — the event-emitting decorator (plan §8.5, mechanism A)
// ===========================================================================

/// A [`Tool`] decorator that emits an [`AgentEvent`] around the wrapped tool's execution, so a
/// streaming consumer sees tool activity live.
///
/// It delegates `schema` and `target` unchanged, so the tool-use loop's dispatch-by-name and
/// artifact keying are unaffected — the sink rides on the capability (Path A), and
/// [`run_llm_loop`](crate::agent::payload::run_llm_loop) never sees it.
///
/// The per-call `id` is deliberately absent: the loop dispatches by advertised name and does not
/// thread the model's call id through [`Tool::call`], so execution events carry `name` (a
/// [`AgentEvent::ToolCallProposed`] from the LLM adapter carries the id for correlation).
pub struct StreamingTool {
    inner: Box<dyn Tool>,
    name: String,
    target: ArtifactKey,
    sink: Arc<dyn EventSink>,
}

impl StreamingTool {
    /// Wraps one tool with a shared sink, capturing its advertised name and target up front.
    pub fn new(inner: Box<dyn Tool>, sink: Arc<dyn EventSink>) -> Self {
        let name = inner.schema().name;
        let target = inner.target();
        Self {
            inner,
            name,
            target,
            sink,
        }
    }

    /// Wraps every tool in a grant with one shared sink — the per-turn wiring of mechanism A.
    pub fn wrap_all(grant: Vec<Box<dyn Tool>>, sink: Arc<dyn EventSink>) -> Vec<Box<dyn Tool>> {
        grant
            .into_iter()
            .map(|t| Box::new(Self::new(t, sink.clone())) as Box<dyn Tool>)
            .collect()
    }
}

#[async_trait]
impl Tool for StreamingTool {
    fn schema(&self) -> ToolSchema {
        self.inner.schema()
    }

    fn target(&self) -> ArtifactKey {
        self.target.clone()
    }

    async fn call(&self, arguments: serde_json::Value) -> Result<ToolOutcome, AgentError> {
        self.sink.emit(AgentEvent::ToolStarted {
            name: self.name.clone(),
        });
        let outcome = self.inner.call(arguments).await;
        match &outcome {
            Ok(ToolOutcome::Produced(_)) => self.sink.emit(AgentEvent::ToolProduced {
                name: self.name.clone(),
                target: self.target.clone(),
            }),
            Ok(ToolOutcome::Rejected { reason }) => self.sink.emit(AgentEvent::ToolRejected {
                name: self.name.clone(),
                reason: reason.clone(),
            }),
            // A fatal error surfaces via `?` in the loop; no event (the orchestrator emits Error).
            Err(_) => {}
        }
        outcome
    }
}

// ===========================================================================
// TESTS — the tool contract's rules, exercised without a network
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use serde::{Deserialize, Serialize};

    // ── the "chart protocol" (a shared serde + schemars type) ──
    #[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
    #[serde(rename_all = "lowercase")]
    enum ChartType {
        Bar,
        Line,
        Pie,
    }
    #[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
    struct DataPoint {
        label: String,
        value: f64,
    }
    #[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
    struct ChartSpec {
        chart_type: ChartType,
        title: String,
        data_points: Vec<DataPoint>,
    }

    #[derive(Serialize, Deserialize, JsonSchema)]
    struct CalcArgs {
        a: f64,
        b: f64,
        op: String,
    }

    fn valid_chart() -> serde_json::Value {
        serde_json::json!({
            "chart_type": "bar",
            "title": "Q3 revenue",
            "data_points": [{ "label": "AC", "value": 1.0 }, { "label": "DC", "value": 2.0 }]
        })
    }

    // ── schema: valid args produce, bad shape rejects (not fatal) ──

    #[tokio::test]
    async fn schema_sink_produces_on_valid_and_advertises_a_schema() {
        let tool = SchemaTool::<ChartSpec>::sink(
            "emit_chart",
            "emit a chart",
            ArtifactKey::fetcher_records(),
        );
        assert_eq!(tool.schema().name, "emit_chart");
        assert!(tool.schema().parameters.get("properties").is_some());

        match tool.call(valid_chart()).await.unwrap() {
            ToolOutcome::Produced(ArtifactValue::Json(v)) => assert_eq!(v["title"], "Q3 revenue"),
            other => panic!("expected Produced(Json), got {other:?}"),
        }
    }

    #[tokio::test]
    async fn schema_sink_rejects_a_bad_shape_without_failing() {
        let tool = SchemaTool::<ChartSpec>::sink(
            "emit_chart",
            "emit a chart",
            ArtifactKey::fetcher_records(),
        );
        let bad = serde_json::json!({ "chart_type": "donut" });
        match tool.call(bad).await {
            Ok(ToolOutcome::Rejected { reason }) => assert!(reason.starts_with("schema:")),
            other => panic!("expected Ok(Rejected), got {other:?}"),
        }
    }

    #[tokio::test]
    async fn validator_can_reject_on_a_domain_rule() {
        let calc = SchemaTool::<CalcArgs>::new(
            "calculate",
            "arithmetic",
            ArtifactKey::fetcher_schema(),
            |args: CalcArgs| match args.op.as_str() {
                "div" if args.b == 0.0 => Err("division by zero".into()),
                "div" => Ok(ArtifactValue::Number(args.a / args.b)),
                "add" => Ok(ArtifactValue::Number(args.a + args.b)),
                other => Err(format!("unknown op: {other}")),
            },
        );
        let div0 = serde_json::json!({ "a": 1.0, "b": 0.0, "op": "div" });
        assert!(matches!(
            calc.call(div0).await.unwrap(),
            ToolOutcome::Rejected { .. }
        ));
        let ok = serde_json::json!({ "a": 6.0, "b": 2.0, "op": "div" });
        assert!(matches!(
            calc.call(ok).await.unwrap(),
            ToolOutcome::Produced(ArtifactValue::Number(_))
        ));
    }

    // ── emit_chart: the charter's sink, advertised name + validate/reject ──

    #[tokio::test]
    async fn emit_chart_tool_targets_charts_spec_and_validates_the_batch() {
        let tool = emit_chart_tool();
        assert_eq!(tool.schema().name, "emit_chart");
        assert_eq!(tool.target(), ArtifactKey::charts_spec());

        // A valid one-chart batch produces the serialized batch.
        let good = serde_json::json!({
            "charts": [{
                "version": 1, "chartType": "bar", "title": "近兩月營收",
                "data": [{ "name": "5月", "value": 120 }, { "name": "6月", "value": 180 }]
            }]
        });
        match tool.call(good).await.unwrap() {
            ToolOutcome::Produced(ArtifactValue::Json(v)) => {
                assert_eq!(v["charts"][0]["chartType"], "bar");
            }
            other => panic!("expected Produced(Json), got {other:?}"),
        }

        // A malformed chart type is a retryable rejection, not a crash.
        let bad = serde_json::json!({ "charts": [{ "chartType": "donut", "title": "x", "data": [] }] });
        assert!(matches!(
            tool.call(bad).await.unwrap(),
            ToolOutcome::Rejected { .. }
        ));
    }

    // ── registry: closed set, completeness, unknown grant, duplicate ──

    #[test]
    fn registry_completeness_and_unknown_are_fail_fast() {
        let mut reg = ToolRegistry::new();
        reg.register(
            ToolId::EmitChart,
            Arc::new(|| {
                Box::new(SchemaTool::<ChartSpec>::sink(
                    "emit_chart",
                    "",
                    ArtifactKey::fetcher_records(),
                ))
            }),
        )
        .unwrap();

        // Only EmitChart is backed → the closed set is incomplete.
        assert_eq!(
            reg.assert_complete(ALL_TOOL_IDS),
            Err(ToolError::Unbacked(ToolId::BillRevenue))
        );
        // A grant for an unbacked id fails fast.
        assert_eq!(
            reg.resolve(&[ToolId::Calculate]).err(),
            Some(ToolError::Unknown(ToolId::Calculate))
        );
        // A grant for a backed id resolves.
        assert_eq!(reg.resolve(&[ToolId::EmitChart]).unwrap().len(), 1);
        // Double registration is rejected.
        assert_eq!(
            reg.register(
                ToolId::EmitChart,
                Arc::new(|| {
                    Box::new(SchemaTool::<ChartSpec>::sink(
                        "emit_chart",
                        "",
                        ArtifactKey::fetcher_records(),
                    ))
                })
            )
            .err(),
            Some(ToolError::Duplicate(ToolId::EmitChart))
        );
    }

    // ── ToolId <-> canonical string ──

    #[test]
    fn tool_id_round_trips_through_its_canonical_string() {
        for id in ALL_TOOL_IDS {
            assert_eq!(id.to_string().parse::<ToolId>().unwrap(), *id);
        }
        assert!("not_a_tool".parse::<ToolId>().is_err());
    }

    // ── McpTool: advertised name and argument coercion (no network) ──

    #[test]
    fn arg_object_coerces_null_and_object_but_rejects_scalars() {
        assert!(arg_object(serde_json::Value::Null).unwrap().is_empty());
        assert_eq!(
            arg_object(serde_json::json!({ "year": 2026 })).unwrap()["year"],
            serde_json::json!(2026)
        );
        assert!(arg_object(serde_json::json!(42)).is_err());
        assert!(arg_object(serde_json::json!("x")).is_err());
    }

    // ── StreamingTool: emits Started then Produced/Rejected, delegates schema/target ──

    use crate::agent::events::test_support::CollectingSink;

    /// A tool that ignores its arguments and returns a fixed outcome — lets us assert exactly what
    /// the decorator emits without a real backend.
    struct FixedTool {
        outcome: ToolOutcome,
    }
    #[async_trait]
    impl Tool for FixedTool {
        fn schema(&self) -> ToolSchema {
            ToolSchema {
                name: "fixed".into(),
                description: String::new(),
                parameters: serde_json::json!({ "type": "object", "properties": {} }),
            }
        }
        fn target(&self) -> ArtifactKey {
            ArtifactKey::fetcher_records()
        }
        async fn call(&self, _args: serde_json::Value) -> Result<ToolOutcome, AgentError> {
            Ok(self.outcome.clone())
        }
    }

    #[tokio::test]
    async fn streaming_tool_emits_started_then_produced() {
        let sink = Arc::new(CollectingSink::new());
        let tool = StreamingTool::new(
            Box::new(FixedTool {
                outcome: ToolOutcome::Produced(ArtifactValue::Text("rows".into())),
            }),
            sink.clone(),
        );
        assert!(matches!(
            tool.call(serde_json::json!({})).await.unwrap(),
            ToolOutcome::Produced(_)
        ));
        assert_eq!(
            sink.events(),
            vec![
                AgentEvent::ToolStarted {
                    name: "fixed".into()
                },
                AgentEvent::ToolProduced {
                    name: "fixed".into(),
                    target: ArtifactKey::fetcher_records(),
                },
            ]
        );
    }

    #[tokio::test]
    async fn streaming_tool_emits_started_then_rejected_with_reason() {
        let sink = Arc::new(CollectingSink::new());
        let tool = StreamingTool::new(
            Box::new(FixedTool {
                outcome: ToolOutcome::Rejected {
                    reason: "bad shape".into(),
                },
            }),
            sink.clone(),
        );
        let _ = tool.call(serde_json::json!({})).await.unwrap();
        assert_eq!(
            sink.events(),
            vec![
                AgentEvent::ToolStarted {
                    name: "fixed".into()
                },
                AgentEvent::ToolRejected {
                    name: "fixed".into(),
                    reason: "bad shape".into(),
                },
            ]
        );
    }

    #[tokio::test]
    async fn streaming_tool_delegates_schema_and_target() {
        let sink = Arc::new(CollectingSink::new());
        let tool = StreamingTool::new(
            Box::new(FixedTool {
                outcome: ToolOutcome::Produced(ArtifactValue::Text("x".into())),
            }),
            sink,
        );
        assert_eq!(tool.schema().name, "fixed");
        assert_eq!(tool.target(), ArtifactKey::fetcher_records());
    }
}
