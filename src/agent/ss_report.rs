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

//! The **星星電力 (SS) report data** protocol: the structured payload the `ss-report` composer emits
//! and the renderer injects into the boot-loaded SS HTML template.
//!
//! This is [`crate::agent::report`]'s sibling, and it exists as a *separate* protocol rather than a
//! generalization of [`ReportData`](crate::agent::report::ReportData) because the two businesses
//! have genuinely different shapes:
//!
//! | | Starcharger (`/agent/stream` report) | 星星電力 (`/ss-report`) |
//! |---|---|---|
//! | Data model | one monthly time series, every metric per month | six **heterogeneous snapshots**, one per tool |
//! | Time axis | shared across every metric | three tools take a window, three are "right now" |
//! | Absent topic | a `0` in a period row | a whole section with no rows |
//!
//! Forcing SS into a `periods[]` spine would have meant inventing a month axis that four of the six
//! tools do not have (`ss_power_pipeline`, `ss_btm_projects`, `ss_ftm_projects` are current-stage
//! snapshots; `ss_sunshine_hours` is a comparison snapshot, explicitly *not* a series). So the SS
//! payload is **section-shaped**: one field per tool, each carrying its own period label and rows.
//!
//! # Two design rules learned from the starcharger report
//!
//! 1. **Empty means empty, never zero.** Every section's content is a `Vec`, and an empty `Vec`
//!    makes the template *hide* that section. The starcharger schema's fixed `periods[]` fields
//!    forced the composer to zero-fill a topic the fetcher never fetched, which rendered as a flat
//!    chart no reader could distinguish from a real zero. Here an unfetched tool simply yields no
//!    section — visibly absent, and honestly so. The SS tool grant is narrowed per caller by their
//!    Falcon permissions, so a partial report is the normal case, not the failure case.
//! 2. **Flat rows, not nested objects.** The composer runs at minimal reasoning (it transcribes,
//!    it does not reason), and nested arrays-of-objects-of-arrays are where such a model truncates
//!    or string-encodes. FTM construction progress is therefore three flat vectors
//!    (`projects`, `stages`, `receivables`) keyed by project *name*, and the template does the
//!    grouping — not one `projects[].stages[]` tree.
//!
//! The wire shape matches the template's client-side reader verbatim (camelCase fields, the
//! `report` / `insight` / `solar` / `storage` / `wheeling` / `pipeline` / `btm` / `ftm` top level).
//! See `config/report_template/ss_report.html`.
//!
//! # References
//!
//! - `config/prompt_guide/ss_report_composer_system.md` — the authored composing instruction
//! - `config/prompt_guide/ss_fetcher_system.md` — the six `ss_*` tools and their response shapes

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::agent::report::ReportInsight;

/// The whole SS report as structured data — the single artifact the renderer injects.
///
/// Field order is deliberate (`serde_json` serializes in declaration order): the injected JSON
/// reads header → narrative → the six tool sections, in the order the template lays them out.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
pub struct SsReportData {
    /// Report-level metadata: titles, the period framing, and the data-scope note.
    pub report: SsReportMeta,
    /// The executive narrative panel — the analyst's insight, folded in by the composer.
    pub insight: ReportInsight,
    /// `ss_sunshine_hours` — the nine-station solar comparison snapshot.
    pub solar: SolarSection,
    /// `ss_energy_storage` — FTM bid statistics and per-case revenue.
    pub storage: StorageSection,
    /// `ss_power_wheeling` — green-power wheeling and revenue per site.
    pub wheeling: WheelingSection,
    /// `ss_power_pipeline` — the sell / purchase development funnels.
    pub pipeline: PipelineSection,
    /// `ss_btm_projects` — behind-the-meter case funnels.
    pub btm: BtmSection,
    /// `ss_ftm_projects` — front-of-meter construction progress.
    pub ftm: FtmSection,
}

