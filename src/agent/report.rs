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
//! [`ReportData`] via its validated `emit_report`
//! ([`emit_report_tool`](crate::agent::tools::emit_report_tool)) sink — a malformed shape **or a
//! shape the template cannot render** ([`ReportData::validate`]) is `Rejected` and fed back until
//! valid, never crashing — and the pure-logic `renderer`
//! ([`render_report_html`](crate::agent::pipeline::render_report_html)) escapes it and drops it
//! into the template's single `__REPORT_DATA_JSON__` placeholder.
//!
//! One shared `serde` + `schemars` family drives both ends: `emit_report`'s advertised argument
//! schema and its on-receipt validation both derive from these types, and the serialized value
//! *is* the `report.data` artifact ([`ArtifactKey::report_data`](crate::agent::payload::ArtifactKey::report_data))
//! the renderer reads back.
//!
//! # Two validation layers
//!
//! Deserialization only proves the *shape*. The template's client script additionally relies on
//! **cross-field invariants** that no JSON schema expresses — `summary.latestCompletedPeriod` must
//! name a non-partial `periods` entry, the arrays must be non-empty, months must be `YYYY-MM` and
//! oldest first. A payload that passes the schema but breaks one of these throws inside the
//! browser *before* any chart is drawn, so the report opens with its header filled in and four
//! blank canvases. [`ReportData::validate`] enforces those invariants at the sink, and its reason
//! strings are written for the model: they name the field and say what to change.
//!
//! The wire shape matches the template's client-side reader verbatim (camelCase fields, the
//! `report` / `summary` / `insight` / `periods` / `stationRanking` top level). See
//! `config/report_template/report.html`.
//!
//! # References
//!
//! - Sub-agent plan §10 — the endpoint pipelines (`/report`: fetch → analyse → compose → render)
//! - `config/prompt_guide/report_composer_system.md` — the authored composing instruction

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

impl ReportData {
    /// Checks the cross-field invariants the HTML template's client script depends on.
    ///
    /// Some of these make the template throw before drawing its charts (empty arrays, an anchor
    /// month that matches nothing, an unparseable locale); the rest make it render silently wrong
    /// (months out of order, a partial month mid-window, an anchor on a stale month). See the
    /// module docs. The `Err` string is fed back to the model verbatim as the rejection reason, so
    /// each one names the offending field and states the fix.
    ///
    /// Invariants, in check order:
    ///
    /// 1. `report.locale` is a well-formed language tag (`Intl.NumberFormat` throws otherwise).
    /// 2. `periods` is non-empty.
    /// 3. Every `periods[].period` is `YYYY-MM` (zero-padded month `01`–`12`).
    /// 4. `periods` are oldest first with no duplicate months.
    /// 5. At most one period is `partial`, and only the last one may be.
    /// 6. `summary.latestCompletedPeriod` equals the `period` of the **most recent** non-partial
    ///    entry.
    /// 7. `stationRanking` is non-empty, with `rank` running `1, 2, …` in order.
    pub fn validate(&self) -> Result<(), String> {
        if !is_language_tag(&self.report.locale) {
            return Err(format!(
                "report.locale `{}` is not a BCP-47 language tag; use hyphen-separated subtags \
                 such as `zh-TW`",
                self.report.locale
            ));
        }
        if self.periods.is_empty() {
            return Err(
                "periods must contain at least one month; if the fetched data has no \
                        monthly figures, do not emit a report — reply that the data is missing"
                    .into(),
            );
        }
        for p in &self.periods {
            if !is_year_month(&p.period) {
                return Err(format!(
                    "periods[].period `{}` must be `YYYY-MM` with a zero-padded month \
                     (e.g. `2026-05`)",
                    p.period
                ));
            }
        }
        for pair in self.periods.windows(2) {
            if pair[1].period <= pair[0].period {
                return Err(format!(
                    "periods must be oldest first without duplicates, but `{}` follows `{}`",
                    pair[1].period, pair[0].period
                ));
            }
        }
        let last = self.periods.len() - 1;
        if let Some(i) = self.periods.iter().position(|p| p.partial) {
            if i != last {
                return Err(format!(
                    "only the last month may be partial, but periods[{i}] (`{}`) is partial",
                    self.periods[i].period
                ));
            }
        }
        let anchor = &self.summary.latest_completed_period;
        // The partial month, if any, is the last one (checked above), so the most recent complete
        // month is the last entry that is not partial.
        let expected_anchor = self.periods.iter().rev().find(|p| !p.partial);
        match self.periods.iter().find(|p| &p.period == anchor) {
            None => {
                let months: Vec<&str> = self
                    .periods
                    .iter()
                    .filter(|p| !p.partial)
                    .map(|p| p.period.as_str())
                    .collect();
                return Err(format!(
                    "summary.latestCompletedPeriod `{anchor}` matches no periods[].period; copy \
                     the most recent complete month verbatim (complete months: {months:?})"
                ));
            }
            Some(p) if p.partial => {
                let hint = expected_anchor
                    .map(|e| format!(" (`{}`)", e.period))
                    .unwrap_or_default();
                return Err(format!(
                    "summary.latestCompletedPeriod `{anchor}` is the partial month; it must be \
                     the most recent month whose partial is false{hint}"
                ));
            }
            Some(_) => {
                if let Some(expected) = expected_anchor {
                    if &expected.period != anchor {
                        return Err(format!(
                            "summary.latestCompletedPeriod `{anchor}` is not the most recent \
                             complete month; use `{}`",
                            expected.period
                        ));
                    }
                }
            }
        }
        if self.station_ranking.is_empty() {
            return Err(
                "stationRanking must contain at least one station; if the fetched data \
                        has no station figures, do not emit a report — reply that the data is \
                        missing"
                    .into(),
            );
        }
        for (i, s) in self.station_ranking.iter().enumerate() {
            let expected = i as u32 + 1;
            if s.rank != expected {
                return Err(format!(
                    "stationRanking[{i}].rank is {} but ranks must run 1, 2, … in order \
                     (expected {expected})",
                    s.rank
                ));
            }
        }
        Ok(())
    }
}

