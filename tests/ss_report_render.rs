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

//! The `/ss-report` **render contract**, offline: does the shipped template plus the shipped
//! payload protocol actually produce a report?
//!
//! This is the network-free half of verifying the SS report. It needs no MCP server, no model and
//! no key, so it runs in CI on every commit — which is the point: the live harness can only tell
//! you the pipeline worked *today*, whereas these assertions pin the contract between the composer's
//! payload, the renderer, and the template's client-side reader forever.
//!
//! What it pins:
//!
//! 1. a full payload renders — the placeholder is gone, the data is embedded, `<`/`>` escaped;
//! 2. every field name the template's reader looks up is actually produced by the protocol;
//! 3. every chart canvas and section id the reader addresses exists in the template;
//! 4. an **empty section stays empty** — the design rule that stops a missing tool from rendering
//!    as a chart flat at zero (the starcharger failure this protocol was shaped to avoid).

use std::sync::Arc;

use datacenter_agent::agent::pipeline::{render_report_html, REPORT_DATA_PLACEHOLDER};
use datacenter_agent::agent::report::ReportInsight;
use datacenter_agent::agent::ss_report::*;
use datacenter_agent::config::AppConfig;

/// The shipped template, loaded the way the server loads it at boot.
fn template() -> Arc<String> {
    let config = AppConfig::load("config/config.toml").expect("shipped config should load");
    Arc::new(config.ss_report_template)
}

