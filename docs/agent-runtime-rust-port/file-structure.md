# Agent Runtime Rust 移植 — 嚴謹 File Structure

**版本**: v1.0.0  
**狀態**: draft  
**SSOT**: 本檔定義落地檔案結構；行為細節以 `spec/` 分檔為準。

---

## 1. 原則

1. `src/runtime/` 是領域無關 core：不得 import `axum`、HTTP DTO、handler、route、auth。
2. `src/server/` 是 host edge：負責 HTTP、SSE、auth、DTO、`AppError` 映射。
3. `src/llm_connector/` 保留 MCP/LLM loop，但必須讓 tool metadata 以 internal `AgentTurnFrame::{ToolCalled, ToolResult}` 被 orchestrator audit。
4. `config/runtime/` 是 capability pack：領域內容、模組組裝、eval fixtures 都在這裡。
5. 測試跟著模組走；跨模組 turn flow 用 fake trait object，不打 live LLM/MCP。

---

## 2. 目標檔案樹

```text
datacenter-agent/
├── Cargo.toml
├── src/
│   ├── lib.rs
│   ├── main.rs
│   ├── config.rs
│   ├── appstate.rs
│   ├── model.rs
│   ├── mcp_client.rs        # 既存 host 檔，移植不動（main/appstate/agent 使用中）
│   ├── llm_connector/
│   │   ├── mod.rs
│   │   ├── client.rs
│   │   └── agent.rs
│   ├── server/
│   │   ├── mod.rs
│   │   ├── route.rs
│   │   ├── auth.rs
│   │   ├── dto.rs
│   │   ├── error.rs
│   │   ├── greeting.rs
│   │   └── handler.rs
│   ├── runtime/
│   │   ├── mod.rs
│   │   ├── config.rs
│   │   ├── registry.rs
│   │   ├── schema.rs
│   │   ├── error.rs
│   │   ├── orchestrator.rs
│   │   ├── audit.rs
│   │   ├── input/
│   │   │   ├── mod.rs
│   │   │   ├── normalizer.rs
│   │   │   ├── intent.rs
│   │   │   ├── slots.rs
│   │   │   └── pipeline.rs
│   │   ├── guardrails/
│   │   │   ├── mod.rs
│   │   │   ├── injection.rs
│   │   │   ├── input_guard.rs
│   │   │   └── answer_policy.rs
│   │   ├── llm_normalizer.rs
│   │   ├── memory/
│   │   │   ├── mod.rs
│   │   │   ├── store.rs
│   │   │   └── context.rs
│   │   └── eval/
│   │       ├── mod.rs
│   │       ├── evaluator.rs
│   │       ├── fixtures.rs
│   │       ├── baseline.rs
│   │       ├── report.rs
│   │       └── runner.rs
│   └── bin/
│       └── eval.rs
├── config/
│   ├── config.toml
│   ├── prompt_guide/
│   │   ├── agent_system.md
│   │   ├── greeting_system.md
│   │   └── greeting_user.md
│   └── runtime/
│       ├── intents.toml
│       ├── lexicon.toml
│       ├── thresholds.toml
│       ├── injection.toml
│       └── evals/
│           ├── inputs.json
│           └── response-baseline.json
├── tests/
│   ├── llm_connector.rs
│   └── runtime_contract.rs
└── docs/
    └── agent-runtime-rust-port/
        ├── meta.yml
        ├── prd.md
        ├── runtime-architecture-spec.md
        ├── file-structure.md
        └── spec/
            ├── spec-overview.md
            ├── spec-01-config-registry.md
            ├── spec-02-input.md
            ├── spec-03-guardrails.md
            ├── spec-04-audit-memory.md
            ├── spec-05-orchestrator.md
            └── spec-06-eval.md
```

> **既存 host 檔（移植不動，僅列於樹中供完整對照）**：`src/main.rs`、`src/lib.rs`（僅加 `pub mod runtime;`，見 §5）、`src/model.rs`、`src/mcp_client.rs`。  
> **同名注意**：`src/config.rs`（host manifest，`deny_unknown_fields`）≠ `src/runtime/config.rs`（capability pack 載入器）；兩者責任各見 §5 與 §3，勿混。

---

## 3. Runtime Core Responsibilities

| Path | Responsibility | Must Not Depend On |
|------|----------------|--------------------|
| `src/runtime/mod.rs` | Re-export runtime public surface. | `axum`, `server::*` |
| `src/runtime/config.rs` | Load capability pack refs, parse TOML/JSON, validate allowlists, assembly ids, audit failure policy, and input limits. | HTTP DTOs |
| `src/runtime/registry.rs` | Map config ids to trait constructors: input stages, answer policy, memory, audit, extractors, evaluators. | Runtime request state |
| `src/runtime/schema.rs` | Domain-neutral structs: `NormalizedInput`, `NormalizedSlots`, `RegistryVersions`, eval-observable shape. | Concrete capability content |
| `src/runtime/error.rs` | `RuntimeError` for config and per-request runtime failures. | `AppError` |
| `src/runtime/orchestrator.rs` | Own one turn from raw `AgentTurnInput` to final outcome; emit audit and wire frames. | `axum`, `Json`, `Sse` |
| `src/runtime/audit.rs` | `AuditSink`, `AuditEvent`, redaction/hash helpers, stdout sink, failure policy handling. | HTTP framework types |
| `src/runtime/llm_normalizer.rs` | Optional `LlmInputNormalizer` trait + disabled/default implementation; async enhancement after rule pipeline. | Concrete model provider |

