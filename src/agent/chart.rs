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

//! The **falcon-chart** protocol: the schema the `charter` emits and the `finalizer` renders.
//!
//! One shared `serde` + `schemars` type family drives both ends of the chart wire:
//!
//! - the `charter`'s `emit_chart` tool ([`emit_chart_tool`](crate::agent::tools::emit_chart_tool))
//!   is a [`SchemaTool`](crate::agent::tools::SchemaTool) over [`ChartBatch`] — the advertised
//!   argument schema and the on-receipt validation both derive from these types, so a malformed
//!   chart is `Rejected` and fed back until valid, never crashing;
//! - the `finalizer` ([`render_report`](crate::agent::pipeline::render_report)) reads the stored
//!   [`ChartBatch`] back and emits each [`FalconChart`] as its own ```` ```falcon-chart ```` block.
//!
//! The serialized shape matches the frontend contract in
//! `config/prompt_guide/agent_system.md` verbatim: `{ version, chartType, title, data:[{name,
//! value}] }`.
//!
//! # References
//!
//! - Sub-agent plan §10 — the `charter` emits schema-enforced chart artifacts
//! - `config/prompt_guide/charter_system.md` — the authored charting instruction

#![allow(dead_code)] // groundwork: consumed by the `charter` sink + `finalizer` once wired.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// The chart kind, mirroring the frontend's `chartType` field.
///
/// A closed set — a value outside it fails validation and is `Rejected` back to the model.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ChartType {
    /// Comparing discrete categories or a handful of periods side by side.
    Bar,
    /// Tracking one metric across an ordered time sequence.
    Line,
    /// How parts make up a whole across a few slices.
    Pie,
}

/// One `{name, value}` datum on a chart.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
pub struct ChartPoint {
    /// The category or period label (e.g. `"5月"`, `"內湖堤頂專用站"`).
    pub name: String,
    /// The figure — must come straight from the fetched data, never invented.
    pub value: f64,
}

/// The default schema version stamped on a chart when the model omits it.
fn default_version() -> u32 {
    1
}

/// A single falcon chart, serializing to the frontend's `falcon-chart` JSON shape.
///
/// Field order is deliberate: `serde_json` serializes struct fields in declaration order, so a
/// rendered block reads `version → chartType → title → data`, matching the authored template.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct FalconChart {
    /// Schema version — always `1`. Defaulted when the model omits it.
    #[serde(default = "default_version")]
    pub version: u32,
    /// The chart kind.
    pub chart_type: ChartType,
    /// A short descriptive heading that serves as the chart's title.
    pub title: String,
    /// The plotted points, in display order.
    pub data: Vec<ChartPoint>,
}

/// The `emit_chart` argument — **one or two** charts submitted in a single call.
///
/// A batch (rather than one-chart-per-call) is what lets a single sink slot
/// ([`ArtifactKey::ChartsSpec`](crate::agent::payload::ArtifactKey::ChartsSpec)) hold the whole
/// chart part: a per-call sink would overwrite its slot on a second chart. An empty `charts`
/// (or not calling the tool at all) means "no chart" — the chit-chat / single-value case.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq, Default)]
pub struct ChartBatch {
    /// The charts the report should carry (empty ⇒ none).
    pub charts: Vec<FalconChart>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_batch() -> ChartBatch {
        ChartBatch {
            charts: vec![FalconChart {
                version: 1,
                chart_type: ChartType::Bar,
                title: "近兩月營收".into(),
                data: vec![
                    ChartPoint {
                        name: "5月".into(),
                        value: 120.0,
                    },
                    ChartPoint {
                        name: "6月".into(),
                        value: 180.0,
                    },
                ],
            }],
        }
    }

    #[test]
    fn serializes_to_the_falcon_chart_wire_shape_and_round_trips() {
        let batch = sample_batch();
        let v = serde_json::to_value(&batch).unwrap();
        // camelCase field + lowercase enum on the wire, matching the frontend contract.
        assert_eq!(v["charts"][0]["chartType"], "bar");
        assert_eq!(v["charts"][0]["version"], 1);
        assert_eq!(v["charts"][0]["title"], "近兩月營收");
        assert_eq!(v["charts"][0]["data"][1]["name"], "6月");
        assert_eq!(v["charts"][0]["data"][1]["value"], 180.0);
        // exact key ordering (version → chartType → title → data) for a stable rendered block.
        let one = serde_json::to_string(&batch.charts[0]).unwrap();
        assert!(one.starts_with(r#"{"version":1,"chartType":"bar","title":"#));

        let back: ChartBatch = serde_json::from_value(v).unwrap();
        assert_eq!(back, batch);
    }

    #[test]
    fn version_defaults_to_one_when_the_model_omits_it() {
        let chart: FalconChart = serde_json::from_value(serde_json::json!({
            "chartType": "line",
            "title": "近四週充電量",
            "data": [{ "name": "第1週", "value": 320 }]
        }))
        .unwrap();
        assert_eq!(chart.version, 1);
        assert_eq!(chart.chart_type, ChartType::Line);
    }

    #[test]
    fn an_unknown_chart_type_fails_deserialization() {
        // The `charter`'s sink turns exactly this failure into a retryable `Rejected`.
        let bad: Result<FalconChart, _> = serde_json::from_value(serde_json::json!({
            "chartType": "donut", "title": "x", "data": []
        }));
        assert!(bad.is_err());
    }

    #[test]
    fn an_empty_batch_is_valid_and_means_no_chart() {
        let batch: ChartBatch = serde_json::from_value(serde_json::json!({ "charts": [] })).unwrap();
        assert!(batch.charts.is_empty());
    }
}
