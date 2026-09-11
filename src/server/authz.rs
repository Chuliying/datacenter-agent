//! Per-request authorization grant narrowing.

use std::collections::HashSet;

use crate::agent::wiring::expand_grant;
use crate::config::AuthzConfig;

/// The result of intersecting boot, permission, and intent grants.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AuthorizationDecision {
    /// Whether the request may enter the selected pipeline.
    pub allowed: bool,
    /// Data tools safe to expose to the selected fetcher.
    pub effective_data_grant: Vec<String>,
    /// Tools required by the selected intent or report pipeline.
    pub required_tools: Vec<String>,
    /// Required tools omitted by the effective grant.
    pub omitted_tools: Vec<String>,
    /// User-facing topic labels omitted from a degraded report.
    pub omitted_topics: Vec<String>,
    /// Code-backed chart output grant, deliberately not narrowed.
    pub charter_grant: Vec<String>,
    /// Code-backed report output grant, deliberately not narrowed.
    pub composer_grant: Vec<String>,
    /// Whether the report-pipeline predicate selected report semantics.
    pub report_pipeline: bool,
}

/// Whether a caller may see the **data-aware** greeting.
///
/// Greetings are pre-generated at boot from everything the greeting fetcher may call
/// (`[insight.grants].fetcher`), so a single greeting can quote revenue, member and station
/// figures. A caller therefore only qualifies when their Falcon permissions unlock *every* tool
/// in that grant; anyone narrower gets the neutral greeting instead. Conservative on purpose: a
/// finance-only role loses the data greeting rather than risk seeing a member figure it may not
/// query. An empty grant never qualifies.
pub fn greeting_scope_allows(
    config: &AuthzConfig,
    permission_codes: &HashSet<String>,
    greeting_fetcher_grant: &[String],
    advertised: &[String],
) -> bool {
    let required = expand_grant(greeting_fetcher_grant, advertised);
    if required.is_empty() {
        return false;
    }
    let granted = permission_codes
        .iter()
        .filter_map(|code| config.permission_tools.get(code))
        .flat_map(|grant| expand_grant(grant, advertised))
        .collect::<HashSet<_>>();
    required.iter().all(|tool| granted.contains(tool))
}

/// Intersect boot data grants, Falcon permission grants, and required intent tools.
#[allow(clippy::too_many_arguments)]
pub fn authorize_pipeline(
    config: &AuthzConfig,
    boot_data_grant: &[String],
    permission_codes: &HashSet<String>,
    intent: &str,
    wants_report_pipeline: bool,
    advertised: &[String],
    charter_grant: &[String],
    composer_grant: &[String],
) -> AuthorizationDecision {
    let boot_grant = expand_grant(boot_data_grant, advertised);
    let required_tools = if wants_report_pipeline {
        config
            .intent_tools
            .get("report")
            .map(|grant| expand_grant(grant, advertised))
            .unwrap_or_default()
    } else {
        config
            .intent_tools
            .get(intent)
            .map(|grant| expand_grant(grant, advertised))
            .unwrap_or_default()
    };

    let permission_grant = permission_codes
        .iter()
        .filter_map(|code| config.permission_tools.get(code))
        .flat_map(|grant| grant.iter().cloned())
        .collect::<Vec<_>>();
    let permission_set: HashSet<String> = expand_grant(&permission_grant, advertised)
        .into_iter()
        .collect();
    let required_set: HashSet<String> = required_tools.iter().cloned().collect();
    let effective_data_grant = boot_grant
        .into_iter()
        .filter(|tool| permission_set.contains(tool) && required_set.contains(tool))
        .collect::<Vec<_>>();
    let effective_set: HashSet<String> = effective_data_grant.iter().cloned().collect();
    let omitted_tools = required_tools
        .iter()
        .filter(|tool| !effective_set.contains(*tool))
        .cloned()
        .collect::<Vec<_>>();
    let omitted_topics = if wants_report_pipeline {
        ["revenue", "charging", "member"]
            .into_iter()
            .filter(|topic| {
                config
                    .intent_tools
                    .get(*topic)
                    .map(|tools| {
                        expand_grant(tools, advertised)
                            .iter()
                            .any(|tool| !effective_data_grant.iter().any(|grant| grant == tool))
                    })
                    .unwrap_or(false)
            })
            .map(str::to_string)
            .collect()
    } else if !omitted_tools.is_empty() {
        vec![intent.to_string()]
    } else {
        Vec::new()
    };
    let allowed = if required_tools.is_empty() {
        false
    } else if wants_report_pipeline {
        !effective_data_grant.is_empty()
    } else {
        omitted_tools.is_empty()
    };

    AuthorizationDecision {
        allowed,
        effective_data_grant,
        required_tools,
        omitted_tools,
        omitted_topics,
        charter_grant: charter_grant.to_vec(),
        composer_grant: composer_grant.to_vec(),
        report_pipeline: wants_report_pipeline,
    }
}

