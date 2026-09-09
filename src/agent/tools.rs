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
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::de::DeserializeOwned;

use crate::agent::chart::ChartBatch;
use crate::agent::events::{AgentEvent, EventSink};
use crate::agent::payload::{
    AgentError, ArtifactKey, ArtifactValue, Tool, ToolOutcome, ToolSchema,
};
use crate::agent::report::ReportData;
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

/// Recursively decodes JSON that a model wrapped one level too deep as a **string**.
///
/// A value that is a string whose trimmed form begins with `{` or `[` and parses as JSON is
/// replaced by the parsed value; objects and arrays are walked so the repair reaches nested
/// fields. Plain scalar strings (a title, a note) are left untouched — only string-encoded
/// *objects/arrays* are decoded — so a legitimate text field is never corrupted.
///
/// This exists to accommodate models (e.g. Gemini) that serialize nested tool-call arguments as
/// JSON strings; it is applied only as a fallback when strict deserialization has already failed.
fn coerce_embedded_json(value: serde_json::Value) -> serde_json::Value {
    use serde_json::Value;
    match value {
        Value::String(s) => {
            let looks_like_json = matches!(s.trim_start().chars().next(), Some('{') | Some('['));
            match looks_like_json.then(|| serde_json::from_str::<Value>(&s)) {
                // Decoded an embedded object/array — recurse into it to repair any deeper strings.
                Some(Ok(decoded @ (Value::Object(_) | Value::Array(_)))) => {
                    coerce_embedded_json(decoded)
                }
                // Not actually JSON (or decoded to a scalar) — keep the original string.
                _ => Value::String(s),
            }
        }
        Value::Array(items) => Value::Array(items.into_iter().map(coerce_embedded_json).collect()),
        Value::Object(fields) => Value::Object(
            fields
                .into_iter()
                .map(|(k, v)| (k, coerce_embedded_json(v)))
                .collect(),
        ),
        scalar => scalar,
    }
}

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
        let parsed: T = match deserialize_with_path::<T>(arguments.clone()) {
            Ok(value) => value,
            Err(first_err) => {
                // Some models (notably Gemini) emit *nested* object/array arguments as JSON
                // **strings** rather than real JSON values — e.g. `report` arrives as
                // `"{\"title\":...}"`, which fails as `invalid type: string, expected struct`.
                // The content is correct; only the encoding is wrong. Recover by decoding any
                // embedded JSON strings and retrying once. This fires ONLY on the error path, so a
                // well-formed call is never touched.
                match deserialize_with_path::<T>(coerce_embedded_json(arguments.clone())) {
                    Ok(value) => value,
                    // Report the ORIGINAL error: it describes the shape the model actually sent.
                    Err(_) => {
                        return Ok(ToolOutcome::Rejected {
                            reason: reject_reason(&self.name, &first_err, &arguments),
                        })
                    }
                }
            }
        };
        Ok(match (self.on_valid)(parsed) {
            Ok(value) => ToolOutcome::Produced(value),
            Err(reason) => ToolOutcome::Rejected {
                reason: format!(
                    "{reason}. Nothing was recorded. Fix exactly that and call `{}` again.",
                    self.name
                ),
            },
        })
    }
}

// ===========================================================================
// Rejection feedback — the model's only channel for self-correction
// ===========================================================================

/// A deserialization failure that knows **where** it happened.
///
/// `serde_json`'s own error says *what* was wrong (`missing field \`kwh\``) but locates it only by
/// line/column of a buffer the model never saw. [`serde_path_to_error`] adds the JSON path
/// (`periods[3].kwh`), which is the part a model can actually act on.
struct PathError {
    /// The JSON path to the offending value, e.g. `periods[3].kwh`. Empty at the root.
    path: String,
    /// The underlying `serde` message, with its meaningless line/column tail stripped.
    message: String,
}

impl fmt::Display for PathError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.path.is_empty() {
            write!(f, "{}", self.message)
        } else {
            write!(f, "at `{}`: {}", self.path, self.message)
        }
    }
}

/// Deserializes `value` into `T`, keeping the JSON path of any failure.
fn deserialize_with_path<T: DeserializeOwned>(value: serde_json::Value) -> Result<T, PathError> {
    serde_path_to_error::deserialize(value).map_err(|e| {
        let path = e.path().to_string();
        // `serde_path_to_error` renders a root failure as ".", which reads like a real path.
        let path = if path == "." { String::new() } else { path };
        let message = e.into_inner().to_string();
        // Drop the ` at line 1 column 4271` tail: the model never saw that buffer, and the offset
        // only invites it to "fix" a position rather than a field.
        let message = match message.find(" at line ") {
            Some(cut) => message[..cut].to_string(),
            None => message,
        };
        PathError { path, message }
    })
}

