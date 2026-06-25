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

- `cargo` is not available in the current shell, so baseline Rust gates could not be executed during this capture.
- Required baseline commands once Rust toolchain is available:
  - `cargo test`
  - `cargo clippy --all-targets --all-features -- -D warnings`

## PR Record

| PR | Status | Notes |
|----|--------|-------|
| PR-00 | in progress | Baseline docs and contract tests added. |
| PR-01 | in progress | Runtime skeleton and dependencies added. |