/// Report-level metadata: the header, the period framing, and the data-scope note.
///
/// There is deliberately no single `dateFrom`/`dateTo` pair as the starcharger report has: the six
/// tools do not share one window (three take a period, three are unfiltered "right now"), so a
/// single range in the header would be a claim the data does not support. [`Self::period_label`]
/// frames the windowed tools and [`Self::snapshot_label`] frames the rest.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SsReportMeta {
    /// The report's main title (e.g. `"營運綜合報告"`).
    pub title: String,
    /// The operating organization (e.g. `"星星電力"`).
    pub organization: String,
    /// The brand line shown beside the title (e.g. `"投資平台"`).
    pub brand: String,
    /// Human-readable label for the **windowed** tools (e.g. `"2026 年第 2 季"`).
    pub period_label: String,
    /// Human-readable label for the **unfiltered snapshot** tools (e.g. `"即時快照"`).
    pub snapshot_label: String,
    /// Report generation / data-as-of date, `YYYY-MM-DD`.
    pub as_of: String,
    /// BCP-47 locale for number/label formatting (e.g. `"zh-TW"`).
    pub locale: String,
    /// ISO 4217 currency code (e.g. `"TWD"`).
    pub currency: String,
    /// The data-scope note rendered in the report footer.
    ///
    /// This is where a **partial report is disclosed**: when a section is empty because its tool
    /// was not granted, was not called, or returned nothing, say so here by name. An empty section
    /// is hidden by the template, so this note is the only place a reader learns it is missing.
    pub data_note: String,
}

// ===========================================================================
// ss_sunshine_hours — the nine-station solar comparison snapshot
// ===========================================================================

/// `ss_sunshine_hours`: sunshine hours per weather station, with its own baselines.
///
/// Not a time series — one row per station carrying the current period beside the year-ago and
/// previous-period baselines, exactly as the tool returns it.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct SolarSection {
    /// The period this snapshot covers (e.g. `"2026 年第 2 季"`). Empty ⇒ the meta period label.
    pub label: String,
    /// One row per weather station, highest current hours first. Empty ⇒ the section is hidden.
    pub stations: Vec<SolarStation>,
}

/// One weather station's sunshine-hour comparison.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SolarStation {
    /// The station name (e.g. `"臺南"`).
    pub name: String,
    /// The station's upstream id (e.g. `"467410"`).
    pub station_id: String,
    /// Sunshine hours in the current period.
    pub current_hours: f64,
    /// Sunshine hours in the same period a year earlier. `0` when the tool returned `null`.
    pub prev_year_hours: f64,
    /// Sunshine hours in the immediately preceding period. `0` when the tool returned `null`.
    pub prev_period_hours: f64,
    /// Year-over-year change, as a percentage.
    pub yoy_pct: f64,
    /// Change against the preceding period, as a percentage.
    pub period_change_pct: f64,
}

// ===========================================================================
// ss_energy_storage — FTM bid statistics + per-case revenue
// ===========================================================================

/// `ss_energy_storage`: market-wide bid statistics plus this platform's per-case revenue.
///
/// The two vectors come from the tool's two independent blocks (`bid_status`, `case_income`) and
/// are kept apart because they are not row-comparable: one is a market bucket, the other a case.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct StorageSection {
    /// The cumulative window this covers (e.g. `"2026 年 1–4 月累計"`).
    pub label: String,
    /// `bid_status`: one row per period bucket, oldest first. Empty ⇒ no bid panel.
    pub bids: Vec<StorageBid>,
    /// `case_income`: per period × case revenue, highest service income first. Empty ⇒ no panel.
    pub cases: Vec<StorageCase>,
}

/// One period's FTM energy-storage bid statistics.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct StorageBid {
    /// The period bucket, as the tool labels it (e.g. `"2026-01"`).
    pub period: String,
    /// Market-wide offered capacity, MW.
    pub market_supply_mw: f64,
    /// Market-wide cleared capacity, MW.
    pub market_cleared_mw: f64,
    /// Clearing rate, as a percentage in `[0, 100]`.
    pub win_rate_pct: f64,
    /// This platform's submitted bid capacity, MW.
    pub platform_bid_mw: f64,
    /// This platform's capacity that did not clear, MW.
    pub platform_lost_mw: f64,
    /// The clearing price per MW.
    pub clearing_price: f64,
}

