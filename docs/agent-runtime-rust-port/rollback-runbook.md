# Agent Runtime Rust Port Rollback Runbook

**Story ID**: S-RUNTIME-01  
**Date**: 2026-06-26  

## Rollback Switches

| Scope | Switch | Safe Value |
|-------|--------|------------|
| `datacenter-agent` runtime route wiring | `RUNTIME_ENABLED` | unset, `false`, or `0` |
| `falcon-client` endpoint selection | `EOMC_AGENT_BASE_URL` | unset or current stable agent base URL |
| `falcon-client` server memory mode | `EOMC_AGENT_SERVER_MEMORY` | unset or `false` |
| Response eval release gate | approved response baseline | keep previous accepted artifact |

`RUNTIME_ENABLED` defaults off in `datacenter-agent`; if it is unset, handlers keep using the existing `llm_connector` path.

## Rollback Steps

1. Disable the frontend endpoint flag so clients stop selecting the Rust runtime path.
2. Unset `EOMC_AGENT_BASE_URL` or set it back to the current stable agent base URL.
3. Set `EOMC_AGENT_SERVER_MEMORY=false` if rolling back to client-side memory prompt injection.
4. Set `RUNTIME_ENABLED=false` for `datacenter-agent` and restart the service.
5. Confirm `/agent` returns `AgentResponse { user_prompt, model_response }`.
6. Confirm `/agent/stream` emits only `token`, `done`, `error`, and `clear` frames.
7. Run the local smoke gate:

```bash
cargo test --test runtime_contract
cargo run --bin eval -- --pipeline-only
```

8. Keep the TS runtime fallback available until the agreed soak period closes.

## Rollback Verification

| Check | Expected |
|-------|----------|
| `RUNTIME_ENABLED=false` or unset | old handler path selected |
| `EOMC_AGENT_BASE_URL` unset/restored | frontend selects stable agent endpoint |
| `EOMC_AGENT_SERVER_MEMORY=false` or unset | frontend can keep legacy client memory prompt injection |
| `/agent` schema | unchanged JSON response |
| `/agent/stream` schema | no internal tool frames exposed |
| Pipeline eval | still green; rollback does not change capability pack fixtures |
| Audit logs | runtime audit stops for route traffic when route wiring is disabled |

## Roll-Forward Criteria

- Static, test, and pipeline eval gates pass.
- Response eval live or replay passes against an approved baseline.
- Staging smoke passes with `RUNTIME_ENABLED=true`.
- Frontend endpoint flag and `session_id` / `option_id` forwarding are verified.