/// Builds the feedback string a rejected schema call sends back to the model.
///
/// The loop hands this to the model verbatim as the tool result, so it is written *at* the model:
/// what failed, where, what it actually sent there, and what to do next. A bare
/// `schema: missing field \`kwh\`` leaves a model guessing which of 12 `periods` entries to fix;
/// this names the path and echoes the shape it sent, which is what turns a retry into a correction.
fn reject_reason(tool: &str, err: &PathError, arguments: &serde_json::Value) -> String {
    let mut reason = format!(
        "the arguments did not match the tool's schema, so NOTHING was recorded and this call did \
         not count. Problem {err}."
    );
    if let Some(observed) = describe_at_path(arguments, &err.path) {
        reason.push_str(&format!(" You sent there: {observed}."));
    }
    reason.push_str(&format!(
        " Fix only that, keep every other field you already had, and call `{tool}` again with the \
         COMPLETE payload — the schema in this tool's definition is authoritative, every required \
         field must be present, and nested values must be real JSON objects/arrays, never strings."
    ));
    reason
}

/// Renders a short, truncated description of whatever the model actually put at `path`.
///
/// `path` is [`serde_path_to_error`]'s rendering — dot-separated keys with bracketed sequence
/// indices, e.g. `periods[3].kwh`.
///
/// Returns `None` when the path does not resolve (a missing field — there is nothing to echo).
fn describe_at_path(arguments: &serde_json::Value, path: &str) -> Option<String> {
    let mut cursor = arguments;
    for segment in path.split('.').filter(|s| !s.is_empty()) {
        // A segment is `key`, `key[3]`, or a bare `[3]` — walk the key first, then each index.
        let (key, mut rest) = match segment.find('[') {
            Some(at) => (&segment[..at], &segment[at..]),
            None => (segment, ""),
        };
        if !key.is_empty() {
            cursor = cursor.get(key)?;
        }
        while let Some(close) = rest.find(']') {
            let index: usize = rest.get(1..close)?.parse().ok()?;
            cursor = cursor.get(index)?;
            rest = &rest[close + 1..];
        }
    }
    let rendered = match cursor {
        serde_json::Value::Object(map) => format!(
            "an object with keys [{}]",
            map.keys()
                .map(|k| format!("`{k}`"))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        serde_json::Value::Array(items) => format!("an array of {} items", items.len()),
        other => {
            let mut text = other.to_string();
            if text.chars().count() > 120 {
                text = text.chars().take(120).collect::<String>() + "…";
            }
            text
        }
    };
    Some(rendered)
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

/// Builds the `emit_report` sink: a [`SchemaTool`] over [`ReportData`] whose validated value lands
/// at [`ArtifactKey::report_data()`].
///
/// This is the report `composer`'s only tool and is **code-backed, not MCP** — the model calls it
/// once with the whole structured report (metadata, the insight narrative, the monthly periods, and
/// the station ranking) built strictly from the fetched numbers. A malformed shape is `Rejected`
/// and fed back until valid ("loop until valid" for free, tool contract), and the serialized
/// [`ReportData`] becomes the `report.data` artifact the pure-logic `renderer` injects into the
/// boot-loaded HTML template. The model never writes HTML — only this data.
///
/// # References
///
/// - Sub-agent plan §10 — `emit_report` is a code-registered `SchemaTool` sink
pub fn emit_report_tool() -> SchemaTool<ReportData> {
    SchemaTool::<ReportData>::sink(
        "emit_report",
        "Emit the whole operations report as structured data. Call exactly once with the report \
         metadata, the executive insight narrative, every monthly period, and the station ranking \
         — all built strictly from the fetched numbers, never invented. Do NOT write HTML; the \
         server renders it from this data.",
        ArtifactKey::report_data(),
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
            "the arguments must be a JSON object of this tool's named parameters, but a bare \
             {kind} was sent ({preview}). Nothing was recorded. Call the tool again, passing every \
             parameter by name.",
            kind = match other {
                serde_json::Value::Array(_) => "array",
                serde_json::Value::String(_) => "string",
                serde_json::Value::Bool(_) => "boolean",
                _ => "number",
            },
            preview = {
                let text = other.to_string();
                if text.chars().count() > 120 {
                    text.chars().take(120).collect::<String>() + "…"
                } else {
                    text
                }
            }
        )),
    }
}