/// One case's estimated storage revenue for one period.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct StorageCase {
    /// The period bucket (e.g. `"2026-04"`).
    pub period: String,
    /// The case / site name (e.g. `"星星花蓮三"`).
    pub case: String,
    /// The investment platform / SPV owning it (e.g. `"富邦平台 / 匯旭"`).
    pub platform: String,
    /// Capacity fee.
    pub capacity_fee: f64,
    /// Estimated energy service fee.
    pub energy_fee: f64,
    /// Estimated performance service fee.
    pub performance_fee: f64,
    /// Estimated total service income (the sum the report ranks on).
    pub service_income: f64,
    /// Whether the upstream flagged these figures as verified (`is_true_value`).
    ///
    /// The sample data has this `false` throughout, so the template marks unverified rows rather
    /// than hiding them — an estimate labelled as such is useful; one passed off as final is not.
    pub verified: bool,
}

// ===========================================================================
// ss_power_wheeling — green-power wheeling and revenue
// ===========================================================================

/// `ss_power_wheeling`: generation, wheeled energy and fees per site per period.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct WheelingSection {
    /// The cumulative window this covers (e.g. `"2025 年上半年累計"`).
    pub label: String,
    /// How many rows the tool returned in total, before any cap.
    ///
    /// This is the **honesty field**. `ss_power_wheeling` multiplies sites by periods — a
    /// year-to-date quarter query returns north of a hundred rows — so the report caps the detail
    /// table rather than printing all of them. A cap is fine; a cap the reader cannot see is not.
    /// When this exceeds `sites.len()`, the template says so beside the panel title.
    ///
    /// Set it equal to `sites.len()` when nothing was dropped.
    pub total_rows: u64,
    /// One row per period × site, highest generation first. Empty ⇒ the section is hidden.
    ///
    /// May be a capped subset — see [`Self::total_rows`]. The chart aggregates by site across
    /// periods, so a cap costs the reader detail rows, not the shape of the section.
    pub sites: Vec<WheelingSite>,
}

/// One site's wheeling figures for one period.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct WheelingSite {
    /// The period bucket, as the tool labels it (e.g. `"Q1"`, `"2025-Q2"`).
    pub period: String,
    /// The site / contract name.
    pub site: String,
    /// Contract capacity, kW.
    pub contract_kw: f64,
    /// Generated energy, kWh.
    pub generation_kwh: f64,
    /// Energy actually wheeled to buyers, kWh.
    pub wheeled_kwh: f64,
    /// Generated but un-wheeled energy, kWh.
    pub retained_kwh: f64,
    /// Wheeling rate, as a percentage in `[0, 100]`.
    pub wheeling_rate_pct: f64,
    /// Wheeling fee, before tax.
    pub fee_untaxed: f64,
}

// ===========================================================================
// ss_power_pipeline / ss_btm_projects — the development funnels
// ===========================================================================

/// One stage of a development funnel.
///
/// Shared by [`PipelineSection`] and [`BtmSection`] because all four funnels have the same row
/// shape; only the unit of `quantity` differs, and each section declares its own.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct FunnelStage {
    /// The stage name, keeping the upstream's leading ordinal (e.g. `"2 提案中"`).
    pub stage: String,
    /// Open deals sitting in this stage.
    pub cases: u64,
    /// The stage's quantity, in the section's declared unit. `0` is a real zero here — the funnels
    /// are padded to a fixed stage spine, so an empty stage is data, not a gap.
    pub quantity: f64,
    /// This stage's share of the funnel's total quantity, as a percentage in `[0, 100]`.
    pub share_pct: f64,
}

/// `ss_power_pipeline`: the sell and purchase funnels, as a current-stage snapshot.
///
/// The two funnels measure **different physical quantities** — sell is energy (kWh), purchase is
/// capacity (kW) — and the tool's own description warns they are never comparable. They therefore
/// carry explicit per-funnel units and are charted separately, never on a shared axis.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct PipelineSection {
    /// How this snapshot is framed (e.g. `"即時快照"`).
    pub label: String,
    /// The sell-side funnel's unit (`"kWh"`).
    pub sell_unit: String,
    /// The purchase-side funnel's unit (`"kW"`).
    pub purchase_unit: String,
    /// 潛在售電 — the sell funnel, earliest stage first. Empty ⇒ no sell panel.
    pub sell: Vec<FunnelStage>,
    /// 潛在購電 — the purchase funnel, earliest stage first. Empty ⇒ no purchase panel.
    pub purchase: Vec<FunnelStage>,
}

