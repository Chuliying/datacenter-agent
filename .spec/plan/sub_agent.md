# SubAgent — Implementation Plan

Turns the [SubAgent contract](../contract/sub_agent/Contract.md) into working code inside
`datacenter-agent`. The contract fixes the *what* (config model + resolution/composition
rules); this plan covers the *how* and the items the contract explicitly deferred (§6 there).

Sequenced so each step compiles and is testable on its own. Nothing here changes the
normative payload or sub-agent contracts.

---

## 0. Where the pieces land in `src/`

| Concern | Home | Notes |
|---|---|---|
| Payload types (`AgentPayload`, `AgentError`, `Tool`, `LlmCapability`, `run_llm_loop`) | `src/agent/payload.rs` | Port of `.spec/contract/agent_payload/agent_payload.rs`. |
| Config model + resolution (`SubAgentConfig`, `LlmConfig`, `ToolId`, `ResolvedLlm`, merge, secret bind) | `src/agent/config.rs` | Port of `.spec/contract/sub_agent/sub_agent.rs` PART A + resolution. |
| Generic engine + orchestrator (`ConfiguredAgent`, `Orchestrator`, `resolve_pipeline`) | `src/agent/engine.rs` | PART B. |
| Tool registry + MCP-backed tools | `src/agent/tools.rs` | Wraps one or more `McpHandle`s (`src/mcp_client.rs`); registry is backend-agnostic. |
| MCP server pool (N connections + per-server instructions) | `src/agent/mcp_pool.rs` | Connects every configured server at boot; owns handles + instruction blocks. |
| LLM factory (`ResolvedLlm` → `Arc<dyn LlmCapability>`) | `src/agent/llm.rs` | async-openai adapter; bumps the crate to 0.41.1 (see step 6). |
| TOML raw schema + loader | extend `src/config.rs` | `[llm.default]`, `[[sub_agent]]`, `[[pipeline]]`. |

The reference `.rs` files compile standalone; porting is mostly moving code into modules and
replacing the `#[path]` include with `use crate::agent::payload::*`.

---

## 1. Land the contracts as code (no behavior change yet)

1. Copy `agent_payload.rs` → `src/agent/payload.rs`; drop the `#[path]`-style standalone bits;
   keep the `#[cfg(feature = "openai")]` adapter but gate behind a crate feature.
2. Copy `sub_agent.rs` → split into `config.rs` / `engine.rs` / `tools.rs` per the table;
   replace the `mod agent_payload` include with `use crate::agent::payload::…`.
3. Bring the reference tests across. **Exit check:** `cargo test` green, `cargo clippy` clean.

## 2. Multi-server MCP pool + tool registry

The registry in the contract is backend-agnostic (each `ToolId` binds its own backend), so
**several MCP servers are supported with no type change** — this step makes that concrete.

- **Config surface for N servers.** Replace the single `DATACENTER_MCP_URL` + one
  `[endpoints]` block with `[[mcp_server]]` entries: `{ id, url, tools = [<ToolId>, …] }`.
  Each `ToolId` variant declares (via its server) which backend serves it. One server remains
  the common case; the schema simply stops hard-coding *one*.
- **`McpPool`.** At boot, connect to every configured server (`McpClient::connect_http`),
  keep a `HashMap<McpServerId, McpHandle>` plus each server's handshake `instructions`
  (`McpHandle::server_instructions`). **Fail-fast:** any failed handshake aborts boot, naming
  the server.
- **`McpTool`.** `McpTool { handle: McpHandle, mcp_name: String, target: ArtifactKey, id: ToolId }`
  implementing `Tool`; `call()` delegates to `McpHandle::call_tool_text`. Its **advertised
  schema name is the canonical `ToolId` string, not `mcp_name`** — so two servers exposing the
  same raw tool name never collide within an agent's exposed set. `call()` still sends
  `mcp_name` to its own server.
- **Registry population.** Build one `ToolFactory` per `ToolId`, each capturing the correct
  server's `McpHandle` from the pool. Resolve every `SubAgentConfig.tools` grant at boot;
  abort with a clear error listing the offending `(sub_agent, ToolId)`.
- **Exit check:** a unit test resolves a grant spanning two mock servers, rejects an
  unregistered `ToolId`, and asserts two same-named raw tools get distinct advertised names.

## 3. Per-server MCP instruction routing

MCP handshake `instructions` are **per-server** conventions. Today they are appended globally
to every prompt ([appstate.rs `generation_config`](../../src/appstate.rs)); with several
servers and agents granted tool subsets, that is wrong.

