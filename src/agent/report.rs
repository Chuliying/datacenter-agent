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

//! The **report data** protocol: the structured payload the `composer` emits and the `renderer`
//! injects into the boot-loaded HTML template.
//!
//! This is the design's core economy. A rendered report is ~99% static — the design-system CSS,
//! the layout skeleton, and the client-side JS that builds KPI cards, tables, and charts. The
//! **only** varying part is a small JSON data block. So instead of an LLM emitting a full HTML
//! document from scratch (slow, token-heavy, error-prone), the `composer` emits exactly this
//! [`ReportData`] via its schema-validated `emit_report`
//! ([`emit_report_tool`](crate::agent::tools::emit_report_tool)) sink — a malformed shape is
//! `Rejected` and fed back until valid, never crashing — and the pure-logic `renderer`
//! ([`render_report_html`](crate::agent::pipeline::render_report_html)) escapes it and drops it
//! into the template's single `__REPORT_DATA_JSON__` placeholder.
//!
//! One shared `serde` + `schemars` family drives both ends: `emit_report`'s advertised argument
//! schema and its on-receipt validation both derive from these types, and the serialized value
//! *is* the `report.data` artifact ([`ArtifactKey::report_data`](crate::agent::payload::ArtifactKey::report_data))
//! the renderer reads back.
//!
//! The wire shape matches the template's client-side reader verbatim (camelCase fields, the
//! `report` / `summary` / `insight` / `periods` / `stationRanking` top level). See
//! `config/report_template/report.html`.
//!
//! # References
//!
//! - Sub-agent plan §10 — the endpoint pipelines (`/report`: fetch → analyse → compose → render)
//! - `config/prompt_guide/report_composer_system.md` — the authored composing instruction

#![allow(dead_code)] // groundwork: consumed by the `composer` sink + `renderer` once wired.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// The whole report as structured data — the single artifact the `renderer` injects.
///
/// Field order is deliberate (`serde_json` serializes in declaration order): the injected JSON
/// reads `report → summary → insight → periods → stationRanking`, matching the template's reader.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
pub struct ReportData {
    /// Report-level metadata: titles, the period window, and data-quality notes.
    pub report: ReportMeta,
    /// Derived pointers the template needs to compute KPIs (the latest *complete* period, etc.).
    pub summary: ReportSummary,
    /// The executive narrative panel — the analyst's insight, folded in by the composer.
    pub insight: ReportInsight,
    /// One entry per month, oldest first. The trailing month may be `partial`.
    pub periods: Vec<Period>,
    /// Stations ranked by revenue, rank `1` first.
    #[serde(rename = "stationRanking")]
    pub station_ranking: Vec<StationRank>,
}

/// Report-level metadata: the header, the period window, and the data-quality note.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ReportMeta {
    /// The report's main title (e.g. `"充電網路營運報表"`).
    pub title: String,
    /// The operating organization / network name (e.g. `"EOMC 充電網路"`).
    pub organization: String,
    /// The brand line shown beside the title (e.g. `"Starcharger 星舟"`).
    pub brand: String,
    /// Human-readable period label (e.g. `"2026年1月-6月"`).
    pub period_label: String,
    /// Reporting window start, `YYYY-MM-DD`.
    pub date_from: String,
    /// Reporting window end, `YYYY-MM-DD`.
    pub date_to: String,
    /// Data-as-of date, `YYYY-MM-DD`.
    pub as_of: String,
    /// BCP-47 locale for number/label formatting (e.g. `"zh-TW"`).
    pub locale: String,
    /// ISO 4217 currency code (e.g. `"TWD"`).
    pub currency: String,
    /// The partial-period / data-quality note rendered under the monthly table.
    pub partial_period_note: String,
}

/// Derived pointers the template's KPI logic needs.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ReportSummary {
    /// The most recent **complete** month (`YYYY-MM`); KPIs and MoM anchor here, never on a
    /// partial trailing month. Must reference a `periods` entry whose `partial` is `false`.
    pub latest_completed_period: String,
    /// The display label for the station-ranking window (e.g. `"2026年 Q2 累計"`).
    pub top_station_period_label: String,
}

/// The executive narrative panel — a short headline plus a few commentary paragraphs.
///
/// The analyst authors this prose (grounded in the fetched numbers); the composer carries it into
/// [`ReportData`], and the renderer shows it as a panel above the tables. Empty `paragraphs` (and
/// an empty `headline`) render nothing — the panel is hidden.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq, Default)]
pub struct ReportInsight {
    /// A one-line executive takeaway (bold lead of the panel). May be empty.
    pub headline: String,
    /// Two to four short commentary paragraphs, in display order. Empty ⇒ no body.
    pub paragraphs: Vec<String>,
}

