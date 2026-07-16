//! # Tool — the capability a sub-agent's LLM invokes
//!
//! This file layers on top of [`agent_payload`](../agent_payload/agent_payload.rs), which owns
//! the `Tool` trait, `ToolOutcome`, `ArtifactKey`/`ArtifactValue`, and the tool-use loop
//! (`run_llm_loop`). **This** file owns the *tool layer*: the closed logical `ToolId`, the
//! backend-agnostic `ToolRegistry` (with a boot-time completeness check), and the generic
//! [`SchemaTool`] adapter that turns any `serde` + `schemars` type into a validating,
//! self-correcting tool. See `Contract.md` for the normative rules.
//!
//! ## What binds (normative)
//!
//! - A **tool** is a *named capability the LLM invokes, whose result fills an artifact slot*
//!   (`Tool::target`). MCP data fetches, output **sinks** (validate the model's own structured
//!   output), and pure **validators/compute** (a calculator) are all the same `Tool`.
//! - `Tool::call` returns [`ToolOutcome`]: `Produced(v)` fills `target`; `Rejected { reason }`
//!   is **fed back to the model** (not recorded, not fatal) so it retries — this is what makes
//!   a validating tool "loop until valid" for free. A fatal `Err(AgentError)` aborts.
//! - `ToolId` is a **closed set**; the registry is resolved at boot and an unknown or
//!   unbacked id **fails fast** (never a deferred failure at first call).
//!
//! ## What is suggested (advisory)
//!
//! The `ToolRegistry`/`SchemaTool` encoding here, and the `#[tool]`-macro + auto-registration
//! ergonomics discussed in `Contract.md`. Swap freely as long as the rules above hold.

#![allow(dead_code)]

#[path = "../agent_payload/agent_payload.rs"]
mod agent_payload;

use std::collections::HashMap;
use std::fmt;
use std::marker::PhantomData;
use std::sync::Arc;

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::de::DeserializeOwned;

use agent_payload::{AgentError, ArtifactKey, ArtifactValue, Tool, ToolOutcome, ToolSchema};

// ===========================================================================
// ToolId — the closed logical name (parity with ArtifactKey: a typo is a parse error)
// ===========================================================================

/// The logical identifier of a tool, decoupled from any backend. Closed on purpose: every
/// grant and every registration is checked against it at boot. Grows as tools are added.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum ToolId {
    /// MCP-backed data fetch (example).
    BillRevenue,
    /// Output sink: validate + emit a chart spec (code-backed).
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

/// Every `ToolId` variant, so the registry can assert completeness at boot. A `#[tool]` macro
/// with `inventory`-style auto-registration (see `Contract.md`) would still be validated
/// against this list: the closed set stays the source of truth, auto-collection is convenience.
pub const ALL_TOOL_IDS: &[ToolId] = &[ToolId::BillRevenue, ToolId::EmitChart, ToolId::Calculate];

// ===========================================================================
// Registry — backend-agnostic, boot-resolved, fail-fast
// ===========================================================================

/// A boot/resolution failure at the tool layer. `sub_agent`'s grant resolution maps these onto
/// its own `ResolveError` (tool doesn't depend on the sub-agent layer).
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
            ToolError::Unbacked(id) => write!(f, "tool `{id}` is in the closed set but has no backend"),
            ToolError::Duplicate(id) => write!(f, "tool `{id}` registered more than once"),
        }
    }
}

impl std::error::Error for ToolError {}

/// Builds a fresh boxed [`Tool`] on demand — capturing an `McpHandle`, a validator, etc. Lets
/// one logical [`ToolId`] be re-backed (MCP → HTTP → sink → mock) without touching any grant.
pub type ToolFactory = Arc<dyn Fn() -> Box<dyn Tool> + Send + Sync>;