/// `/ss-chat/stream`'s authorization: permission-set membership instead of intent requirements.
///
/// Holding **any** code in `[authz].ss_chat_permissions` unlocks the full SS grant (intersected
/// with the advertised MCP set, mirroring [`authorize_pipeline`]'s never-widen rule); holding
/// none — including holding only the `startrade-power` *parent* page, if the config lists only
/// sub-pages — refuses. `omitted_topics` stays empty: SS has no degraded mode today, the answer
/// is all-or-nothing.
pub fn authorize_ss_chat(
    config: &AuthzConfig,
    ss_grant: &[String],
    permission_codes: &std::collections::HashSet<String>,
    advertised: &[String],
    charter_grant: &[String],
) -> AuthorizationDecision {
    let allowed = config
        .ss_chat_permissions
        .iter()
        .any(|code| permission_codes.contains(code));
    let effective_data_grant = if allowed {
        ss_grant
            .iter()
            .filter(|tool| advertised.iter().any(|name| name == *tool))
            .cloned()
            .collect()
    } else {
        Vec::new()
    };
    AuthorizationDecision {
        allowed,
        required_tools: ss_grant.to_vec(),
        omitted_tools: Vec::new(),
        omitted_topics: Vec::new(),
        effective_data_grant,
        charter_grant: charter_grant.to_vec(),
        composer_grant: Vec::new(),
        report_pipeline: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FINANCE: &str = "hdrenewables/elecsvc/starcharger/finance";
    const OPPERF: &str = "hdrenewables/elecsvc/starcharger/opperf";
    const BIZDEV: &str = "hdrenewables/elecsvc/starcharger/bizdev";

    fn config() -> AuthzConfig {
        crate::config::AppConfig::load("config/config.toml")
            .expect("shipped config should load")
            .authz
            .expect("shipped config declares authz")
    }

    /// The real `[insight.grants].fetcher` ceiling, not `["*"]`.
    ///
    /// Every other test in this module passes a wildcard boot grant, which makes the boot
    /// leg of the three-way intersection a no-op. A ceiling that fails to cover an intent's
    /// required tools is therefore invisible to them; this helper exists so at least one
    /// test spends the shipped ceiling.
    fn shipped_insight_fetcher_grant() -> Vec<String> {
        crate::config::AppConfig::load("config/config.toml")
            .expect("shipped config should load")
            .insight_grants
            .fetcher
    }

    fn advertised() -> Vec<String> {
        [
            "bill_revenue",
            "station_revenue_ranking",
            "bill_charge",
            "business_metrics",
            "member_analysis",
            "bill_member_analysis",
        ]
        .into_iter()
        .map(str::to_string)
        .collect()
    }

    fn permissions(code: &str) -> HashSet<String> {
        [code.to_string()].into_iter().collect()
    }

    const ENGPROJ: &str = "hdrenewables/elecsvc/starcharger/engproj";

    /// The data-aware greeting requires every greeting-fetcher tool to be unlocked.
    #[test]
    fn greeting_scope_requires_the_whole_greeting_grant() {
        let config = config();
        let grant = shipped_insight_fetcher_grant();
        let all = [FINANCE, OPPERF, BIZDEV]
            .into_iter()
            .map(str::to_string)
            .collect::<HashSet<_>>();
        assert!(greeting_scope_allows(&config, &all, &grant, &advertised()));
    }

    /// A page-only role (engproj maps to no tools) and a partial role both get the neutral greeting.
    #[test]
    fn greeting_scope_is_neutral_for_page_only_and_partial_roles() {
        let config = config();
        let grant = shipped_insight_fetcher_grant();
        assert!(!greeting_scope_allows(
            &config,
            &permissions(ENGPROJ),
            &grant,
            &advertised()
        ));
        assert!(!greeting_scope_allows(
            &config,
            &permissions(FINANCE),
            &grant,
            &advertised()
        ));
        assert!(!greeting_scope_allows(
            &config,
            &HashSet::new(),
            &grant,
            &advertised()
        ));
    }

    /// An empty greeting grant can never qualify, whatever the caller holds.
    #[test]
    fn greeting_scope_denies_on_empty_grant() {
        let config = config();
        let all = [FINANCE, OPPERF, BIZDEV]
            .into_iter()
            .map(str::to_string)
            .collect::<HashSet<_>>();
        assert!(!greeting_scope_allows(&config, &all, &[], &advertised()));
    }

    /// The SS gate: any configured startrade-power sub-page unlocks the full SS grant.
    #[test]
    fn ss_chat_any_configured_permission_unlocks_the_grant() {
        let ss_grant: Vec<String> = ["ss_sunshine_hours", "ss_energy_storage"]
            .into_iter()
            .map(str::to_string)
            .collect();
        let advertised = ss_grant.clone();

        let decision = authorize_ss_chat(
            &config(),
            &ss_grant,
            &permissions("hdrenewables/elecsvc/startrade-power/finance"),
            &advertised,
            &["emit_chart".into()],
        );
        assert!(decision.allowed);
        assert_eq!(decision.effective_data_grant, ss_grant);
        assert!(!decision.report_pipeline);
    }

    /// The SS gate refuses without a configured permission — including the startrade-power
    /// *parent* page (AC-008's rule carries over) and a starcharger-only permission set.
    #[test]
    fn ss_chat_refuses_parent_only_and_other_unit_permissions() {
        let ss_grant: Vec<String> = vec!["ss_sunshine_hours".to_string()];
        for code in [
            "hdrenewables/elecsvc/startrade-power",
            "hdrenewables/elecsvc/starcharger/finance",
        ] {
            let decision = authorize_ss_chat(
                &config(),
                &ss_grant,
                &permissions(code),
                &ss_grant.clone(),
                &["emit_chart".into()],
            );
            assert!(!decision.allowed, "`{code}` must not unlock the SS grant");
            assert!(decision.effective_data_grant.is_empty());
        }
    }

    /// AC-008: holding only the business-unit parent page grants nothing.
    ///
    /// Falcon issues page-level permissions, so a user can hold the starcharger overview page
    /// without any of its four sub-pages. The parent is not a wildcard over its children, and
    /// treating it as one would silently hand every topic to someone Falcon only let into the
    /// landing page. The expected outcome is an empty grant and a refusal, not a partial answer.
    #[test]
    /// S-RUNTIME-SEC-02 AC-008
    fn ac008_business_unit_parent_permission_grants_no_tools() {
        for intent in ["revenue", "charging", "member", "report"] {
            let decision = authorize_pipeline(
                &config(),
                &["*".into()],
                &permissions("hdrenewables/elecsvc/starcharger"),
                intent,
                intent == "report",
                &advertised(),
                &["emit_chart".into()],
                &["emit_report".into()],
            );

            assert!(
                !decision.allowed,
                "parent-only permission must not authorize `{intent}`"
            );
            assert!(
                decision.effective_data_grant.is_empty(),
                "parent-only permission must yield no data tools for `{intent}`"
            );
        }
    }

    /// AC-022 companion: a permission for a different business unit is equally empty here,
    /// because every MCP endpoint this runtime knows about serves starcharger.
    #[test]
    fn ac008_another_business_unit_permission_grants_no_tools() {
        let decision = authorize_pipeline(
            &config(),
            &["*".into()],
            &permissions("hdrenewables/elecsvc/startrade-power/finance"),
            "revenue",
            false,
            &advertised(),
            &["emit_chart".into()],
            &["emit_report".into()],
        );

        assert!(!decision.allowed);
        assert!(decision.effective_data_grant.is_empty());
    }

    /// AC-022: the gate keys off the tools the intent *requires*, not merely whether the
    /// narrowed set is non-empty — a finance-only user still holds finance tools.
    #[test]
    /// S-RUNTIME-SEC-02 AC-022
    fn finance_permission_cannot_authorize_a_member_intent() {
        let decision = authorize_pipeline(
            &config(),
            &["*".into()],
            &permissions(FINANCE),
            "member",
            false,
            &advertised(),
            &["emit_chart".into()],
            &["emit_report".into()],
        );

        assert!(!decision.allowed);
        assert!(decision.effective_data_grant.is_empty());
    }

    #[test]
    fn strict_intent_requires_all_required_tools_not_just_one() {
        let decision = authorize_pipeline(
            &config(),
            &["bill_revenue".into()],
            &permissions(FINANCE),
            "revenue",
            false,
            &advertised(),
            &["emit_chart".into()],
            &["emit_report".into()],
        );

        assert_eq!(
            decision.effective_data_grant,
            vec!["bill_revenue".to_string()]
        );
        assert!(
            !decision.allowed,
            "partial strict intent access must default-deny"
        );
        assert_eq!(
            decision.omitted_tools,
            vec!["station_revenue_ranking".to_string()]
        );
    }

    #[test]
    /// S-RUNTIME-SEC-02 AC-007
    fn wildcard_is_expanded_before_intersection_and_output_grants_are_preserved() {
        let decision = authorize_pipeline(
            &config(),
            &["*".into()],
            &permissions(BIZDEV),
            "member",
            false,
            &advertised(),
            &["emit_chart".into()],
            &["emit_report".into()],
        );

        assert_eq!(
            decision.effective_data_grant,
            vec![
                "member_analysis".to_string(),
                "bill_member_analysis".to_string()
            ]
        );
        assert!(decision.allowed);
        assert_eq!(decision.charter_grant, ["emit_chart"]);
        assert_eq!(decision.composer_grant, ["emit_report"]);
    }

    #[test]
    fn report_predicate_allows_partial_access_for_a_mixed_prompt() {
        let decision = authorize_pipeline(
            &config(),
            &["*".into()],
            &permissions(FINANCE),
            "revenue",
            true,
            &advertised(),
            &["emit_chart".into()],
            &["emit_report".into()],
        );

        assert!(decision.report_pipeline);
        assert!(
            decision.allowed,
            "report uses the non-empty partial-grant exception"
        );
        assert_eq!(
            decision.effective_data_grant,
            vec![
                "bill_revenue".to_string(),
                "station_revenue_ranking".to_string()
            ]
        );
        assert_eq!(decision.omitted_tools.len(), 4);
    }

    #[test]
    fn unknown_or_empty_required_intent_is_default_deny() {
        let decision = authorize_pipeline(
            &config(),
            &["*".into()],
            &permissions(OPPERF),
            "unknown",
            false,
            &advertised(),
            &[],
            &[],
        );

        assert!(!decision.allowed);
        assert!(decision.effective_data_grant.is_empty());
    }
    #[test]
    /// S-RUNTIME-SEC-02 AC-007 / FR-004, under the shipped boot ceiling.
    ///
    /// PRD 表列 `member` -> {member_analysis, bill_member_analysis}，權限碼 bizdev 同樣解到
    /// 兩個。非 report 路徑是嚴格規則（`omitted_tools` 必須為空），所以只要
    /// `[insight.grants].fetcher` 少一個，持有正確權限的 bizdev 使用者就會被永久拒絕。
    fn member_intent_is_allowed_for_bizdev_under_the_shipped_insight_ceiling() {
        let decision = authorize_pipeline(
            &config(),
            &shipped_insight_fetcher_grant(),
            &permissions(BIZDEV),
            "member",
            false,
            &advertised(),
            &["emit_chart".into()],
            &["emit_report".into()],
        );

        assert!(
            decision.omitted_tools.is_empty(),
            "the shipped insight ceiling must cover every tool the `member` intent requires, \
             otherwise a correctly-permissioned bizdev user is denied forever; omitted: {:?}",
            decision.omitted_tools
        );
        assert!(decision.allowed);
        assert_eq!(
            decision.effective_data_grant,
            vec![
                "member_analysis".to_string(),
                "bill_member_analysis".to_string()
            ]
        );
    }
}