/// `ss_btm_projects`: the behind-the-meter funnels.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct BtmSection {
    /// How this snapshot is framed (e.g. `"即時快照"`).
    pub label: String,
    /// 潛在案件 — five stages, quantity in **MW**. Empty ⇒ no panel.
    pub potential: Vec<FunnelStage>,
    /// 星展50計畫 — three stages, quantity in **headcount** (the tool carries no capacity here).
    pub star_spark: Vec<FunnelStage>,
}

// ===========================================================================
// ss_ftm_projects — front-of-meter construction progress
// ===========================================================================

/// `ss_ftm_projects`: construction progress, flattened into three keyed vectors.
///
/// The tool returns `projects[].stages[]`; this protocol flattens it, joining `stages` and
/// `receivables` back to [`FtmProject::name`] by string. That is deliberate — see the module docs'
/// second design rule: a minimal-reasoning composer fills three flat tables far more reliably than
/// one nested tree, and the template re-groups them for display.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct FtmSection {
    /// How this snapshot is framed (e.g. `"即時快照"`).
    pub label: String,
    /// One row per project, largest capacity first. Empty ⇒ the whole section is hidden.
    pub projects: Vec<FtmProject>,
    /// Work items across every project, joined by [`FtmStage::project`]. May be empty.
    pub stages: Vec<FtmStage>,
    /// Billable amounts not yet collected, joined by [`FtmReceivable::project`]. May be empty.
    pub receivables: Vec<FtmReceivable>,
}

/// One FTM construction project.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct FtmProject {
    /// The project name (e.g. `"日喜_台中龍井100MW"`). Joins [`FtmStage::project`].
    pub name: String,
    /// The upstream project code (e.g. `"T22A33ES01"`).
    pub code: String,
    /// Installed capacity, MW.
    pub capacity_mw: f64,
    /// Whole-project completion, as a percentage in `[0, 100]`. Never recomputed from the stages —
    /// the upstream carries it as a master field, and the two genuinely disagree.
    pub completion_pct: f64,
    /// Planned start, `YYYY-MM-DD`. Empty when the upstream has none.
    pub planned_from: String,
    /// Planned finish, `YYYY-MM-DD`. Empty when the upstream has none.
    pub planned_to: String,
    /// Actual start, `YYYY-MM-DD`. Empty when construction has not started.
    pub actual_start: String,
    /// The project-level remark (e.g. `"併網/Power SCADA測試階段"`).
    pub note: String,
}

/// One work item within an FTM project.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct FtmStage {
    /// The owning project's name — must match an [`FtmProject::name`] exactly, or the template
    /// cannot group this row and will list it under its own heading.
    pub project: String,
    /// The work item, keeping the upstream's leading ordinal (e.g. `"8 上線"`).
    pub stage: String,
    /// Status as the upstream derives it from actual dates (e.g. `"進行中"`, `"未開始"`).
    pub status: String,
    /// Completion, as a percentage in `[0, 100]`.
    pub completion_pct: f64,
    /// Planned finish, `YYYY-MM-DD`. Empty when the upstream has none.
    pub planned_finish: String,
    /// The work item's remark. Empty when there is none.
    pub note: String,
}