/// Designer-owned map from logical [`ToolId`] to a concrete backend. A **closed** set: every
/// grant is resolved against it at boot, and an unresolvable id fails fast.
#[derive(Clone, Default)]
pub struct ToolRegistry {
    factories: HashMap<ToolId, ToolFactory>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a backend for a logical tool. **Duplicate** registration fails fast — the
    /// closed set has exactly one backend per id.
    pub fn register(&mut self, id: ToolId, factory: ToolFactory) -> Result<&mut Self, ToolError> {
        if self.factories.contains_key(&id) {
            return Err(ToolError::Duplicate(id));
        }
        self.factories.insert(id, factory);
        Ok(self)
    }

    /// Assert every id in the closed set has a backend. The reconciliation of ergonomic
    /// auto-registration with the fail-fast/closed-set guarantee: however backends are
    /// collected (explicit calls, a `#[tool]` macro, `inventory`), boot still verifies the map
    /// covers exactly the closed set.
    pub fn assert_complete(&self, all: &[ToolId]) -> Result<(), ToolError> {
        all.iter()
            .find(|id| !self.factories.contains_key(id))
            .map_or(Ok(()), |id| Err(ToolError::Unbacked(*id)))
    }

    /// Resolve a grant into concrete tools, **failing fast** on the first unknown id.
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
// SchemaTool<T> — the generic validating / sink adapter
// ===========================================================================

/// Turns any `serde` + `schemars` type `T` into a [`Tool`]: the advertised schema is derived
/// from `T`, and `call` **validates** the model's arguments by deserializing into `T`. A
/// deserialization failure becomes a [`ToolOutcome::Rejected`] (fed back so the model
/// self-corrects), never a crash. The `on_valid` step is the variable part:
///
/// - a **sink** ([`SchemaTool::sink`]) is the identity — the validated `T` *is* the artifact;
/// - a **validator/compute** ([`SchemaTool::new`]) transforms the validated `T` into some
///   `ArtifactValue`, and may itself `Reject` on a domain rule (e.g. divide-by-zero).
///
/// This one adapter covers charts, calculators, and any future protocol-checked capability.
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
    /// A validator/compute tool: `on_valid` maps the validated `T` to an artifact, or returns
    /// `Err(reason)` for a domain-level rejection (fed back to the model).
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
    /// A pure **sink**: the validated `T`, serialized, *is* the artifact. This is the chart
    /// case — schema-enforced structured output, keyed to `target`.
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
        self.target
    }

    async fn call(&self, arguments: serde_json::Value) -> Result<ToolOutcome, AgentError> {
        // Schema enforcement: a shape the protocol type can't accept is REJECTED, not fatal —
        // the loop feeds the reason back and the model corrects (loop-until-valid).
        let parsed: T = match serde_json::from_value(arguments) {
            Ok(value) => value,
            Err(e) => return Ok(ToolOutcome::Rejected { reason: format!("schema: {e}") }),
        };
        Ok(match (self.on_valid)(parsed) {
            Ok(value) => ToolOutcome::Produced(value),
            Err(reason) => ToolOutcome::Rejected { reason },
        })
    }
}

