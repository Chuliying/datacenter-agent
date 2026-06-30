# datacenter-agent — System Context

> This file is an agent-facing orientation layer, not an independent architecture source.  
> **Canonical documentation**：[`docs/reference/index.md`](../../docs/reference/index.md)  
> **Target product + build status**：[`docs/reference/prd.md`](../../docs/reference/prd.md)  
> **Current implementation contract**：[`docs/reference/spec/spec.md`](../../docs/reference/spec/spec.md)  
> **Current test evidence/gaps**：[`docs/reference/tests/qa-plan.md`](../../docs/reference/tests/qa-plan.md)

## Project

`datacenter-agent` is a Rust HTTP analytics API. It connects to a datacenter MCP server with rmcp and uses an OpenRouter/OpenAI-compatible LLM tool-calling loop to answer natural-language questions.

## Current architecture

```text
main / AppState
  ├─ MCP client + discovered tools
  ├─ LLM defaults + immutable PromptBank
  ├─ bearer token + greeting cache
  └─ optional AppRuntime (built from config even when runtime routing is disabled)

axum Router
  ├─ all five routes require bearer; failure is 418
  ├─ trace + very-permissive CORS + compression
  ├─ 120s handler-future timeout + security headers + 64KiB body limit
  └─ /agent and /agent/stream
       ├─ legacy llm_connector (default)
       └─ runtime run_agent_turn (RUNTIME_ENABLED=true/1)
```

The runtime is partial, not a completed config-only platform. Current wiring and maturity are maintained in the [reference root](../../docs/reference/index.md#8-runtime-成熟度總覽).

## Stack

| Area | Current dependency |
|---|---|
| Language | Rust 2021 |
| HTTP / async | axum 0.8.9 / tokio 1.52.3 |
| MCP | rmcp 0.17.0 HTTP client |
| LLM | async-openai 0.40.3 / OpenRouter |
| HTTP client | reqwest 0.13.4 |
| Middleware | tower / tower-http: trace, CORS, compression, timeout, headers, body limit; **no rate-limit middleware** |
| Config | TOML + dotenvy |
| Logging | tracing / tracing-subscriber |

## Important current facts

- `/health`, `/ready`, `/greeting`, `/agent`, `/agent/stream` all require bearer; `auth::check` has no path exemption.
- Legacy prompt cap is 2000 chars; runtime EV-pack cap is 4000.
- Runtime SSE validates inside a spawned task, so structural errors are HTTP 200 + SSE error frame.
- `InputPipeline` hard-codes normalize→intent→slots and does not dispatch `input_stages`.
- `InjectionDetector` is validated at startup but is dormant on the request path.
- Rule answer thresholds 0.5/0.7 are hard-coded.
- Runtime SSE uses an unbounded channel and has no explicit disconnect cancellation.
- Memory production scope has `actor_id=None`; audit redaction helper is not applied by stdout sink.
- Eval can report failures while returning process exit 0.

Do not copy these facts into new planning documents. Link the canonical pages, and update those pages when code changes.

## Environment names

See [`.env.example`](../../.env.example) and `src/appstate.rs`/`src/main.rs` for defaults. Relevant names include `OPENROUTER_*`, `GLOBAL_TOKEN`, `DATACENTER_MCP_URL`, `RUNTIME_ENABLED`, `HOST`, `PORT`, and `RUST_LOG`. Never record secret values in documentation or logs.

## Development boundaries

Read [`.agent/guardrails.md`](../guardrails.md) and [`.agent/project-manifest.md`](../project-manifest.md). Future code work must be derived from the [plan-sync implementation](../artifacts/plan/2026-06-29-runtime-correctness/implementation.md), not inferred from historical migration docs.
