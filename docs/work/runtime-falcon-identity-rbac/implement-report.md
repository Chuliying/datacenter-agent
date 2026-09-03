# Implement Report — runtime-falcon-identity-rbac

**Execution Mode**: team-feature  
**Canonical plan**: `plan/plan.md` (canonical-v2, 19 tasks)  
**Status**: complete — T01–T19 done; no blocker

## Source and contract decision

The Falcon guide at [commit c5d0408](https://github.com/HDRenewables/falcon-backend/blob/c5d0408b5e1847bf165e8c5f5859ef61da93e02d/docs/External_Delegated_API_Integration.md) is the S0 source of truth.

It documents `GET /api/auth/me/permissions` with a Bearer access token, a successful response containing `user_id`, object-form `roles`, and `permissions` with `can_read`/`can_write`, and generic `401` behavior for invalid, expired, or revoked tokens. Its row-by-row `external_auth.*` errors belong to `external-login`/`external-refresh`, not the permissions endpoint. The runtime therefore does not parse or forward an undocumented permissions `error_code` enum.

PRD v1.9.0, spec v1.4.0, QA plan, canonical plan, endpoint references, and consumer handoff all use that decision. The previously reported seven-row `auth.*` table is not part of this contract.

## Implementation summary

| Tasks | Delivered |
|---|---|
| T01–T04 | Source-verified Falcon permissions contract; config and boot validation; HMAC actor key; HTTP permissions client with Bearer transport, positive/negative TTL cache, SHA-256 cache keys, bounded LRU, malformed/timeout/401 classification. |
| T05–T08 | Identity middleware, distinct internal identity codes, prompt-route-only enforcement, global and per-actor rate limits, bounded actor buckets, and boot coupling. |
| T09–T10 | Opaque actor audit field/events and boot ∩ Falcon permission ∩ intent-required authorization with wildcard expansion, default-deny, and report partial-access semantics. |
| T11–T13 | Runtime-wired fixtures, scripted permissions provider, MCP call spy, process-wide tracing capture, local streaming OpenAI-compatible stub, identity carrier, and handler grant plumbing. |
| T14–T16 | Request-time memory authorization filtering, actor-isolated session scope, report degradation audit/notice, and grant-aware fetcher instructions. |
| T17–T19 | In-crate route/runtime/MCP/LLM integration coverage, public endpoint contract updates, consumer handoff, local Falcon stub, and pepper rotation runbook. |

## TDD evidence

### T02 config and boot validation

The five config cycles used runtime REDs after adding minimal Rust type skeletons. This keeps static type errors separate from the required observed behavior failures:

1. Missing `[identity]` failed at runtime because the shipped config did not declare the section.
2. The per-actor cycle failed because the global limiter prerequisite was not satisfied.
3. Missing `[authz]` failed at runtime.
4. Missing `[report.grants]` failed at runtime.
5. Boot validation failed with `authz validation not implemented` / `report grant validation not implemented` before the validators were implemented.

GREEN added the sections, explicit limiter policy, full intent/permission mapping validation, and an independent six-tool report grant. `identity.enabled` remains rejected by `deny_unknown_fields`; there is no feature flag that can silently revert the deployed binary to legacy behavior.

### Remaining behavior cycles

- Falcon client tests pin the documented path, Bearer header, role/permission 200 shape, generic 401, timeout/unavailable behavior, malformed 200, positive/negative cache replay, and LRU bound.
- Identity tests pin missing-header, unauthorized, unavailable, extension, OpenAI-envelope, and non-POST behavior.
- Authorization, audit, rate-limit, memory, handler, route, and wire-serialization tests pin the corresponding contract boundaries.
- `authorized_report_crosses_runtime_pipeline_with_terminal_degradation_notice` crosses the real handler, runtime prelude, in-memory MCP transport, local streaming chat-completions adapter, report grant, and terminal degradation notice.

## Verification

| Gate | Command | Result |
|---|---|---|
| bootstrap | `bash .agent/skills/_shared/bootstrap/check.sh` | PASS |
| completion gate | `bash .codex/skills/verification-before-completion/scripts/verify.sh` | PASS — type-check, lint, and tests |
| type-check | `cargo check` | PASS |
| lint | `cargo clippy --all-targets -- -D warnings` | PASS |
| format | `cargo fmt --check` | PASS |
| tests | `cargo test` | PASS — 271 lib + 4 eval + 1 deployment + 1 eval CLI + 2 route + 4 runtime-contract + 23 SQLite = 306 passed; 6 ignored; 0 failed |
| documentation | `bash .agent/skills/_shared/scripts/lint-docs.sh` | PASS |
| plan | `python3 .agent/skills/_shared/plan-sync/scripts/planctl.py check --plan-dir docs/work/runtime-falcon-identity-rbac/plan` | PASS — 19 tasks |
| plan status | `python3 .agent/skills/_shared/plan-sync/scripts/planctl.py status --plan-dir docs/work/runtime-falcon-identity-rbac/plan` | PASS — complete, no blockers |
| whitespace | `git diff --check` | PASS |

## Security review

- Secret scan: `bash .codex/skills/security/scripts/scan-secrets.sh` PASS; no hard-coded secrets, no token logging, `.env` remains ignored, and no staged secret was found. The scanner reported one suppressed candidate only.
- No dependency files changed, so dependency audit was not applicable.
- Manual auth review used base `04548857b1da5c2eb3612b0c15efa7560b7afa83` and covered the service bearer, Falcon permissions authority, default-deny authorization, strict intent checks, report partial grants, actor HMAC/opaque keys, identity error separation, limiter ordering, generic envelopes, and token-free audit/log paths.
- Destructive-operation guard was not applicable; this feature adds read/authorization paths only.

## Post-delivery review round (2026-09-02)

Four independent reviewers (Fable 5.1) covered the runtime identity/Falcon client, the RBAC and
HTTP request path, the runtime data plane, and falcon-client PR #20 plus the cross-repo contract.
No authorization bypass, fail-open, token leak, or cross-user data leak was found. Five findings
were fixed; the rest were filed as follow-ups.

| # | Finding | Fix | Evidence |
|---|---|---|---|
| P0 | `[insight.grants].fetcher` omitted `bill_member_analysis` while `[authz.intent_tools].member` required it. The non-report gate is strict (`omitted_tools.is_empty()`), so **every** `member` question was refused for **every** user regardless of their Falcon permissions — and it surfaced as "權限不足", indistinguishable from a real permission problem. Memory replay was denied for the same reason. Invisible to the suite because every `authz` test passed `["*"]` as the boot grant. | Added the sixth tool to the insight ceiling; added `AuthzConfig::validate_intent_reachability` so the two tables disagreeing fails boot (ERR-011, D-018); added an authz test that spends the *shipped* ceiling rather than a wildcard. | RED: `omitted: ["bill_member_analysis"]`. GREEN: `cargo test --lib -- server::authz:: config::tests` 36 passed. |
| P1 | falcon-client forwarded a header-less request when the caller was signed in but the `access_token` cookie had expired (`Max-Age=900`, and the refresh cookie is scoped to `/api/auth/refresh`). The runtime answered `identity.header_missing`, whose contract says *do not refresh*, so an ordinary session expiry demanded a full re-login. | The stream route now answers `401 AGENT_IDENTITY_REFRESHABLE` itself for that case, after the length gate; anonymous actors still forward unchanged for the staged rollout. C2/C5 handoff rows and the `header_missing` matrix row updated. | RED: `expected 200 to be 401`. GREEN: 6/6 in `route.test.ts`, 49/49 across chief-of-staff. |
| P2a | `UnauthorizedRefreshable`, `UnauthorizedPlatformCanary`, and `UpstreamConflict` had no test at the middleware boundary; AC-010 only covered terminal and unavailable. | Extended the AC-010 case table to all five `PermissionsFailure` variants. | Mutation check: mapping refreshable→terminal now fails (`left: "identity.token_terminal"`). |
| P2b | `PermissionCache::get`'s expiry branch had never executed — every test finishes inside a 10 s TTL — so the positive TTL (the upper bound on how long a revoked permission keeps working) was unpinned, as was the positive/negative TTL routing. | Direct expiry test over an injected `now`; extracted `cache_plan` as a method so the two same-typed TTLs can no longer be transposed at a call site, with a test per outcome. | Mutation check: disabling the expiry comparison now fails. |
| P2c | Nothing in the repo built a `report_pipeline: true` turn (`memory_turn()` hard-codes `false`), leaving `turn.report_pipeline ||` in the replay filter free to be deleted — which would replay a whole cross-topic report to a user who had since lost a topic. | Regression test for a report stored under a *topic* intent: dropped for a finance-only user, kept while the user still holds every topic, contrasted against the same turn without the tag. | Mutation check: replacing the condition with `intent == "report"` now fails. |
| P3 | The Falcon client used reqwest's default redirect policy. A same-origin 3xx **keeps** `Authorization`, so anything in front of Falcon answering 302 would carry a user's bearer to an unreviewed URL — contradicting the PRD's security statement. | `build_http_client` seam with `redirect::Policy::none()`; a 3xx now classifies as `Unavailable` (D-019). | RED: the client followed the redirect and returned `Ok(Permissions …)`. GREEN: 20/20 in `server::falcon::`. |

Deferred to follow-ups: `/v1 stream=true` refusals carry no machine-readable code (chunks have no
`x_refusal_code` field); transport errors collapse to one `Unavailable` with no way to tell DNS from
TLS from timeout; two internal error strings reach the client verbatim; the BFF's pre-stream SSE
error frame puts its code in `data` while runtime frames use `code`; falcon-client maps `418` to
`INVALID_REQUEST` despite the runtime now sending `auth.service_token_invalid`; a 33 KB report answer
exhausts the memory budget and drops the whole context rather than the oldest turn.

Documentation synced in the same round: spec → v1.6.0 (five `PermissionsFailure` variants, ERR-010
restored to the Errors table, ERR-011 added, the S16 test file that was never created, D-018/D-019);
`handoff-consumers.md` (C2, C5, the `header_missing` row, and the falcon-client status note);
`docs/reference/modules/server.md` (`identity.upstream_conflict`, redirect policy) and
`agent.md` (report no longer shares the insight grant; the two-table boot check).

## Scope notes

The local Falcon stub and scripted LLM/MCP fixtures are deterministic development/test topology; they do not claim a live Falcon or OpenRouter smoke test. Consumer-repository edits remain outside this repository and are documented in `handoff-consumers.md`.