/// A BCP-47-shaped language tag: hyphen-separated ASCII alphanumeric subtags, the first alphabetic
/// and 2–8 characters long, the rest 1–8 characters. This accepts a superset of what
/// `Intl.NumberFormat` accepts (it does not check subtag grammar or the registry), but it rejects
/// the realistic failure — an underscore or garbage — and the template's guard catches the rest.
fn is_language_tag(s: &str) -> bool {
    let mut subtags = s.split('-');
    let Some(primary) = subtags.next() else {
        return false;
    };
    if !(2..=8).contains(&primary.len()) || !primary.bytes().all(|b| b.is_ascii_alphabetic()) {
        return false;
    }
    subtags.all(|t| (1..=8).contains(&t.len()) && t.bytes().all(|b| b.is_ascii_alphanumeric()))
}

/// `YYYY-MM`: four digits, a dash, and a zero-padded month in `01..=12`.
fn is_year_month(s: &str) -> bool {
    let b = s.as_bytes();
    if b.len() != 7 || b[4] != b'-' {
        return false;
    }
    if !b[..4].iter().chain(&b[5..]).all(u8::is_ascii_digit) {
        return false;
    }
    matches!(
        &s[5..],
        "01" | "02" | "03" | "04" | "05" | "06" | "07" | "08" | "09" | "10" | "11" | "12"
    )
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
    fn validate_accepts_the_template_wire_sample() {
        assert_eq!(sample().validate(), Ok(()));
    }

    #[test]
    fn validate_names_the_field_for_each_unrenderable_invariant() {
        // Anchor month not present (format drift) — the single most likely production trigger.
        let mut d = sample();
        d.summary.latest_completed_period = "2026-5".into();
        let e = d.validate().unwrap_err();
        assert!(
            e.contains("latestCompletedPeriod") && e.contains("2026-05"),
            "{e}"
        );

        // Anchor month is the partial one.
        let mut d = sample();
        d.summary.latest_completed_period = "2026-06".into();
        assert!(d.validate().unwrap_err().contains("partial"));

        // Empty arrays.
        let mut d = sample();
        d.periods.clear();
        assert!(d
            .validate()
            .unwrap_err()
            .starts_with("periods must contain"));
        let mut d = sample();
        d.station_ranking.clear();
        assert!(d
            .validate()
            .unwrap_err()
            .starts_with("stationRanking must contain"));

        // Month format (checked on every entry, not only the first) and ordering.
        let mut d = sample();
        d.periods[0].period = "2026/05".into();
        assert!(d.validate().unwrap_err().contains("YYYY-MM"));
        let mut d = sample();
        d.periods[1].period = "2026-6".into();
        assert!(d.validate().unwrap_err().contains("YYYY-MM"));

        // Anchor on an older complete month than the most recent one.
        let mut d = sample();
        d.periods[1].partial = false;
        d.summary.latest_completed_period = "2026-05".into();
        let e = d.validate().unwrap_err();
        assert!(
            e.contains("not the most recent") && e.contains("2026-06"),
            "{e}"
        );

        // Locale the browser's `Intl.NumberFormat` would reject.
        let mut d = sample();
        d.report.locale = "zh_TW".into();
        assert!(d.validate().unwrap_err().contains("report.locale"));
        let mut d = sample();
        d.periods.swap(0, 1);
        assert!(d.validate().unwrap_err().contains("oldest first"));
        let mut d = sample();
        d.periods[1].period = "2026-05".into();
        d.periods[1].partial = false;
        assert!(d.validate().unwrap_err().contains("oldest first"));

        // A partial month that is not the trailing one.
        let mut d = sample();
        d.periods[0].partial = true;
        d.periods[1].partial = false;
        d.summary.latest_completed_period = "2026-06".into();
        assert!(d
            .validate()
            .unwrap_err()
            .contains("only the last month may be partial"));

        // Ranks must run 1, 2, … in order.
        let mut d = sample();
        d.station_ranking[0].rank = 3;
        assert!(d.validate().unwrap_err().contains("rank"));
    }

    #[test]
    fn validate_allows_a_window_with_no_partial_month() {
        let mut d = sample();
        d.periods[1].partial = false;
        d.summary.latest_completed_period = "2026-06".into();
        assert_eq!(d.validate(), Ok(()));
    }

    #[test]
    fn is_language_tag_accepts_hyphenated_subtags_only() {
        for ok in ["zh-TW", "en", "zh-Hant-TW", "en-US-u-nu-latn", "Taiwan"] {
            assert!(is_language_tag(ok), "{ok}");
        }
        for bad in [
            "zh_TW",
            "",
            "z",
            "zh-",
            "zh--TW",
            "zh-TW!",
            "123-TW",
            "toolongprimary",
        ] {
            assert!(!is_language_tag(bad), "{bad}");
        }
    }

    #[test]
    fn is_year_month_accepts_only_zero_padded_months() {
        for ok in ["2026-01", "2026-12", "1999-06"] {
            assert!(is_year_month(ok), "{ok}");
        }
        for bad in [
            "2026-5",
            "2026-13",
            "2026-00",
            "2026/05",
            "2026-05-01",
            "2026年5月",
            "",
        ] {
            assert!(!is_year_month(bad), "{bad}");
        }
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