/// How many times one [`McpTool`] instance absorbs a transport failure as retryable feedback
/// before giving up and failing the stage.
///
/// Tools are built per request, so this counter is per turn. The first failure is handed back to
/// the model as a [`ToolOutcome::Rejected`] — a single flaky call must not kill a six-tool report
/// fetch — but a tool that fails *again* means the server is genuinely unreachable, and a fatal
/// error is a far better outcome than a report silently composed from five sixths of the data.
const MCP_TRANSPORT_RETRIES: usize = 1;

/// A [`Tool`] backed by an MCP server.
///
/// `name` is the **advertised**, LLM-facing name; `call` sends `mcp_name` — the server's own raw
/// name, which may differ — to that server. Two constructors set `name`: [`new`](Self::new) from a
/// closed [`ToolId`] (its canonical string), and [`from_name`](Self::from_name) from a plain
/// string (the config-driven path, where the datacenter's wire names are the source of truth
/// rather than a compile-time enum).
///
/// `description` and `parameters` are the server's advertised schema, so the model knows how to
/// call it.
///
/// # References
///
/// - Tool contract §2.3 — advertised name distinct from the raw wire name
pub struct McpTool {
    handle: McpHandle,
    name: String,
    mcp_name: String,
    description: String,
    parameters: serde_json::Value,
    target: ArtifactKey,
    /// Transport failures absorbed so far this turn — see [`MCP_TRANSPORT_RETRIES`].
    transport_failures: AtomicUsize,
}

impl McpTool {
    /// Builds an MCP-backed tool from a plain advertised **name** (the config-driven path).
    ///
    /// # Arguments
    ///
    /// - `handle`: the connected MCP client the call is sent through.
    /// - `name`: the logical, LLM-facing tool name (a datacenter wire name, from config).
    /// - `mcp_name`: the raw name the server is actually asked for (may differ from `name`; equals
    ///   it for the single-server datacenter case).
    /// - `description`: the server's advertised description.
    /// - `parameters`: the tool's JSON-Schema argument spec (typically the `input_schema`
    ///   discovered via [`McpHandle::list_openrouter_tools`]).
    /// - `target`: the artifact slot the tool's result fills.
    pub fn from_name(
        handle: McpHandle,
        name: impl Into<String>,
        mcp_name: impl Into<String>,
        description: impl Into<String>,
        parameters: serde_json::Value,
        target: ArtifactKey,
    ) -> Self {
        Self {
            handle,
            name: name.into(),
            mcp_name: mcp_name.into(),
            description: description.into(),
            parameters,
            target,
            transport_failures: AtomicUsize::new(0),
        }
    }

    /// Builds an MCP-backed tool from a closed [`ToolId`] — its canonical string is the advertised
    /// name. Convenience over [`from_name`](Self::from_name) for the enumerated-tool path (tests).
    pub fn new(
        handle: McpHandle,
        id: ToolId,
        mcp_name: impl Into<String>,
        description: impl Into<String>,
        parameters: serde_json::Value,
        target: ArtifactKey,
    ) -> Self {
        Self::from_name(
            handle,
            id.to_string(),
            mcp_name,
            description,
            parameters,
            target,
        )
    }
}

