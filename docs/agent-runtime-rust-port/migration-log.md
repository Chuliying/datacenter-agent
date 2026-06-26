# Agent Runtime Rust Port — Migration Log

## Source Reference

- Source repo: `/Users/liying.chu/falcon-client`
- Source branch: `feature/chief-of-staff-llm-streaming`
- Source commit: `764d45c195c16b14d9aea64a8ab928e8e9a17eaa`
- Source status at capture: dirty working tree; commit hash recorded as the immutable reference, uncommitted files are not part of the baseline.
- Target repo: `datacenter-agent`
- Target branch: `runtime-test`
- Target commit at capture: `99ed3645860b2ffc265678214781ad82e5756160`
- Capture date: `2026-06-25`
- Owner: TBD

## Baseline Scope

- Existing `/agent` prompt validation remains capped at `USER_PROMPT_LENGTH_CAP = 2000` until runtime guardrails take over behind `RUNTIME_ENABLED`.
- Existing `/agent/stream` external events remain `token`, `done`, `error`, and `clear`.
- The approved future parity diff is that runtime config will move prompt length from the host cap to EV pack `input.max_prompt_chars = 4000`.

## Verification Notes

- Rust toolchain is available in the current shell via `PATH="$HOME/.cargo/bin:/opt/homebrew/opt/rustup/bin:$PATH"`.
- Latest local verification on `2026-06-26`:
  - `cargo fmt -- --check`
  - `cargo clippy --all-targets --all-features -- -D warnings`
  - `cargo test`
  - `cargo run --bin eval -- --pipeline-only`
  - `cargo run --bin eval -- --response --replay config/runtime/evals/replay-smoke.json`
  - `env -u OPENROUTER_API_KEY -u OPENROUTER_MODEL cargo run --bin eval -- --response --live` returns an actionable missing-env error
  - `/Users/liying.chu/falcon-client`: `npm test -- --run src/lib/chief-of-staff/agent-client.test.ts src/lib/chief-of-staff/agent-runtime/run-agent-turn.test.ts`
  - `/Users/liying.chu/falcon-client`: `npm run type-check`
- QA evidence: `.agent/artifacts/qa/2026-06-25-agent-runtime-rust-port/qa-report.md`
- Rollback runbook: `docs/agent-runtime-rust-port/rollback-runbook.md`
- Completion audit: `docs/agent-runtime-rust-port/completion-audit.md`
- Response baseline remains pending until recorded TS responses or an approved live/replay sample is available. The local `replay-smoke.json` only validates response-eval mechanics.

## PR Record

| PR | Status | Notes |
|----|--------|-------|
| PR-00 | implemented locally | Baseline docs and contract tests added; old route behavior remains default. |
| PR-01 | implemented locally | Runtime skeleton and dependencies added. |
| PR-02 | implemented locally | Runtime config, schema, errors, and registry load and validate the EV capability pack. |
| PR-03 | implemented locally | EV capability pack config and seed eval fixtures added; response baseline file records pending provenance. |
| PR-04 | implemented locally | Deterministic input normalization, intent, slots, and pipeline eval are implemented. |
| PR-05 | implemented locally | Input guardrails, injection detection, and rule answer policy are implemented. |
| PR-06 | implemented locally | Audit sink/policy and in-memory session memory are implemented. |
| PR-07 | implemented locally | LLM adapter maps internal token/clear/tool/result/done/error frames while external SSE filters tool metadata. |
| PR-08 | implemented locally | Runtime orchestrator owns one turn, including optional normalizer and server-memory prompt injection. |
| PR-09 | implemented locally | AppState and handlers route through runtime only when `RUNTIME_ENABLED=true`; flag default remains off. |
| PR-10 | partial | Pipeline eval, response replay CLI, response-eval rules, local replay smoke, and guarded live MCP/LLM harness are implemented; approved TS/live baseline remains pending. |
| PR-11 | partial | Local L0-L4 QA evidence and hygiene checks are documented; approved response eval and release live smoke remain pending. |
| PR-12 | partial | Rollback runbook and local `falcon-client` endpoint flag/forwarding are implemented; staging config enablement and staging smoke remain pending. |