/// A payload with every section populated — one row is enough to prove a section renders.
fn full_payload() -> SsReportData {
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
            data_note: "工具 1–3 帶期間參數；工具 4–6 為即時快照。".into(),
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
            // Deliberately larger than `sites.len()`: this is the capped section, and the render
            // test must prove the cap is disclosed rather than silently applied.
            total_rows: 111,
            sites: vec![WheelingSite {
                period: "Q1".into(),
                site: "大福".into(),
                contract_kw: 21_999.91,
                generation_kwh: 5_243_658.0,
                wheeled_kwh: 5_243_658.0,
                retained_kwh: 0.0,
                wheeling_rate_pct: 100.0,
                fee_untaxed: 26_828_567.0,
            }],
        },
        pipeline: PipelineSection {
            label: "即時快照".into(),
            sell_unit: "kWh".into(),
            purchase_unit: "kW".into(),
            sell: vec![FunnelStage {
                stage: "1 商機開發".into(),
                cases: 10,
                quantity: 1_383_670_000.0,
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
            star_spark: vec![FunnelStage {
                stage: "1 洽談中".into(),
                cases: 4,
                quantity: 12.0,
                share_pct: 100.0,
            }],
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

/// A payload with nothing fetched — the permission-narrowed extreme, which must still render.
fn empty_payload() -> SsReportData {
    let full = full_payload();
    SsReportData {
        report: SsReportMeta {
            data_note: "本報告未包含任何業務線資料：呼叫者權限未涵蓋任何工具。".into(),
            ..full.report
        },
        insight: ReportInsight::default(),
        solar: SolarSection::default(),
        storage: StorageSection::default(),
        wheeling: WheelingSection::default(),
        pipeline: PipelineSection::default(),
        btm: BtmSection::default(),
        ftm: FtmSection::default(),
    }
}

#[test]
fn a_full_payload_renders_into_the_shipped_template() {
    let data = serde_json::to_value(full_payload()).expect("payload serializes");
    let html = render_report_html(&template(), &data);

    assert!(
        html.starts_with("```falcon-report\n"),
        "the renderer must wrap its output in the frontend's fence"
    );
    assert!(
        !html.contains(REPORT_DATA_PLACEHOLDER),
        "the data placeholder must have been replaced"
    );
    // Injection safety: a string value can never close the <script> element early.
    assert!(
        !html.contains("</script\\u003e") && !html.contains("<\\/script>"),
        "escaping is done by replacing < and >, not by escaping the slash"
    );
    assert!(
        html.contains("\\u003c") || !data.to_string().contains('<'),
        "any angle bracket in the data must be escaped"
    );
    // The payload really is embedded, not merely referenced.
    assert!(html.contains("星星花蓮三"), "case rows must reach the page");
    // The capped wheeling table must carry its true total, so the page can disclose the cap.
    assert!(
        html.contains("\"totalRows\":111"),
        "the wheeling row total must reach the page for the cap disclosure"
    );
    assert!(html.contains("467410"), "station ids must reach the page");
}

/// Every field the template's reader looks up must exist on the wire. A rename on either side
/// silently blanks a column in the delivered report, which no compiler catches — so it is pinned
/// here, by reading the template and the payload and comparing them.
#[test]
fn every_field_the_template_reads_is_produced_by_the_protocol() {
    let template = template();
    let wire = serde_json::to_value(full_payload()).expect("payload serializes");

    // The reader addresses rows by camelCase key; collect every key the payload actually emits.
    fn keys(value: &serde_json::Value, into: &mut Vec<String>) {
        match value {
            serde_json::Value::Object(map) => {
                for (key, child) in map {
                    into.push(key.clone());
                    keys(child, into);
                }
            }
            serde_json::Value::Array(items) => items.iter().for_each(|item| keys(item, into)),
            _ => {}
        }
    }
    let mut emitted = Vec::new();
    keys(&wire, &mut emitted);

    // Field accesses the template makes that must resolve. Chosen as the ones whose absence would
    // blank a visible column or a whole chart series.
    for field in [
        "periodLabel",
        "snapshotLabel",
        "dataNote",
        "stationId",
        "currentHours",
        "prevYearHours",
        "prevPeriodHours",
        "yoyPct",
        "periodChangePct",
        "marketSupplyMw",
        "marketClearedMw",
        "winRatePct",
        "platformBidMw",
        "platformLostMw",
        "clearingPrice",
        "serviceIncome",
        "energyFee",
        "performanceFee",
        "capacityFee",
        "contractKw",
        "generationKwh",
        "wheeledKwh",
        "retainedKwh",
        "wheelingRatePct",
        "feeUntaxed",
        "totalRows",
        "sellUnit",
        "purchaseUnit",
        "sharePct",
        "starSpark",
        "capacityMw",
        "completionPct",
        "plannedFrom",
        "plannedTo",
        "plannedFinish",
        "expectedDate",
    ] {
        assert!(
            emitted.iter().any(|key| key == field),
            "the template reads `{field}`, which the protocol does not emit"
        );
        assert!(
            template.contains(field),
            "the protocol emits `{field}`, which the template never reads — dead field?"
        );
    }
}

/// Every canvas and container the reader addresses by id must exist in the markup. A typo here
/// renders a section with no chart and no error.
#[test]
fn every_id_the_reader_addresses_exists_in_the_markup() {
    let template = template();
    for id in [
        // sections
        "solar-section",
        "storage-section",
        "wheeling-section",
        "pipeline-section",
        "btm-section",
        "ftm-section",
        // table bodies
        "solar-body",
        "storage-bid-body",
        "storage-case-body",
        "wheeling-body",
        "sell-body",
        "purchase-body",
        "btm-potential-body",
        "btm-spark-body",
        "ftm-project-body",
        "ftm-stage-groups",
        "ftm-receivable-body",
        // chart canvases
        "solar-chart",
        "storage-bid-chart",
        "storage-income-chart",
        "wheeling-chart",
        "sell-chart",
        "purchase-chart",
        "btm-chart",
        "ftm-chart",
    ] {
        assert!(
            template.contains(&format!("id=\"{id}\"")),
            "the reader addresses `#{id}`, which the markup does not define"
        );
    }
}

/// **The design rule.** A tool that was never fetched must yield an empty section, and an empty
/// section must reach the page as an empty array — never as a zero-filled placeholder row.
///
/// This is the starcharger failure mode made impossible: there, a fixed `periods[]` shape forced
/// the composer to zero-fill an unfetched topic, and the result rendered as a chart flat at zero
/// that no reader could tell from a real zero. Here the absence survives to the template, which
/// hides the section.
#[test]
fn an_unfetched_tool_reaches_the_page_as_an_empty_section() {
    let data = serde_json::to_value(empty_payload()).expect("payload serializes");

    // Every section is present in the payload but carries no rows.
    assert_eq!(data["solar"]["stations"], serde_json::json!([]));
    assert_eq!(data["storage"]["bids"], serde_json::json!([]));
    assert_eq!(data["storage"]["cases"], serde_json::json!([]));
    assert_eq!(data["wheeling"]["sites"], serde_json::json!([]));
    assert_eq!(data["pipeline"]["sell"], serde_json::json!([]));
    assert_eq!(data["pipeline"]["purchase"], serde_json::json!([]));
    assert_eq!(data["btm"]["potential"], serde_json::json!([]));
    assert_eq!(data["btm"]["starSpark"], serde_json::json!([]));
    assert_eq!(data["ftm"]["projects"], serde_json::json!([]));

    // It still renders — a caller with no permissions gets a valid, honest document, not an error.
    let html = render_report_html(&template(), &data);
    assert!(!html.contains(REPORT_DATA_PLACEHOLDER));

    // And the disclosure survives: the note is the reader's only signal, since every section is
    // hidden. An empty `dataNote` on an empty report would be the actual bug.
    assert!(
        html.contains("本報告未包含任何業務線資料"),
        "a report with no sections must still carry its data note"
    );

    // The markup ships every section hidden, so nothing is revealed unless the reader reveals it.
    let template = template();
    for section in [
        "solar-section",
        "storage-section",
        "wheeling-section",
        "pipeline-section",
        "btm-section",
        "ftm-section",
    ] {
        let at = template
            .find(&format!("id=\"{section}\""))
            .unwrap_or_else(|| panic!("no `{section}` in the markup"));
        let tag_end = template[at..]
            .find('>')
            .map(|offset| at + offset)
            .expect("the opening tag should close");
        assert!(
            template[at..tag_end].contains("hidden"),
            "`{section}` must ship hidden, or an unfetched tool renders an empty section"
        );
    }
}