/// One month's operating figures.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Period {
    /// The month, `YYYY-MM`.
    pub period: String,
    /// Total revenue for the month, in `report.currency` minor-agnostic units.
    pub revenue: f64,
    /// Month-over-month revenue change, as a percentage in `[-100, 100]`.
    pub revenue_mom: f64,
    /// Energy delivered, kWh.
    pub kwh: f64,
    /// Charging sessions.
    pub sessions: u64,
    /// New members registered in the month.
    pub new_members: u64,
    /// Cumulative member count at month end.
    pub total_members: u64,
    /// Members active in the month.
    pub active_members: u64,
    /// Stations live at month end.
    pub stations: u64,
    /// Chargers live at month end.
    pub chargers: u64,
    /// Whether this month is an in-progress / partial period (marked, and excluded from trends).
    pub partial: bool,
}

/// One station's ranking-table row.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct StationRank {
    /// 1-based rank by revenue (rank `1` is highlighted by the template).
    pub rank: u32,
    /// The station name.
    pub name: String,
    /// The station's revenue over the ranking window.
    pub revenue: f64,
    /// Energy delivered, kWh.
    pub kwh: f64,
    /// Utilization, as a percentage in `[0, 100]`.
    pub utilization: f64,
    /// Revenue per installed kW.
    pub revenue_per_kw: f64,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> ReportData {
        ReportData {
            report: ReportMeta {
                title: "充電網路營運報表".into(),
                organization: "EOMC 充電網路".into(),
                brand: "Starcharger 星舟".into(),
                period_label: "2026年5月-6月".into(),
                date_from: "2026-05-01".into(),
                date_to: "2026-06-30".into(),
                as_of: "2026-06-30".into(),
                locale: "zh-TW".into(),
                currency: "TWD".into(),
                partial_period_note: "2026-06 為部分月份數據。".into(),
            },
            summary: ReportSummary {
                latest_completed_period: "2026-05".into(),
                top_station_period_label: "2026年 Q2 累計".into(),
            },
            insight: ReportInsight {
                headline: "5 月營收創高，成長動能穩健。".into(),
                paragraphs: vec!["5 月營收 NT$5.8M，月增 13.7%。".into()],
            },
            periods: vec![
                Period {
                    period: "2026-05".into(),
                    revenue: 5_805_093.0,
                    revenue_mom: 13.7,
                    kwh: 978_061.0,
                    sessions: 35_861,
                    new_members: 2_888,
                    total_members: 42_758,
                    active_members: 7_247,
                    stations: 248,
                    chargers: 796,
                    partial: false,
                },
                Period {
                    period: "2026-06".into(),
                    revenue: 4_136_808.0,
                    revenue_mom: -28.7,
                    kwh: 794_876.0,
                    sessions: 29_434,
                    new_members: 2_834,
                    total_members: 45_592,
                    active_members: 6_758,
                    stations: 248,
                    chargers: 812,
                    partial: true,
                },
            ],
            station_ranking: vec![StationRank {
                rank: 1,
                name: "內湖堤頂專用站".into(),
                revenue: 1_538_139.0,
                kwh: 291_775.0,
                utilization: 13.1,
                revenue_per_kw: 16.56,
            }],
        }
    }

    #[test]
    fn serializes_to_the_template_wire_shape_and_round_trips() {
        let data = sample();
        let v = serde_json::to_value(&data).unwrap();

        // Top-level key order and camelCase station ranking, matching the template reader.
        assert!(v.get("report").is_some());
        assert!(v.get("summary").is_some());
        assert!(v.get("insight").is_some());
        assert_eq!(v["periods"][0]["period"], "2026-05");
        assert_eq!(v["stationRanking"][0]["name"], "內湖堤頂專用站");

        // camelCase field renames on the wire.
        assert_eq!(v["report"]["periodLabel"], "2026年5月-6月");
        assert_eq!(v["summary"]["latestCompletedPeriod"], "2026-05");
        assert_eq!(v["periods"][0]["revenueMom"], 13.7);
        assert_eq!(v["periods"][1]["partial"], true);
        assert_eq!(v["stationRanking"][0]["revenuePerKw"], 16.56);
        assert_eq!(v["insight"]["headline"], "5 月營收創高，成長動能穩健。");

        let back: ReportData = serde_json::from_value(v).unwrap();
        assert_eq!(back, data);
    }

    #[test]
    fn insight_defaults_to_empty_and_renders_nothing() {
        // An absent insight (the template hides the panel) round-trips through the default.
        let insight = ReportInsight::default();
        assert!(insight.headline.is_empty());
        assert!(insight.paragraphs.is_empty());
    }

    #[test]
    fn integer_and_number_fields_keep_their_json_kinds() {
        // Counts serialize as JSON integers; money/energy/percent as JSON numbers — the schema the
        // model sees constrains it accordingly.
        let v = serde_json::to_value(sample()).unwrap();
        assert!(v["periods"][0]["sessions"].is_u64());
        assert!(v["periods"][0]["revenue"].is_f64());
        assert!(v["stationRanking"][0]["rank"].is_u64());
    }
}