---

## 4. Submodule Responsibilities

### Input

| Path | Responsibility |
|------|----------------|
| `input/normalizer.rs` | NFKC + explicit CJK/fullwidth punctuation map + whitespace/case normalization. |
| `input/intent.rs` | `option_id` option-path, lexicon scoring, margin tiers, text override. |
| `input/slots.rs` | Slot extractor trait + time/metric/asset/rank extractors; asset unknown check uses config allowlist. |
| `input/pipeline.rs` | Execute `[runtime.pipeline].input_stages` in order; synchronous only. |

### Guardrails

| Path | Responsibility |
|------|----------------|
| `guardrails/input_guard.rs` | Required/length checks before LLM/MCP. |
| `guardrails/injection.rs` | Versioned regex heuristics; JS regex portability handled explicitly. |
| `guardrails/answer_policy.rs` | `AnswerPolicy` trait + rule implementation. |
| `guardrails/input_guard.rs` | Uses `[input].max_prompt_chars`; EV parity pack uses 4000 chars. |

### Memory

| Path | Responsibility |
|------|----------------|
| `memory/store.rs` | `SessionMemoryStore` trait and in-memory implementation. |
| `memory/context.rs` | Sanitize/truncate/budget memory hints; build untrusted memory prompt section. |

### Eval

| Path | Responsibility |
|------|----------------|
| `eval/evaluator.rs` | `Evaluator` trait + impls registered by id: `pipeline-deterministic` (pipeline), `response-baseline` / `llm-judge` (response). Ids match `[runtime.eval]` in §6. |
| `eval/fixtures.rs` | Load `config/runtime/evals/inputs.json`. |
| `eval/baseline.rs` | Load and compare `response-baseline.json`. |
| `eval/report.rs` | Aggregate pass/fail, latency, token, refuse/fallback metrics. |
| `eval/runner.rs` | Run pipeline-only, response replay, or live response eval modes. |
| `src/bin/eval.rs` | CLI wrapper; `--pipeline-only` is CI-safe. |

---

## 5. Edge / Connector Touch Points

| Path | Required Change |
|------|-----------------|
| `Cargo.toml` | Add `uuid` / `sha2` / `unicode-normalization` / `regex` / `async-trait` / `thiserror` deps (P1). |
| `src/lib.rs` | Add `pub mod runtime;` to expose the runtime core (P1). |
| `src/config.rs` | Add `[runtime]` refs and assembly sections while preserving `deny_unknown_fields`. |
| `src/appstate.rs` | Store `Arc<RuntimeConfig>`, input pipeline, answer policy, memory store, audit sink, agent port. |
| `src/server/dto.rs` | Add `session_id: Option<String>` and `option_id: Option<String>` to `AgentRequest`. |
| `src/server/handler.rs` | Call orchestrator for `/agent` and `/agent/stream`; map runtime outcomes to existing JSON/SSE wire. |
| `src/llm_connector/agent.rs` | Surface tool called/result metadata through internal frames; preserve external SSE contract. |

---

## 6. Config Pack Structure

```toml
# config/config.toml
[runtime]
intents    = "runtime/intents.toml"
lexicon    = "runtime/lexicon.toml"
thresholds = "runtime/thresholds.toml"
injection  = "runtime/injection.toml"

[runtime.pipeline]
input_stages = ["normalize","input_guard","injection","intent","slots"]

[runtime.answer_policy]
backend = "rule"

[runtime.llm_normalizer]
enabled = false
backend = "disabled"

[runtime.memory]
enabled = true
backend = "in-memory"

[runtime.audit]
sink = "stdout"
failure_policy = "fail-open"

[runtime.guardrails]
enabled = ["injection","input_guard","answer_policy"]

[runtime.slots]
extractors = ["time_range","metric","asset","rank_limit"]

[runtime.eval]
pipeline_evaluators = ["pipeline-deterministic"]
response_evaluators = ["response-baseline","llm-judge"]
fixtures = "runtime/evals/inputs.json"
baseline = "runtime/evals/response-baseline.json"
```

---

## 7. Test Placement

| Test Kind | Location | Required Coverage |
|-----------|----------|-------------------|
| Unit | `#[cfg(test)]` inside runtime modules | normalizer, intent, slots, injection, answer policy, memory context, audit redaction |
| Trait contract | `tests/runtime_contract.rs` or module tests | `SessionMemoryStore`, `AuditSink`, `AgentPort`, `Evaluator` |
| Orchestrator integration | `src/runtime/orchestrator.rs` tests | refusal, disclaimer, clear buffer, tool audit frames, rejected request audit, abort |
| Config contract | `src/runtime/config.rs` tests | unknown id, missing `unknown`, invalid option prefix, duplicate intent, evaluator ids |
| Eval | `src/runtime/eval/*` tests | fixtures load, pipeline-only pass/fail, baseline regression, live/replay gating |

---

## 8. Non-Negotiable Boundaries

- `runtime/` request path must not use `unwrap` / `expect`.
- External SSE remains `token` / `done` / `error` / `clear`; `ToolCalled` and `ToolResult` are internal audit frames only.
- `option_id` is optional. Known prefixes map to option-path intent; unknown prefixes are warning + fallback to text lexicon/unknown, not 400.
- `session_id` controls memory scope; `option_id` must never become part of the memory key.
- Pipeline eval must run without provider credentials.
- Response eval must declare whether it uses live LLM or replay artifacts.