- Attach each server's `instructions` to its pool entry.
- When building a `ConfiguredAgent`, compute the **distinct set of servers backing its granted
  tools** and compose *those* instruction blocks (deduplicated) into its system prompt,
  alongside the agent's own `instruction`. A no-tool agent gets none.
- Keep the existing "Current Time" + base-prompt assembly; only the instructions source
  changes from one global block to the per-agent server set.
- **Exit check:** an agent granted tools from server A only never sees server B's instructions;
  an agent spanning A+B sees both, once each.

## 4. TOML raw→resolved loader

- Add raw serde structs to `src/config.rs` mirroring the contract's §3 TOML surface (plus the
  `[[mcp_server]]` block from step 2), with
  `#[serde(deny_unknown_fields)]` (matches the existing `Manifest` discipline).
- `accepts` / `output` / `provider` / tool names arrive as strings → parse into the closed
  enums (`PayloadKind`, `OutputShape`, `Provider`, `ToolId`) during resolution, not deserialize
  (the contract enums intentionally have no `serde`), reporting the bad token on failure.
- Resolve prompts via the existing `load_prompt` path (relative to the manifest dir).
- Bump `SUPPORTED_VERSION` handling if the schema shape changes.
- **Exit check:** load a fixture `config.toml` with two agents + two pipelines; assert the
  resolved `Orchestrator` has both pipelines and the shared agent is reused.

## 5. Boot-time secret validation (contract §2.3)

- Read secrets from the process environment (keep `dotenvy` as today).
- For every resolved provider, bind its `SecretRef` against the environment; a referenced key
  with no entry aborts boot with `secret <KEY> not present in environment`.
- Collect *all* missing secrets before aborting (report the full set, not just the first) so an
  operator fixes one deploy, not N.
- **Exit check:** boot fails deterministically when `OPENROUTER_API_KEY` is unset for an
  OpenRouter agent; an all-Ollama config boots with no secrets.

## 6. LLM factory (`ResolvedLlm` → `Arc<dyn LlmCapability>`)

- Implement in `src/agent/llm.rs` behind the `openai` feature. One `OpenAiLlm` per distinct
  `ResolvedLlm`, carrying model + params + base URL + bound key.
- **async-openai:** the contract's adapter targets **0.41.1**; the crate is on **0.40**
  (`Cargo.toml`). Bump and re-check the chat/tool type paths (`types::chat`,
  `ChatCompletionTools`) — this is the one dependency change with real surface area.
- Ollama / Custom are the same adapter with a different base URL and (maybe absent) key.
- **Global attribution block:** move OpenRouter `HTTP-Referer` / `X-Title` out of per-request
  config into one app-level block (they identify the app, not a per-agent provider); the
  current `build_client` in `src/llm_connector/client.rs` shows where they attach.
- **Exit check:** a live smoke test against one real model per provider kind available.

## 7. Wire the orchestrator into the HTTP layer

- `AppState` holds the `Orchestrator` (replacing the single shared tool list + `GenerationConfig`).
- Request → select a `PipelineId` (initially a fixed/default pipeline; routing rule is a
  follow-up) → build the `Initial` payload from prompt + history → `orchestrator.run(id, …)`.
- Preserve streaming: the terminal agent's `run_llm_loop` is where `LlmEvent` tokens originate
  today (`src/llm_connector/agent.rs`); keep that stream for the final stage, run upstream
  stages buffered. (Streaming across a multi-stage pipeline is its own design note.)
- **Exit check:** `/agent` returns the same shape as today for a one-stage pipeline.

## 8. Migration & compatibility

- Map the current single-agent flow to a **one-stage pipeline** (`stages = ["main"]`) whose
  agent holds today's full MCP tool set — behavior-preserving default.
- Keep the old path working until the config-driven path is proven, then remove.
- Update `config/` fixtures and `README`.

---

## Deferred beyond this plan

- **Namespace enforcement** (contract §2.5) — a post-run validator that an agent wrote only
  keys under its own `id`. Cheap; add when a real collision risk appears.
- **Pipeline routing** — how a request chooses among multiple pipelines (header, path, or an
  LLM classifier). Design once more than one pipeline exists in production.
- **Multi-stage streaming** — token streaming semantics when non-terminal stages also emit.

---

## Risk notes

- **async-openai 0.40 → 0.41.1** (step 6) is the only externally-forced change; the tool-call
  enum shapes shifted across releases. Do it in an isolated commit.
- **Streaming vs. staged execution** — today's single loop streams directly; a pipeline must
  decide where the user-visible stream begins. Keep it to the terminal stage first.
- **`ResolvedLlm` deduplication** — many agents may share one `ResolvedLlm`; build one client
  per distinct config, not per agent, to avoid connection sprawl.