#[async_trait]
impl Tool for McpTool {
    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: self.name.clone(), // the advertised, LLM-facing name (ToolId string or config wire name)
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
        // Note `call_tool_text` already absorbs a *tool-reported* error into `Ok(text)` (logging a
        // warning), so the model still sees it as data it can react to; only a genuine transport
        // failure reaches the `Err` arm below.
        match self.handle.call_tool_text(&self.mcp_name, args).await {
            Ok(text) => {
                self.transport_failures.store(0, Ordering::Relaxed);
                Ok(ToolOutcome::Produced(ArtifactValue::Text(text)))
            }
            // A transport failure is recoverable ONCE: the model is told, in words it can act on,
            // that this specific fetch produced nothing, and can retry it or carry on with the
            // remaining tools. A second failure of the same tool is fatal — see
            // `MCP_TRANSPORT_RETRIES`.
            Err(e) => {
                let seen = self.transport_failures.fetch_add(1, Ordering::Relaxed) + 1;
                if seen > MCP_TRANSPORT_RETRIES {
                    tracing::error!(
                        tool = %self.name,
                        failures = seen,
                        error = %format!("{e:#}"),
                        "mcp tool failed repeatedly — failing the stage"
                    );
                    return Err(AgentError::Capability(format!(
                        "mcp tool `{}` failed {seen} times: {e:#}",
                        self.mcp_name
                    )));
                }
                tracing::warn!(
                    tool = %self.name,
                    failures = seen,
                    error = %format!("{e:#}"),
                    "mcp tool transport failure — feeding it back to the model as retryable"
                );
                Ok(ToolOutcome::Rejected {
                    reason: format!(
                        "the data server could not be reached, so this call returned NO data and \
                         nothing was recorded. Transport error: {e:#}. You may call `{name}` once \
                         more with the same arguments. If it fails again, do NOT invent or estimate \
                         the missing figures — call the other granted tools, then state plainly \
                         which data could not be retrieved.",
                        name = self.name
                    ),
                })
            }
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
            Ok(ToolOutcome::Rejected { reason }) => {
                // The feedback must be actionable: name the offending field, quote what was sent,
                // say nothing was recorded, and tell the model to call again.
                assert!(reason.contains("`chart_type`"), "reason: {reason}");
                assert!(reason.contains("\"donut\""), "reason: {reason}");
                assert!(reason.contains("NOTHING was recorded"), "reason: {reason}");
                assert!(reason.contains("`emit_chart` again"), "reason: {reason}");
                // The line/column tail is noise the model cannot act on.
                assert!(!reason.contains("at line "), "reason: {reason}");
            }
            other => panic!("expected Ok(Rejected), got {other:?}"),
        }
    }

    // A failure deep inside a nested payload must be located by JSON *path*, not by byte offset:
    // "missing field `y`" is useless when there are twelve entries to choose between.
    #[tokio::test]
    async fn a_nested_rejection_names_the_json_path_and_echoes_what_was_sent() {
        let tool = SchemaTool::<ChartSpec>::sink(
            "emit_chart",
            "emit a chart",
            ArtifactKey::fetcher_records(),
        );
        let bad = serde_json::json!({
            "chart_type": "bar",
            "title": "revenue",
            "data_points": [
                { "label": "AC", "value": 1.0 },
                { "label": "DC", "value": "not-a-number" },
            ]
        });
        match tool.call(bad).await {
            Ok(ToolOutcome::Rejected { reason }) => {
                // The path pinpoints the offending element, not just "somewhere in data_points".
                assert!(reason.contains("data_points[1].value"), "reason: {reason}");
                // …and the value it actually sent there is echoed back.
                assert!(reason.contains("not-a-number"), "reason: {reason}");
            }
            other => panic!("expected Ok(Rejected), got {other:?}"),
        }
    }

    // ── Gemini quirk: a nested array/object arrives JSON-encoded as a *string*. The strict pass
    //    fails ("invalid type: string, expected a sequence"); the coercion fallback decodes it and
    //    the call still Produces. ──
    #[tokio::test]
    async fn schema_sink_coerces_a_stringified_nested_argument() {
        let tool = SchemaTool::<ChartSpec>::sink(
            "emit_chart",
            "emit a chart",
            ArtifactKey::fetcher_records(),
        );
        // `data_points` is a JSON *string*, not an array — exactly what Gemini sometimes emits.
        let stringified = serde_json::json!({
            "chart_type": "bar",
            "title": "Q3 revenue",
            "data_points": "[{ \"label\": \"AC\", \"value\": 1.0 }, { \"label\": \"DC\", \"value\": 2.0 }]"
        });
        match tool.call(stringified).await.unwrap() {
            ToolOutcome::Produced(ArtifactValue::Json(v)) => {
                assert_eq!(v["title"], "Q3 revenue");
                assert_eq!(v["data_points"][0]["label"], "AC");
            }
            other => panic!("expected Produced(Json) after coercion, got {other:?}"),
        }
    }

    // A plain-text field that merely *contains* no JSON is never mangled by the coercion pass, and
    // a genuinely malformed shape still Rejects (with the original, honest error).
    #[tokio::test]
    async fn coercion_leaves_plain_strings_and_still_rejects_bad_shapes() {
        let tool = SchemaTool::<ChartSpec>::sink(
            "emit_chart",
            "emit a chart",
            ArtifactKey::fetcher_records(),
        );
        // `data_points` is neither an array nor a JSON string — still an honest reject.
        let bad = serde_json::json!({
            "chart_type": "bar",
            "title": "{not json}",
            "data_points": 42
        });
        match tool.call(bad).await {
            Ok(ToolOutcome::Rejected { reason }) => {
                assert!(reason.contains("data_points"), "reason: {reason}");
                assert!(reason.contains("42"), "reason: {reason}");
            }
            other => panic!("expected Ok(Rejected), got {other:?}"),
        }
    }

    // A transport failure must be RECOVERABLE once — a single flaky call cannot be allowed to kill
    // a six-tool report fetch — and fatal on the second, because a server that is genuinely
    // unreachable must not yield a report quietly composed from five sixths of the data.
    #[tokio::test]
    async fn an_mcp_transport_failure_is_retryable_once_then_fatal() {
        let session = crate::test_support::stub_mcp_with_tools(["bill_revenue"]).await;
        let tool = McpTool::from_name(
            session.handle.clone(),
            "bill_revenue",
            "bill_revenue",
            "revenue",
            serde_json::json!({ "type": "object", "properties": {} }),
            ArtifactKey::new("fetcher", "bill_revenue"),
        );
        // Tear the session down: every call now fails at the transport, exactly as an unreachable
        // datacenter does.
        drop(session);

        // First failure: handed back to the model with something it can act on.
        match tool.call(serde_json::json!({})).await {
            Ok(ToolOutcome::Rejected { reason }) => {
                assert!(reason.contains("could not be reached"), "reason: {reason}");
                assert!(reason.contains("NO data"), "reason: {reason}");
                // It must be told what NOT to do with the gap.
                assert!(reason.contains("do NOT invent"), "reason: {reason}");
            }
            other => panic!("the first transport failure must be retryable, got {other:?}"),
        }

        // Second failure of the same tool: fatal, so the stage fails loudly.
        match tool.call(serde_json::json!({})).await {
            Err(AgentError::Capability(message)) => {
                assert!(message.contains("bill_revenue"), "message: {message}");
                assert!(message.contains("failed 2 times"), "message: {message}");
            }
            other => panic!("a repeated transport failure must be fatal, got {other:?}"),
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
        let bad =
            serde_json::json!({ "charts": [{ "chartType": "donut", "title": "x", "data": [] }] });
        assert!(matches!(
            tool.call(bad).await.unwrap(),
            ToolOutcome::Rejected { .. }
        ));
    }

    // ── emit_report: the composer's sink, advertised name + validate/reject ──

    /// A minimal well-formed [`ReportData`](crate::agent::report::ReportData) JSON, as the composer
    /// would pass it to `emit_report`.
    fn valid_report() -> serde_json::Value {
        serde_json::json!({
            "report": {
                "title": "營運報表", "organization": "EOMC", "brand": "Starcharger",
                "periodLabel": "2026年5月-6月", "dateFrom": "2026-05-01", "dateTo": "2026-06-30",
                "asOf": "2026-06-30", "locale": "zh-TW", "currency": "TWD",
                "partialPeriodNote": "2026-06 為部分月份數據。"
            },
            "summary": { "latestCompletedPeriod": "2026-05", "topStationPeriodLabel": "Q2 累計" },
            "insight": { "headline": "營收創高。", "paragraphs": ["5 月月增 13.7%。"] },
            "periods": [
                { "period": "2026-05", "revenue": 5805093, "revenueMom": 13.7, "kwh": 978061,
                  "sessions": 35861, "newMembers": 2888, "totalMembers": 42758,
                  "activeMembers": 7247, "stations": 248, "chargers": 796, "partial": false },
                { "period": "2026-06", "revenue": 4136808, "revenueMom": -28.7, "kwh": 794876,
                  "sessions": 29434, "newMembers": 2834, "totalMembers": 45592,
                  "activeMembers": 6758, "stations": 248, "chargers": 812, "partial": true }
            ],
            "stationRanking": [
                { "rank": 1, "name": "內湖堤頂專用站", "revenue": 1538139, "kwh": 291775,
                  "utilization": 13.1, "revenuePerKw": 16.56 }
            ]
        })
    }

    #[tokio::test]
    async fn emit_report_tool_targets_report_data_and_validates_the_payload() {
        let tool = emit_report_tool();
        assert_eq!(tool.schema().name, "emit_report");
        assert_eq!(tool.target(), ArtifactKey::report_data());

        // A well-formed payload produces the serialized report data.
        match tool.call(valid_report()).await.unwrap() {
            ToolOutcome::Produced(ArtifactValue::Json(v)) => {
                assert_eq!(v["summary"]["latestCompletedPeriod"], "2026-05");
                assert_eq!(v["stationRanking"][0]["name"], "內湖堤頂專用站");
            }
            other => panic!("expected Produced(Json), got {other:?}"),
        }

        // A missing required field (no `periods`) is a retryable rejection, not a crash.
        let mut bad = valid_report();
        bad.as_object_mut().unwrap().remove("periods");
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