/// One expected payment tied to an FTM work item.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct FtmReceivable {
    /// The owning project's name.
    pub project: String,
    /// The work item the payment is tied to.
    pub stage: String,
    /// What the payment is for (e.g. `"利通-潮州SI營收"`).
    pub note: String,
    /// Expected collection date, `YYYY-MM-DD`. Empty when not scheduled.
    pub expected_date: String,
    /// The amount, in `report.currency`.
    pub amount: f64,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> SsReportData {
        SsReportData {
            report: SsReportMeta {
                title: "營運綜合報告".into(),
                organization: "星星電力".into(),
                brand: "投資平台".into(),
                period_label: "2026 年第 2 季".into(),
                snapshot_label: "即時快照".into(),
                as_of: "2026-08-25".into(),
                locale: "zh-TW".into(),
                currency: "TWD".into(),
                data_note: "工具 1–3 帶期間參數，工具 4–6 為即時快照。".into(),
            },
            insight: ReportInsight {
                headline: "九站日照合計年減 4.9%。".into(),
                paragraphs: vec!["轉供率維持 98.93%。".into()],
            },
            solar: SolarSection {
                label: "2026 年第 2 季".into(),
                stations: vec![SolarStation {
                    name: "臺南".into(),
                    station_id: "467410".into(),
                    current_hours: 628.9,
                    prev_year_hours: 639.9,
                    prev_period_hours: 655.8,
                    yoy_pct: -1.7,
                    period_change_pct: -4.1,
                }],
            },
            storage: StorageSection {
                label: "2026 年 1–4 月累計".into(),
                bids: vec![StorageBid {
                    period: "2026-01".into(),
                    market_supply_mw: 1070.0,
                    market_cleared_mw: 816.79,
                    win_rate_pct: 87.8,
                    platform_bid_mw: 930.01,
                    platform_lost_mw: 113.22,
                    clearing_price: 0.0,
                }],
                cases: vec![StorageCase {
                    period: "2026-04".into(),
                    case: "星星花蓮三".into(),
                    platform: "富邦平台 / 匯旭".into(),
                    capacity_fee: 0.0,
                    energy_fee: 5_702_189.0,
                    performance_fee: 11_585_952.0,
                    service_income: 17_288_141.0,
                    verified: false,
                }],
            },
            wheeling: WheelingSection {
                label: "2025 年上半年累計".into(),
                total_rows: 1,
                sites: vec![WheelingSite {
                    period: "Q2".into(),
                    site: "星泓八案/旭康(水試所)".into(),
                    contract_kw: 21_999.91,
                    generation_kwh: 357_660.0,
                    wheeled_kwh: 99_806.0,
                    retained_kwh: 257_854.0,
                    wheeling_rate_pct: 27.91,
                    fee_untaxed: 26_828_567.0,
                }],
            },
            pipeline: PipelineSection {
                label: "即時快照".into(),
                sell_unit: "kWh".into(),
                purchase_unit: "kW".into(),
                sell: vec![FunnelStage {
                    stage: "1 商機開發".into(),
                    cases: 12,
                    quantity: 1_414_220_000.0,
                    share_pct: 97.84,
                }],
                purchase: vec![FunnelStage {
                    stage: "5 完成簽約".into(),
                    cases: 106,
                    quantity: 654_963.15,
                    share_pct: 98.49,
                }],
            },
            btm: BtmSection {
                label: "即時快照".into(),
                potential: vec![FunnelStage {
                    stage: "2 提案中".into(),
                    cases: 15,
                    quantity: 5_039.295,
                    share_pct: 99.62,
                }],
                star_spark: vec![],
            },
            ftm: FtmSection {
                label: "即時快照".into(),
                projects: vec![FtmProject {
                    name: "日喜_台中龍井100MW".into(),
                    code: "T22A33ES01".into(),
                    capacity_mw: 100.0,
                    completion_pct: 90.0,
                    planned_from: "2024-08-15".into(),
                    planned_to: "2026-11-30".into(),
                    actual_start: "2024-08-15".into(),
                    note: "併網/Power SCADA測試階段".into(),
                }],
                stages: vec![FtmStage {
                    project: "日喜_台中龍井100MW".into(),
                    stage: "8 上線".into(),
                    status: "進行中".into(),
                    completion_pct: 80.0,
                    planned_finish: "2026-12-31".into(),
                    note: "日喜_台中龍井SI營收".into(),
                }],
                receivables: vec![FtmReceivable {
                    project: "日喜_台中龍井100MW".into(),
                    stage: "8 上線".into(),
                    note: "日喜_台中龍井SI營收".into(),
                    expected_date: "2026-12-01".into(),
                    amount: 39_000_000.0,
                }],
            },
        }
    }

    #[test]
    fn serializes_to_the_template_wire_shape_and_round_trips() {
        let data = sample();
        let v = serde_json::to_value(&data).unwrap();

        // The eight top-level keys the template's reader expects.
        for key in [
            "report", "insight", "solar", "storage", "wheeling", "pipeline", "btm", "ftm",
        ] {
            assert!(v.get(key).is_some(), "missing top-level `{key}`");
        }

        // camelCase renames on the wire, at every depth the template reads.
        assert_eq!(v["report"]["periodLabel"], "2026 年第 2 季");
        assert_eq!(v["report"]["snapshotLabel"], "即時快照");
        assert_eq!(
            v["report"]["dataNote"],
            "工具 1–3 帶期間參數，工具 4–6 為即時快照。"
        );
        assert_eq!(v["solar"]["stations"][0]["stationId"], "467410");
        assert_eq!(v["solar"]["stations"][0]["prevYearHours"], 639.9);
        assert_eq!(v["storage"]["bids"][0]["platformBidMw"], 930.01);
        assert_eq!(v["storage"]["cases"][0]["serviceIncome"], 17_288_141.0);
        assert_eq!(v["wheeling"]["sites"][0]["wheeledKwh"], 99_806.0);
        assert_eq!(v["wheeling"]["totalRows"], 1);
        assert_eq!(v["pipeline"]["sellUnit"], "kWh");
        assert_eq!(v["pipeline"]["purchase"][0]["sharePct"], 98.49);
        assert_eq!(v["btm"]["potential"][0]["quantity"], 5_039.295);
        assert_eq!(v["ftm"]["projects"][0]["completionPct"], 90.0);
        assert_eq!(v["ftm"]["receivables"][0]["expectedDate"], "2026-12-01");

        let back: SsReportData = serde_json::from_value(v).unwrap();
        assert_eq!(back, data);
    }

    // The central rule: an unfetched tool is an EMPTY section, never a zero-filled one. The
    // starcharger schema had no way to express this, which is how a skipped tool became a chart
    // rendered flat at zero.
    #[test]
    fn an_absent_tool_is_an_empty_section_not_a_zero_row() {
        let data = sample();
        assert!(
            data.btm.star_spark.is_empty(),
            "the sample leaves 星展50 unfetched"
        );

        let v = serde_json::to_value(&data).unwrap();
        // It serializes as a real empty array — the template's hide-when-empty check — rather than
        // as one placeholder row of zeros.
        assert_eq!(v["btm"]["starSpark"], serde_json::json!([]));

        // Every section defaults to empty, so a report with nothing fetched is representable.
        let bare = SolarSection::default();
        assert!(bare.stations.is_empty() && bare.label.is_empty());
    }

    #[test]
    fn counts_stay_integers_and_measures_stay_numbers() {
        // The schema the model sees constrains these: case counts are integers, capacities and
        // money are numbers, so a "15 件" can never arrive as 15.0000001.
        let v = serde_json::to_value(sample()).unwrap();
        assert!(v["pipeline"]["purchase"][0]["cases"].is_u64());
        assert!(v["btm"]["potential"][0]["quantity"].is_f64());
        assert!(v["ftm"]["receivables"][0]["amount"].is_f64());
        assert!(v["storage"]["cases"][0]["verified"].is_boolean());
    }

    // The FTM flattening only works if the join key is exact; this pins that the sample's stage and
    // receivable both name a project that actually exists.
    #[test]
    fn ftm_child_rows_join_back_to_a_real_project_name() {
        let data = sample();
        let names: Vec<&str> = data.ftm.projects.iter().map(|p| p.name.as_str()).collect();
        for stage in &data.ftm.stages {
            assert!(names.contains(&stage.project.as_str()), "orphan stage row");
        }
        for receivable in &data.ftm.receivables {
            assert!(
                names.contains(&receivable.project.as_str()),
                "orphan receivable row"
            );
        }
    }
}