// ===========================================================================
// TESTS — the tool contract's rules, exercised without a network
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use agent_payload::{run_llm_loop, LlmCapability, LlmMessage, LlmResponse, ToolCall};
    use serde::{Deserialize, Serialize};
    use std::sync::Mutex;

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

    // ── §schema: valid args produce, bad shape rejects (not fatal) ──

    #[tokio::test]
    async fn schema_sink_produces_on_valid_and_advertises_a_schema() {
        let tool = SchemaTool::<ChartSpec>::sink("emit_chart", "emit a chart", ArtifactKey::FetcherRecords);
        // the advertised schema is derived from the type and is non-empty
        assert_eq!(tool.schema().name, "emit_chart");
        assert!(tool.schema().parameters.get("properties").is_some());

        match tool.call(valid_chart()).await.unwrap() {
            ToolOutcome::Produced(ArtifactValue::Json(v)) => {
                assert_eq!(v["title"], "Q3 revenue");
            }
            other => panic!("expected Produced(Json), got {other:?}"),
        }
    }

    #[tokio::test]
    async fn schema_sink_rejects_a_bad_shape_without_failing() {
        let tool = SchemaTool::<ChartSpec>::sink("emit_chart", "emit a chart", ArtifactKey::FetcherRecords);
        // missing `title` and `data_points`, and an invalid chart_type
        let bad = serde_json::json!({ "chart_type": "donut" });
        match tool.call(bad).await {
            Ok(ToolOutcome::Rejected { reason }) => assert!(reason.starts_with("schema:")),
            other => panic!("expected Ok(Rejected), got {other:?}"),
        }
    }

    #[tokio::test]
    async fn validator_can_reject_on_a_domain_rule() {
        // A calculator: schema-valid args can still be rejected by a domain rule (÷0).
        let calc = SchemaTool::<CalcArgs>::new(
            "calculate",
            "arithmetic",
            ArtifactKey::FetcherSchema,
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

    // ── §registry: closed set, completeness, unknown grant ──

    #[test]
    fn registry_completeness_and_unknown_are_fail_fast() {
        let mut reg = ToolRegistry::new();
        reg.register(ToolId::EmitChart, Arc::new(|| {
            Box::new(SchemaTool::<ChartSpec>::sink("emit_chart", "", ArtifactKey::FetcherRecords))
        }))
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
            reg.register(ToolId::EmitChart, Arc::new(|| {
                Box::new(SchemaTool::<ChartSpec>::sink("emit_chart", "", ArtifactKey::FetcherRecords))
            }))
            .err(),
            Some(ToolError::Duplicate(ToolId::EmitChart))
        );
    }

    // ── §loop: a Rejected sink call is fed back and the model self-corrects ──

    /// A scripted LLM that first calls the sink with a bad shape, sees the REJECTED tool
    /// message, then calls it with a valid shape, then finishes.
    struct RetryingLlm {
        turns: Mutex<Vec<LlmResponse>>,
        saw_rejection: Mutex<bool>,
    }

    #[async_trait]
    impl LlmCapability for RetryingLlm {
        async fn chat(
            &self,
            messages: &[LlmMessage],
            _tools: &[ToolSchema],
        ) -> Result<LlmResponse, AgentError> {
            // Observe whether the loop fed a rejection back before the retry.
            if let Some(LlmMessage::Tool { content, .. }) = messages.last() {
                if content.starts_with("REJECTED:") {
                    *self.saw_rejection.lock().unwrap() = true;
                }
            }
            Ok(self.turns.lock().unwrap().remove(0))
        }
    }

    #[tokio::test]
    async fn rejected_sink_call_is_fed_back_then_the_artifact_lands() {
        let tools: Vec<Box<dyn Tool>> = vec![Box::new(SchemaTool::<ChartSpec>::sink(
            "emit_chart",
            "emit a chart",
            ArtifactKey::FetcherRecords,
        ))];
        let call = |args: serde_json::Value| {
            LlmResponse::ToolCalls(vec![ToolCall { id: "c1".into(), name: "emit_chart".into(), arguments: args }])
        };
        let llm = RetryingLlm {
            turns: Mutex::new(vec![
                call(serde_json::json!({ "chart_type": "donut" })), // rejected
                call(valid_chart()),                                 // accepted
                LlmResponse::Message("done".into()),                 // final
            ]),
            saw_rejection: Mutex::new(false),
        };

        let (text, produced) = run_llm_loop(&llm, "system", "make a chart", &tools).await.unwrap();

        assert_eq!(text, "done");
        assert!(*llm.saw_rejection.lock().unwrap(), "the rejection must be fed back");
        // The rejected attempt recorded nothing; only the valid one landed.
        let chart = produced.get(&ArtifactKey::FetcherRecords).expect("chart artifact");
        match chart {
            ArtifactValue::Json(v) => assert_eq!(v["title"], "Q3 revenue"),
            other => panic!("expected Json, got {other:?}"),
        }
    }
}
