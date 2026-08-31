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

## Scope notes

The local Falcon stub and scripted LLM/MCP fixtures are deterministic development/test topology; they do not claim a live Falcon or OpenRouter smoke test. Consumer-repository edits remain outside this repository and are documented in `handoff-consumers.md`.
