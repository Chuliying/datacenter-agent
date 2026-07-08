# Agent Runtime Response Baseline Guide

**Story ID**: S-RUNTIME-01  
**Date**: 2026-06-26  

Use this guide to replace the pending
`config/runtime/evals/response-baseline.json` with an approved PR-10 response
baseline.

The local `config/runtime/evals/replay-smoke.json` validates response-eval
mechanics only. It must not be promoted as the parity baseline unless a reviewer
explicitly approves its cases as release coverage.

## Accepted Sources

An approved response baseline may come from either source:

- Recorded `falcon-client` / TypeScript runtime responses for the same prompts.
- Approved live Rust runtime samples captured with the staging MCP and provider
  configuration.

The `provenance` field must name the source, capture date, source commit or
deployment identifier, reviewer, and any intentional parity differences.

## Baseline JSON Shape

```json
{
  "status": "loaded",
  "provenance": "TS capture from falcon-client <commit> on <date>; approved by <reviewer>; intentional diffs: <none or list>.",
  "cases": [
    {
      "id": "revenue-overview-monthly",
      "prompt": "近三個月的整體營收概況如何？",
      "expected_response": "optional exact response when deterministic enough",
      "actual_response": "recorded response used by replay mode",
      "must_include": ["營收"],
      "must_not_include": ["system prompt", "保證"],
      "max_latency_ms": 15000,
      "max_tokens": 2048,
      "expected_refused": false,
      "expected_fallback": false,
      "latency_ms": 0,
      "tokens": 0,
      "refused": false,
      "fallback": false
    }
  ]
}
```

Field rules:

- `status` must be `loaded` before response replay or live eval can pass.
- `provenance` must be non-empty and reviewable.
- `cases` must be non-empty.
- `id` must be stable and unique.
- `prompt` must be the exact user prompt sent to the runtime.
- `actual_response` is required for replay mode.
- `expected_response` is optional; use it only when exact text is intentionally
  stable.
- `must_include` and `must_not_include` should carry the primary parity and
  safety assertions.
- `max_latency_ms` and `max_tokens` should reflect the release budget.
- refusal/fallback fields must be set for refusal and gray/fallback cases.

The Rust baseline loader enforces non-empty provenance, non-empty loaded
baselines, unique non-empty case ids, and non-empty prompts. Replay mode also
requires `actual_response` per case.

## Minimum Coverage

Before staging enablement, include at least:

- one normal revenue or charging analytics answer
- one station or member ranking answer
- one `option_id` driven prompt
- one follow-up prompt intended to exercise `session_id` server memory
- one prompt-injection refusal
- one off-scope or low-confidence refusal/fallback case

## Approval Steps

1. Capture the candidate responses and fill
   `config/runtime/evals/response-baseline.json`.
2. Set `status` to `loaded` and write full provenance.
3. Run replay eval:

```bash
cargo run --bin eval -- --response --replay config/runtime/evals/response-baseline.json
```

4. If the approved gate is live eval, run:

```bash
cargo run --bin eval -- --response --live --baseline config/runtime/evals/response-baseline.json
```

5. Record the command, result, source commit/deployment, reviewer, and any
   waivers in `docs/archives/migration-log.md` (archived, gitignored).

## Rejection Conditions

Do not approve a baseline when:

- `status` is still `pending`
- `cases` is empty
- `provenance` omits source commit/deployment or reviewer
- replay cases omit `actual_response`
- all cases are smoke-only and do not cover TS parity or approved live behavior
- forbidden substrings allow prompt/system/tool leakage
