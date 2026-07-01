# Agent Runtime Rust Port QA Report

**Story ID**: S-RUNTIME-01  
**Date**: 2026-06-26  
**Scope**: local `datacenter-agent` runtime implementation through PR-10 replay-smoke coverage.

## Summary

Local static, unit, integration, pipeline eval, replay-smoke, and `falcon-client` forwarding gates pass without provider credentials. Live/provider and staging gates remain release-only and are not completed by this report.

## Executed Gates

| Gate | Command | Result | Evidence |
|------|---------|--------|----------|
| L1 format | `cargo fmt -- --check` | PASS | exit 0 |
| L1 lint | `cargo clippy --all-targets --all-features -- -D warnings` | PASS | exit 0 |
| L2/L4 tests | `cargo test` | PASS | 61 lib tests, 4 eval-bin tests, 3 runtime-contract tests; ignored live test remains opt-in |
| Eval pipeline | `cargo run --bin eval -- --pipeline-only` | PASS | `passed=3, failed=0` |
| Eval replay smoke | `cargo run --bin eval -- --response --replay config/runtime/evals/replay-smoke.json` | PASS | `passed=2, failed=0`, provenance printed |
| Falcon endpoint forwarding | `npm test -- --run src/lib/chief-of-staff/agent-client.test.ts src/lib/chief-of-staff/agent-runtime/run-agent-turn.test.ts` | PASS | 2 files / 4 tests pass in `/Users/liying.chu/falcon-client` |
| Falcon type-check | `npm run type-check` | PASS | `tsc --noEmit` exit 0 in `/Users/liying.chu/falcon-client` |
| Runtime hygiene | `rg -n "unwrap\\(|expect\\(" src/runtime || true` | PASS with scoped caveat | hits are test-only; no request-path violations found |
| Runtime host isolation | `rg -n "\\baxum\\b|crate::server|super::server|server::" src/runtime || true` | PASS | no `axum` or `server::*` imports in runtime core |
| Secrets | `bash .agent/skills/_shared/security/scripts/scan-secrets.sh` | PASS | no hardcoded secrets, NEXT_PUBLIC sensitive vars, or console secret prints |

## QA Plan Coverage

| Area | Status | Notes |
|------|--------|-------|
| L1 static | PASS | Format and clippy gates pass. |
| L2 unit | PASS | Config, registry, input, guardrails, audit, memory, eval, and orchestrator unit tests pass. |
| L4 integration | PASS locally | Handler wire contract and runtime orchestrator fake-agent paths pass. |
| Observability | PASS locally | `/agent` and `/agent/stream` spans include prompt length, history length, `session_id`, and `option_id`. |
| LLM normalizer gate | PASS locally | Tests prove high-confidence deterministic input skips the LLM normalizer and low-confidence input can be recovered before answer policy. |
| Pipeline eval | PASS | Offline EV pack seed fixtures pass. |
| CI offline eval | PASS local config | `.github/workflows/runtime.yml` runs mandatory offline pipeline eval and response replay smoke after fmt, clippy, and tests. |
| Response replay | PASS smoke | `replay-smoke.json` validates response-eval mechanics; it is not a TS parity baseline. |
| Response baseline approval guide | PASS local / baseline pending | `docs/agent-runtime-rust-port/response-baseline-guide.md` records accepted sources, JSON shape, minimum coverage, approval steps, and rejection conditions; loader tests enforce loaded-case shape constraints. |
| Response live harness | IMPLEMENTED / not executed live | `--response --live` connects the MCP/LLM loop when required env and baseline exist; missing env returns an actionable config error. |
| Response baseline approval | PENDING | `response-baseline.json` remains pending until recorded TS responses or approved live samples exist. |
| Live/provider smoke | WAIVED for local PR | Requires provider credentials and live MCP; still opt-in before release. |
| `falcon-client` endpoint flag/cutover | PASS local / staging pending | `EOMC_AGENT_BASE_URL` selects the Rust agent endpoint; `EOMC_AGENT_SERVER_MEMORY=true` stops client memory injection upstream; `session_id` / `option_id` are forwarded. Staging smoke is still pending. |
| Rollback switch behavior | PASS local / staging pending | `RUNTIME_ENABLED` parsing is tested so unset, `false`, and `0` keep runtime routing off; staging rollback verification is still pending. |
| Staging smoke checklist/script | PASS local / execution pending | `docs/archives/staging-smoke-checklist.md` (archived) and `scripts/staging-smoke.sh` record required env, direct `/agent` and `/agent/stream` smoke commands, and evidence to append after staging. |

## Waivers

| Gate | Waiver | Owner/Date |
|------|--------|------------|
| Live response eval | Not required for local runtime mechanics; must run or be explicitly risk-accepted before staging. | AI / 2026-06-25 |
| `falcon-client` staging cutover smoke | Local endpoint flag/forwarding is implemented; staging deployment smoke remains pending. | AI / 2026-06-26 |
| TS parity response baseline | No recorded TS response artifact is available in this repo; pending baseline is intentionally rejected by replay mode. | AI / 2026-06-25 |

## Release Blockers

- Create or import an approved `config/runtime/evals/response-baseline.json` from recorded TS responses or approved live samples.
- Run accepted response eval in either replay or live mode against that approved baseline.
- Run staging smoke for `/agent` and `/agent/stream` with `RUNTIME_ENABLED=true` and `falcon-client` configured via `EOMC_AGENT_BASE_URL`.
