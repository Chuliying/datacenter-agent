# SubAgent — Implementation Plan

Turns the [SubAgent contract](../contract/sub_agent/Contract.md), the
[AgentPayload contract](../contract/agent_payload/Contract.md), and the
[Tool contract](../contract/tool/Contract.md) into working code inside `datacenter-agent`. The
contracts fix the *what* (payload sum type + config model + tool abstraction + resolution/
composition rules); this plan covers the *how*, the items the contracts deferred (sub-agent §6,
payload §6, tool §6), and the concrete migration of **today's monolithic endpoints into
sub-agent pipelines**.

Sequenced so each step compiles and is testable on its own. Nothing here changes the
normative payload or sub-agent contracts.

---

## 0. Snapshot: the monolith we are migrating from

Read this first — the earlier draft of this plan described a config surface that does not
exist in the tree. The real starting point:

- **One MCP server, auto-discovered tools.** [`main.rs`](../../src/main.rs) connects to a
  single server (`DATACENTER_MCP_URL`) and calls `McpClient::list_openrouter_tools()` at boot,
  storing **every** discovered tool as one `Arc<Vec<ChatCompletionTool>>` in
  [`AppState`](../../src/appstate.rs). There is **no `[endpoints]` block, no `ToolId`, no
  enumerated tool set** — tools are whatever the server advertises.
- **One LLM, hardcoded OpenRouter.** [`LlmDefaults::from_env`](../../src/appstate.rs) reads
  `OPENROUTER_*`. No provider enum, no per-agent LLM, no secret-ref indirection.
- **Four endpoints, one loop.** [`handler.rs`](../../src/server/handler.rs): `/agent` +
  `/agent/stream` and `/report` + `/report/stream` differ **only by system prompt and
  `max_tokens`** (`agent_system` @ 4096 vs `report_system` @ 16384). All four feed the *same*
  full tool set into the *same* streaming tool-loop
  ([`llm_connector::agent_stream`](../../src/llm_connector/agent.rs)).
- **A pre-existing `runtime/` turn-orchestrator — NOT the contract's orchestrator.**
  [`runtime/orchestrator.rs`](../../src/runtime/orchestrator.rs) owns `run_agent_turn`, an
  `AgentPort` trait, and an *input* "pipeline" (normalize → guard → intent → slots) plus
  answer-policy, memory, and audit. Its `LlmAgentPort` wraps the single streaming loop. `/agent`
  routes through this (default `RUNTIME_ENABLED=on`); `/report` **bypasses it** on the legacy
  direct path.

> **Naming: the sub-agent layer owns the bare terms (Option B).** *Orchestrator* and *pipeline*
> read ambiguously today because `runtime/` uses them for a per-turn input-processing concept
> while the contract uses them for agent composition. Rather than coexist behind qualifiers
> forever, **§2 reserves the bare terms for the sub-agent layer first** — a small, gated,
> behavior-preserving rename (`runtime::orchestrator → runtime::turn`,
> `[runtime.pipeline] → [runtime.input]`) — so the new `src/agent/` modules land in a clean
> namespace. After §2 the rule is: bare `Pipeline` / `Orchestrator` ⇒ `src/agent/`; the runtime
> speaks of *turns* and *input stages*. The measured blast radius is small because the runtime
> never took the bare **type** names (it uses `InputPipeline` + free functions; there is **no**
> `Orchestrator` type today).

### Locked decisions for this plan

| Decision | Choice |
|---|---|
| Term ownership | **Sub-agent layer owns bare `pipeline` / `orchestration`** (Option B). Runtime renamed to `turn` / `input` **first**, behind a behavior-preserving gate (§2). |
| Agent anatomy | **Three optional components — LLM, Tools, Logic** — behind one `SubAgent` trait unifying **config-defined** (`ConfiguredAgent`) and **code-defined** (`agents.rs`) agents (contract §1.1). The two endpoints are all config; code path is groundwork. |
| `/report` shape | **Three-stage** `fetcher → charter → finalizer` — the `finalizer` combines the fetcher's data with the `charter`'s schema-checked chart artifacts into the final HTML (§10). |
| Sub-agent orchestrator ↔ runtime | **Pipeline sits *behind* `AgentPort`** — a `PipelineAgentPort` replaces `LlmAgentPort`; guardrails/intent/memory/audit are preserved. |
| Tool set | **Closed, hand-authored `ToolId` enum** + registry, resolved at boot (contract §1.3, §2.2). Auto-discovery is replaced. Each grant additionally binds an explicit wire name (`mcp_name`) when it differs from the canonical `ToolId` string (§4). |
| Streaming | An injected **`EventSink`** (same idiom as the runtime's `TurnEmit`) with a **no-op** for the buffered path. *All* stages emit **process** events (`Stage*` / `Tool*` / reasoning) onto it; only the **terminal** stage additionally streams **content tokens**. Buffered-vs-streaming is a capability choice at wiring — the normative `run(payload)->Result` morphism is unchanged. Per-turn sink injection via **mechanism A** (sink baked into per-turn capabilities), `task_local` as escape hatch (§8.5). |
| async-openai | Build the adapter against the **0.40** already in the tree. A survey (§8.7) shows **0.40 → 0.41.1 is near-zero blast radius but unlocks nothing we need** — reasoning tokens are absent from the streaming delta in *every* version; the fix is the **`byot`** feature (already in 0.40.3), not a bump. Reasoning is **dropped for the first cut**. |
| `output` default | **Position-derived when unset**, per the amended contract (§1.1/§2.4/§4 of `Contract.md`): a stage's `Final` vs `Intermediate` shape is computed from whether it is terminal in every pipeline that references it. Ambiguous position (terminal in one pipeline, not in another) fails resolution and demands an explicit value. This removed a real footgun — `output` and pipeline position could previously disagree — and, for our two pipelines below, means **no agent needs to set `output` at all**. |
| Turn time | **Time-as-data (option B)** — a `Clock` stamps `now` once at the boundary; every payload variant carries it; stages *render* it, never read a clock. Detail: §12.1. |
| Artifact key | **Open `{agent}.{name}` string** (was a closed enum), so any produced value — tool result, agent message, computed value — is keyed uniformly; `ToolId` stays closed. Detail: §12.2. |
| Message capture | **Per-stage `capture_message`, default-on, suppress throwaway**; `FinalResult` carries the full provenance map. Detail: §12.3. |

---

## 1. Where the pieces land in `src/`

| Concern | Home | Notes |
|---|---|---|
| Payload types (`AgentPayload`, `AgentError`, `Tool`, `LlmCapability`, `run_llm_loop`) | `src/agent/payload.rs` | Port of `agent_payload.rs`. Drop the `#[cfg(feature = "openai")]` gate — the crate depends on async-openai unconditionally. |
| Config model + resolution (`SubAgentConfig`, `LlmConfig`, `Provider`, `ToolId`, `ResolvedLlm`, merge, secret bind) | `src/agent/config.rs` | Port of `sub_agent.rs` PART A + resolution. |
| Generic engine + sub-agent orchestrator (`ConfiguredAgent`, `Orchestrator`, `resolve_pipeline`, `effective_output`) | `src/agent/engine.rs` | PART B. Bare `Orchestrator` / `Pipeline*` names, free after §2. Uses the `sub_agent.rs` `SubAgent` trait (`id()` + `accepts()`, **no `produces()`**). `ConfiguredAgent` is the **config path** — its Logic is the built-in LLM tool-loop; it always holds an LLM. `ConfiguredAgent::new` takes the *resolved* `OutputShape` as an explicit argument (never reads `cfg.output` itself) — `effective_output` computes it from the full pipeline set before construction. |
| Code-defined agents (hand-written `SubAgent` impls) | `src/agent/agents.rs` | The **code path** (contract §1.1): agents whose Logic is Rust, with any component absent — a Logic-only session-memory keeper, a fixed responder. Each is `Arc<dyn SubAgent>`, inserted into the resolved agent map beside config agents before `resolve_pipeline` (§6). The reference's `HelloWorld` is the template. |
| Tool registry + MCP-backed tools | `src/agent/tools.rs` | Wraps one or more `McpHandle`s; registry is backend-agnostic. |
| MCP server pool (N connections + per-server instructions) | `src/agent/mcp_pool.rs` | Connects every configured server at boot; owns handles + instruction blocks. |
| LLM factory (`ResolvedLlm` → `Arc<dyn LlmCapability>`) + streaming capability | `src/agent/llm.rs` | async-openai adapter (0.40); buffered `chat` + a streaming path for the terminal stage. |
| Pipeline-as-transport bridge (`PipelineAgentPort`) | `src/agent/port.rs` | Implements `runtime::turn::AgentPort` (the module renamed in §2); runs a selected pipeline, streaming the terminal stage. |
| TOML raw schema + loader (`[llm.default]`, `[[mcp_server]]`, `[[sub_agent]]`, `[[pipeline]]`) | extend `src/config.rs` | Mirrors the existing `Manifest` discipline (`deny_unknown_fields`). |

The reference `.rs` files compile standalone; porting is mostly moving code into modules and
replacing the `mod agent_payload` include with `use crate::agent::payload::*`.

**The three-component model (contract §1.1).** An agent = up to three *optional* components —
**LLM** (talk to the model), **Tools** (interact with the real world), **Logic** (the actions
it processes, i.e. the `run` procedure). Two provenances, one `SubAgent` trait: **config-defined**
(`ConfiguredAgent` — always an LLM, optional tools, Logic = the built-in tool-loop) and
**code-defined** (`agents.rs` — arbitrary Logic, any component absent). This plan's two
endpoints (§10) are *all config* agents; `agents.rs` exists for the cases a prompt can't express
and stays empty until one appears. The Logic can be iterative — the fetcher's "loop tool calls
until the data is ready" *is* the built-in loop ([`llm_connector/agent.rs`](../../src/llm_connector/agent.rs),
`MAX_ITERATIONS`), driven by the model, not a fixed script.

---

## 2. Reserve the vocabulary — Option B rename (behavior-preserving, do this first)

Before any sub-agent code lands, give the bare terms *pipeline* and *orchestration* to the
sub-agent layer, so the new modules land in a clean namespace instead of coexisting behind
`Input*` / `agent::pipeline::` qualifiers forever. The runtime never took the bare **type**
names (it uses `InputPipeline` and free functions; there is **no** `Orchestrator` type), so
this is a rename of one module, one config key, and prose — **not** a refactor.

**The naming rule this establishes.** Bare `Pipeline` / `Orchestrator` / "orchestration" ⇒ the
sub-agent layer (`src/agent/`). The runtime speaks of *turn* and *input*. Names that already
carry a disambiguating qualifier stay (`InputPipeline`, `pipeline_evaluators`,
`--pipeline-only`) — they never compete with a bare `Pipeline`.

Renames (each is pure identifier/key substitution, no logic change):

1. **Module `runtime::orchestrator` → `runtime::turn`.** It runs one agent *turn*
   (`run_agent_turn`, `plan_stream_turn`, `TurnEvent`), so the name finally matches. Touches:
   the `pub mod orchestrator;` decl in [`runtime/mod.rs`](../../src/runtime/mod.rs), the file
   rename `orchestrator.rs → turn.rs`, the single import site
   [`handler.rs:41`](../../src/server/handler.rs), the test name
   `rest_consumes_same_orchestration_with_noop_emit`, and the doc page
   `docs/reference/modules/runtime-orchestrator.md`. `AgentPort` / `AgentTurnDeps` /
   `AgentTurnOutcome` / `TurnEvent` keep their names (already turn-scoped).
2. **Config key `[runtime.pipeline]` → `[runtime.input]`.** Its body is `input_stages`, so
   `[runtime.input]` reads truer and stops a bare `[[pipeline]]` (sub-agent) from sitting beside
   a same-named runtime block. Touches: [`config.toml`](../../config/config.toml), the
   `RuntimePipelineManifest` struct + its `manifest.pipeline.input_stages` access in
   [`config.rs`](../../src/config.rs), the two `"runtime.pipeline"` error-section labels in
   [`registry.rs`](../../src/runtime/registry.rs) /
   [`runtime/config.rs`](../../src/runtime/config.rs), and their asserting tests. Rename the
   struct to `RuntimeInputManifest` for consistency.
3. **Keep, do not rename:** `InputPipeline` (the type), `runtime::input::pipeline` (module),
   `pipeline_evaluators`, and the eval CLI `--pipeline-only`. These always read with their
   qualifier and do not collide with the sub-agent's bare `Pipeline`. (An optional later
   `--pipeline-only → --input-only` tidy is deferred — renaming a shipped CLI flag now breaks
   muscle memory for no correctness gain.)

### The gate — this step must not change any current behavior

This is a rename-only diff; treat it as a **hard gate that runs before §3 begins**. Do not
start the sub-agent work until *all* of the following are green:

- **Diff is rename-only.** `git diff` shows only identifier / key / string renames — **zero**
  control-flow or logic edits. A reviewer confirms behavior-preservation by inspection.
- **Build + lint + unit tests.** `cargo build` and `cargo clippy -- -D warnings` clean;
  `cargo test` fully green. The existing `runtime::turn` tests, `handler.rs` tests, and
  `config.rs` runtime-refs test are the regression oracle — they pass unchanged except for the
  new names.
- **Deterministic input-pipeline eval unchanged.** `cargo run --bin eval -- --pipeline-only`
  reproduces the pre-rename result (golden check that the renamed `[runtime.input]` still loads
  and the input chain is byte-for-byte untouched).
- **Endpoint smoke parity.** All four endpoints (`/agent`, `/agent/stream`, `/report`,
  `/report/stream`) return shapes identical to `main` — e.g. via
  [`scripts/staging-smoke.sh`](../../scripts/staging-smoke.sh).

Only when the gate is green is the room clean: the sub-agent modules (§3+) then use bare
`Pipeline` / `Orchestrator` with no qualifier and no ambiguity.

> **Deploy note.** The `[runtime.pipeline] → [runtime.input]` key is a breaking config change.
> It is internal (mounted with the binary, not a wire contract), but any *deployed override*
> that still says `[runtime.pipeline]` fails boot under `deny_unknown_fields`. Ship the config
> change together with the binary.

---

## 3. Land the contracts as code (no behavior change yet)

1. Copy `agent_payload.rs` → `src/agent/payload.rs`. **Remove** the `#[cfg(feature = "openai")]`
   gate (the reference file gates its adapter for standalone compilation; in-crate,
   async-openai is always present). Keep the abstract `LlmCapability` / `Tool` / `run_llm_loop`
   verbatim — these are normative.
2. Copy `sub_agent.rs` → split into `config.rs` / `engine.rs` per §1, replacing
   `mod agent_payload` with `use crate::agent::payload::…`. Use the `sub_agent.rs` `SubAgent`
   trait (with `id()`), not the payload file's older `produces()`-carrying one. The reference
   already carries the Option B output-shape amendment (`SubAgentConfig.output:
   Option<OutputShape>`, `effective_output`, `ResolveError::AmbiguousOutput`) — port it as-is,
   no separate step needed.
3. Bring both reference test suites across (they mock capabilities, so they need no network).
4. Add `pub mod agent;` to [`lib.rs`](../../src/lib.rs).
5. **Exit check:** `cargo test` green, `cargo clippy` clean. Nothing is wired into `AppState`
   yet — this step only adds dormant, tested modules.

## 4. The tool layer: closed `ToolId`, MCP pool, sinks/validators, registry

Implements the [Tool contract](../contract/tool/Contract.md). Replaces auto-discovery with a
closed, boot-resolved set (sub-agent §1.3/§2.2; tool §1–§2).

- **Author the `ToolId` enum.** Run the server once and read the `discovered MCP tools` boot
  log (`main.rs` already logs `names = [...]`) to get the real datacenter tool names, then mint
  one `ToolId` variant per logical tool with its canonical string (parse via `FromStr`, render
  via `Display`, per the reference). The contract's `bill_revenue` /
  `station_revenue_ranking` are placeholders — replace with the actual set.
- **Config surface for N servers, and the `ToolId` ↔ wire-name convention.** Add
  `[[mcp_server]]` entries: `{ id, url, tools = [...] }`. Each `tools` entry is **either** a
  bare string — a `ToolId`, whose wire name defaults to its own canonical string, the common
  case — **or** a table `{ id, mcp_name }` when the server's raw tool name differs from the
  `ToolId` string (e.g. two servers each independently naming a tool `"query"`, which needs two
  distinct `ToolId`s each mapped onto the same raw wire name for its own server):

  ```toml
  [[mcp_server]]
  id = "datacenter"
  url = "http://127.0.0.1:8000/mcp"
  tools = [
    "bill_revenue",                                       # mcp_name defaults to "bill_revenue"
    "station_revenue_ranking",
    { id = "bill_charge", mcp_name = "get_bill_charge" },  # this server's raw name differs
    "member_analysis",
  ]
  ```

  One server stays the common case; the schema simply stops hard-coding *one*
  (`DATACENTER_MCP_URL`). The bare-string shorthand is what most entries use — a per-tool
  `mcp_name` override is the escape hatch, not the default.
- **`McpPool`.** At boot, connect to every configured server (`McpClient::connect_http`), keep
  a `HashMap<McpServerId, McpHandle>` plus each server's handshake `instructions`
  (`McpClient::server_instructions`). **Fail-fast:** any failed handshake aborts boot, naming
  the server.
- **`McpTool`.** `McpTool { handle, mcp_name, target: ArtifactKey, id: ToolId }` implementing
  `Tool`. `mcp_name` comes straight from the parsed `[[mcp_server]].tools` entry above — the
  bare string, or the table's `mcp_name`, defaulting to `id`'s `Display` when the table omits
  it. Its **advertised schema name is the canonical `ToolId` string, not `mcp_name`** — so two
  servers exposing the same raw name never collide within an agent's exposed set. `call()`
  delegates to `McpHandle::call_tool_text`, sending `mcp_name` to its own server.
- **Code-backed sinks & validators via `SchemaTool<T>`.** MCP is one backend; the other is
  code (tool contract §1.1). Port the reference [`SchemaTool<T>`](../contract/tool/tool.rs): a
  generic `Tool` whose advertised schema is `schemars`-derived from a protocol type `T` and
  whose `call` validates by deserializing into `T`, mapping a failure to `ToolOutcome::Rejected`
  (fed back, retried) rather than a crash. `SchemaTool::sink` (identity — the validated `T` *is*
  the artifact, e.g. `emit_chart`) and `SchemaTool::new` (transform, e.g. a `calculate` tool
  that may `Reject` on divide-by-zero) cover the structured-output cases. Adds the `schemars`
  dependency.
- **`ToolOutcome::Rejected` retry-feedback in the loop.** The generalized streaming loop (§8)
  and the buffered `run_llm_loop` must both feed a `Rejected { reason }` back to the model as a
  tool message **without recording an artifact**, and abort only on a fatal `Err` — bounded by
  the step cap. This is the "loop until valid" behavior; the reference already encodes it.
- **Registry population + completeness.** Build one `ToolFactory` per `ToolId` — MCP tools from
  the parsed `(ToolId, mcp_name, server)` triples, sink/validator tools from code. Resolve every
  `SubAgentConfig.tools` grant at boot; abort listing the offending `(sub_agent, ToolId)`. Also
  run the **completeness check** (`assert_complete(ALL_TOOL_IDS)`) so every `ToolId` in the
  closed set has exactly one backend — no gap, no duplicate.
- **Registration ergonomics (auto-register) — decision.** Default to **explicit** registration
  (a designer-owned `build_registry()` with one `register` per tool). A `#[tool]` attribute
  proc-macro (schema derive + `Tool` impl + optional `inventory` link-time collection) is a
  deferred ergonomic add (tool §3/§6); *if* adopted, auto-collection is validated against the
  closed `ToolId` set by the completeness check above, so the fail-fast guarantee survives — a
  link-dropped or mistagged tool fails boot rather than vanishing.
- **Optional drift guard (MCP only).** After discovery, assert every `[[mcp_server]].tools`
  entry's `mcp_name` maps to a name the server actually advertised, and warn on advertised names
  with no `ToolId`. Compare against `mcp_name`, not the `ToolId` string: the server only
  advertises the former. (Sink/validator tools have no server, so this guard skips them.)
- **Exit check:** a unit test resolves a grant spanning two mock servers, rejects an
  unregistered `ToolId`, asserts two same-named raw tools get distinct advertised names, covers
  the `mcp_name` override, and (from the tool reference) covers a `SchemaTool` sink that
  `Rejects` a bad shape then `Produces` on a valid one — with the loop recording the artifact
  only for the valid attempt.

## 5. Per-server MCP instruction routing

Today the single server's `instructions` are appended globally to every prompt
([`appstate.rs::generation_config`](../../src/appstate.rs)). With several servers and agents
granted tool subsets, that is wrong.

- Attach each server's `instructions` to its pool entry.
- When building a `ConfiguredAgent`, compute the **distinct set of servers backing its granted
  tools** and compose *those* instruction blocks (deduplicated) into its system prompt,
  alongside the agent's own `instruction`. A no-tool agent (e.g. the report `finalizer`) gets none.
- Keep the existing "Current Time" + base-prompt assembly (`generation_config`); only the
  instructions source changes from one global block to the per-agent server set. (The
  `# Current Time` block is now emitted by the shared `current_time_header` formatter, §12.1 — the
  sub-agent stages, the legacy path, and the eval runner all call it so the format cannot drift.)
- **Exit check:** an agent granted tools from server A only never sees server B's instructions;
  an agent spanning A+B sees both, once each; the no-tool `finalizer` sees neither.

## 6. TOML raw→resolved loader

- Add raw serde structs to `src/config.rs` mirroring the contract's §3 TOML surface plus the
  `[[mcp_server]]` block, with `#[serde(deny_unknown_fields)]` (matches the existing
  `Manifest`). With §2 done, a top-level `[[pipeline]]` is unambiguous — the runtime's input
  block is now `[runtime.input]`.
- **`[[mcp_server]].tools` is a heterogeneous array** (TOML 1.0 permits mixed element types):
  deserialize each entry via `#[serde(untagged)]` into either a bare `String` (the `ToolId`
  token; wire name defaults to it) or a `{ id: String, mcp_name: Option<String> }` table (§4).
  Resolve to `(ToolId, String)` pairs — `mcp_name.unwrap_or_else(|| id.clone())` — *after*
  parsing `id` into the closed `ToolId` enum, so an unknown `id` token fails exactly like any
  other bad token, not as a separate error class.
- `accepts` / `output` / `provider` / tool names arrive as strings → parse into the closed
  enums (`PayloadKind`, `OutputShape`, `Provider`, `ToolId`) during **resolution**, not
  deserialization (the contract enums intentionally carry no `serde`), reporting the bad token
  on failure. `output` is `Option<String>` in TOML (usually just absent).
- **Ordering constraint: parse every `[[pipeline]]` before resolving any agent's output.**
  `effective_output` needs the full pipeline set to tell terminal from non-terminal, so the
  loader must finish parsing all `[[pipeline]]` blocks before it calls
  `ConfiguredAgent::new` for *any* `[[sub_agent]]` — agent construction now has a dependency on
  pipeline parsing that it didn't have before this amendment.
- Surface `ResolveError::AmbiguousOutput` at boot exactly like `UnknownTool` / `MissingSecret` /
  `MissingModel` / `UnknownAgentRef` — name the offending `SubAgentId` and collect it alongside
  any other resolution errors (§7) rather than aborting on the first one.
- Resolve `instruction = { file = … }` prompt refs via the existing `load_prompt` path
  (relative to the manifest dir) — the report `finalizer` reuses `report_system`, the analytics
  agent reuses `agent_system`.
- **Merge code-defined agents into the same map.** After building `ConfiguredAgent`s from
  `[[sub_agent]]`, insert any hand-written `SubAgent`s (§1, `agents.rs`) into the *same*
  `HashMap<SubAgentId, Arc<dyn SubAgent>>` before `resolve_pipeline`, so a `[[pipeline]]` stage
  can reference a code agent by id exactly like a config one. A collision between a code
  agent's id and a `[[sub_agent]]` id fails boot (two agents claiming one id). Code agents have
  no `[[sub_agent]]` entry — they carry their own prompt/tools/Logic (or none) in Rust.
- Keep `SUPPORTED_VERSION` handling; bump if the schema shape changes materially.
- **Exit check:** load a fixture `config.toml` with the two agents + two pipelines from §10;
  assert the resolved sub-agent orchestrator has both pipelines and any shared agent is reused.
  Add a second fixture reusing one agent at different positions across two pipelines with no
  `output` set, and assert it fails resolution with `AmbiguousOutput`. Add a third covering
  `[[mcp_server]].tools`: a fixture mixing a bare string and a `{ id, mcp_name }` table parses
  both into the same `(ToolId, mcp_name)` shape.

## 7. Boot-time secret validation (contract §2.3)

- Read secrets from the process environment (keep `dotenvy` as today).
- For every resolved provider, bind its `SecretRef` against the environment; a referenced key
  with no entry aborts boot with `secret <KEY> not present in environment`.
- Collect *all* missing secrets before aborting (report the full set) so an operator fixes one
  deploy, not N.
- **Exit check:** boot fails deterministically when `OPENROUTER_API_KEY` is unset for an
  OpenRouter agent; an all-Ollama config boots with no secrets.

## 8. LLM factory + the streaming event architecture

### 8.1 The LLM factory

- Implement `src/agent/llm.rs`: one `OpenAiLlm` per **distinct** `ResolvedLlm` (dedup — many
  agents may share one config; build one client per distinct config, not per agent).
- **Global attribution block.** OpenRouter `HTTP-Referer` / `X-Title` identify the *app*, not a
  per-agent provider — keep them as one app-level block (they already live in
  [`client.rs::build_client`](../../src/llm_connector/client.rs)), not per-`ResolvedLlm`.
- **Exit check (factory):** a live smoke test against one real model per available provider kind;
  a unit test asserting one client is built for two agents sharing a `ResolvedLlm`.

### 8.2 Streaming is an *effect sink*, not an LLM feature

The naïve framings both fail. "Thread a streamer through the sub-agents" breaks the normative
morphism (§8.5). "Streaming is an LLM capability" is directionally right but incomplete — the
pipeline has **three** event sources and the LLM owns only one:

| Event source | Owns | Emits | Examples |
|---|---|---|---|
| LLM turn | `chat()` (streaming adapter) | reasoning/content deltas, tool-call *intent* | `ReasoningDelta`, `ContentDelta`, `ToolCallProposed` |
| Tool execution | the tool (a `StreamingTool` wrapper) | tool started, produced, rejected-retry | `ToolStarted`, `ToolProduced`, `ToolRejected` |
| Pipeline stage | the `Orchestrator` | stage transitions | `StageStarted`, `StageProduced`, `StageFinished` |

"Tool `bill_revenue` returned N rows" and "now the `finalizer` is writing" originate **outside**
`chat()`. So generalize: define **one injected `EventSink` + one tagged `AgentEvent` enum**, and
have each of the three boundaries emit to it. This is not a new idiom — it is exactly the runtime's
existing [`TurnEmit = &dyn Fn(TurnEvent) + Send + Sync`](../../src/runtime/turn.rs) sink ("single
orchestration, two consumption styles — no second frame loop, no drift"), one layer down.

```rust
// src/agent/events.rs (as implemented)
pub enum AgentEvent {
    // pipeline framing — emitted by the Orchestrator, from OUTSIDE run()
    StageStarted  { agent: SubAgentId, input: PayloadKind },   // the kind handed to the stage
    StageProduced { agent: SubAgentId, keys: Vec<ArtifactKey> },// newly-present keys (diffed), sorted
    StageFinished { agent: SubAgentId },
    // llm turn — emitted by the streaming adapter (id known here, from the stream)
    ReasoningDelta { text: String },            // its OWN channel (dropped for now — §8.7)
    ContentDelta   { text: String },
    ToolArgsDelta  { id: String, fragment: String },   // raw json fragment (optional UI sugar)
    ToolCallProposed { id: String, name: String },     // args assembled + parsed
    // tool execution — emitted by the StreamingTool wrapper (NO id: see note)
    ToolStarted  { name: String },
    ToolProduced { name: String, target: ArtifactKey },
    ToolRejected { name: String, reason: String },
    // terminal
    Finished { assistant: String },
    Error    { message: String },               // a DELIVERABLE frame, never a dropped socket
}

pub trait EventSink: Send + Sync { fn emit(&self, ev: AgentEvent); }
pub struct NullSink;                             // buffered path + every unit test
impl EventSink for NullSink { fn emit(&self, _: AgentEvent) {} }
pub struct ChannelSink(pub tokio::sync::mpsc::Sender<AgentEvent>);
impl EventSink for ChannelSink { fn emit(&self, ev: AgentEvent) { let _ = self.0.try_send(ev); } }
```

> **Why the execution events carry no `id`.** The normative `Tool::call(&self, arguments)` does
> **not** thread the model's tool-call id, so the `StreamingTool` wrapper cannot know it — adding it
> would change the tool contract. The id lives on the adapter-emitted `ToolCallProposed` /
> `ToolArgsDelta` (where the stream *does* carry it); a consumer correlates proposed→executed by
> name and order. This kept the tool trait untouched, consistent with Path A's "core signatures
> unchanged" property.

`emit` is **sync fire-and-forget** to match the house `&dyn Fn` sink; `try_send` on a bounded
channel drops on a full buffer and never stalls token generation. If lossless delivery is required,
use an unbounded channel or hand the blocking send to a spawned drainer — a later refinement, not
a launch requirement.

- `AgentEvent` **supersedes the monolith's [`LlmEvent`](../../src/llm_connector/agent.rs)** at the
  sub-agent layer: it keeps `Token`→`ContentDelta`, `ToolCalled`/`ToolResult`→`ToolStarted`/
  `ToolProduced`, `Done`→`Finished`, `Error`→`Error`, and **adds** `ReasoningDelta`, the `Stage*`
  trio, and artifact-keyed tool results. `LlmEvent::Clear` (the "these were thinking tokens, clear
  the answer buffer" hack) exists only because today thinking and answer share one stream; once
  `ReasoningDelta` ships (§8.7) reasoning gets its own channel and `Clear` can be retired.
- **The seam to the runtime (reconciles with §9).** The wire contract stays the runtime's
  [`TurnEvent`](../../src/runtime/turn.rs) (`IntentResolved` / `Token` / `Clear` / `Done` /
  `Error`) → SSE. `PipelineAgentPort` (§9) is the sole translation point: it maps `AgentEvent →
  TurnEvent`, exposing user-facing frames (`ContentDelta→Token`, `Finished→Done`, `Error→Error`)
  and keeping internal ones (`Stage*`, `Tool*`, `ReasoningDelta` until productized) as audit-only,
  exactly as the monolith marks `ToolCalled`/`ToolResult` "must not expose." Richness lives in the
  sub-agent layer; the browser protocol is unchanged.
- **All stages emit *process* events; only the terminal stage streams *content tokens*.** A
  buffered upstream stage (the `fetcher`) still emits `Stage*` + `Tool*` — the user watches data
  being fetched — but its LLM runs the non-streaming `chat`. The terminal stage (`analyst` /
  `finalizer`) additionally streams `ContentDelta`. Buffered-vs-streaming is a **capability choice
  at wiring**, not a fork in the loop; the tool-dispatch isolation guard runs identically in both.

### 8.3 The streaming adapter (the one genuinely new transport)

A decorator over the buffered `Arc<dyn LlmCapability>` **cannot** stream — the inner `chat()`
returns a fully-materialized `LlmResponse`, so a wrapper only ever sees the final value. Token
streaming must happen *at the transport*: a sibling `StreamingOpenAiLlm` that calls
`client.chat().create_stream(...)`, emits deltas as chunks arrive, and returns the **assembled**
`LlmResponse`. From `run_llm_loop`'s view nothing changes — it still receives one complete response.
The buffered `OpenAiLlm` and the `LlmCapability` trait are untouched; streaming is an additive
sibling selected at wiring.

```rust
async fn chat(&self, messages, tools) -> Result<LlmResponse, AgentError> {
    let mut stream = self.client.chat().create_stream(req).await?;   // 0.40 supports this
    let (mut content, mut acc) = (String::new(), BTreeMap::<u32, ToolBuf>::new());
    while let Some(chunk) = stream.next().await {
        let delta = /* chunk?.choices[0].delta */;
        if let Some(t) = delta.content { content.push_str(&t); self.sink.emit(ContentDelta{text:t}); }
        for tc in delta.tool_calls.unwrap_or_default() {          // universal fragment assembly:
            let buf = acc.entry(tc.index).or_default();            //   name-once,
            // set buf.id/buf.name once; buf.args.push_str(fragment); emit ToolArgsDelta          //   args as raw-string deltas,
        }                                                          //   keyed by index,
    }                                                             //   parse at close.
    if acc.is_empty() { Ok(LlmResponse::Message(content)) }
    else { /* serde_json::from_str each buf.args, emit ToolCallProposed, Ok(ToolCalls(..)) */ }
}
```

Tool-call **args are never structured JSON on the wire** — always a partial-JSON *string* you
concatenate per `index` and parse only at the terminal signal (`finish_reason == "tool_calls"`).
Buffer as `String`; do not attempt incremental structural parsing. This is confirmed identical
across every framework surveyed (§8.4) and is what the monolith's `ToolSlot` already does.

### 8.4 Prior art — the converged streaming event taxonomy (recorded)

Five mature agent/LLM streaming protocols were surveyed; they converge on the **same** shape: a
*lifecycle envelope* (`start … finish`) wrapping *typed content channels*, each channel a
`start → delta* → end` triad tagged with an identity, **reasoning as its own channel (never folded
into text)**, tool args as raw-JSON-string fragments assembled by key and parsed at close, and an
explicit terminal event that separates *semantic* failure from *transport* teardown.

| Concept | Vercel AI SDK | OpenAI Responses | Anthropic Messages | LangChain `astream_events` | OpenRouter (chat) |
|---|---|---|---|---|---|
| assistant text | `text-delta` | `response.output_text.delta` | `text_delta` | `on_chat_model_stream` | `delta.content` |
| **reasoning** | `reasoning-delta` | `response.reasoning_text.delta` | `thinking_delta` (+`signature_delta`) | (in chunk) | `delta.reasoning` |
| tool-call args | `tool-input-delta` | `response.function_call_arguments.delta` | `input_json_delta` | `tool_call_chunks` | `delta.tool_calls[].function.arguments` |
| tool result | `tool-output-available` | function-output item | tool_result (next msg) | `on_tool_end` | (next msg) |
| step/block boundary | `start-step`/`finish-step` | `response.output_item.added/done` | `content_block_start/stop` | parent/child `run_id` | — |
| terminal | `finish` + `[DONE]` | `response.completed` + `[DONE]` | `message_stop` | iterator exhaustion | `finish_reason` + `[DONE]` |

Cross-cutting lessons baked into §8.2's `AgentEvent`:
- **Reasoning is a distinct variant**, not text (`ReasoningDelta`), and carries an optional trailing
  signature in the providers that sign it (Anthropic/OpenRouter).
- **Every block carries an identity** so interleaved blocks demux; we key tool args by the delta
  `index` (`ToolBuf` map), mirroring `tool_calls[].index`.
- **Sink injection: nobody threads a param through business logic.** The three real mechanisms are
  *return-a-stream* (Vercel; every provider HTTP body), *inject a writer/handle once at the
  boundary* (Vercel `createUIMessageStream({execute(writer)})`; our `TurnEmit`/`EventSink`), and
  *ambient context propagation* (LangChain flattens a nested graph into one ordered stream via a
  callback handler carried on **contextvars** → an `asyncio.Queue`). Rust analogs: `impl Stream`,
  an `mpsc::Sender`, and `tokio::task_local!` respectively — this is precisely the A-vs-B choice in
  §8.5.
- **Separate semantic failure from transport teardown.** A mid-stream error must be a *deliverable
  frame* (`AgentEvent::Error` → `TurnEvent::Error`), never merely a dropped socket — LangChain's
  exception-only model is the one to avoid for a browser SSE protocol.

Sources: Vercel AI SDK stream protocol (`ai-sdk.dev/docs/ai-sdk-ui/stream-protocol`); OpenAI
Responses & Assistants streaming-events reference (`developers.openai.com/api/reference`);
Anthropic Messages streaming (`platform.claude.com/docs/en/build-with-claude/streaming`);
LangChain `astream_events` (`reference.langchain.com` / `docs.langchain.com/oss/python/langchain/streaming`);
OpenRouter reasoning-tokens & streaming (`openrouter.ai/docs`).

### 8.5 Injecting the per-turn sink into a boot-built pipeline — A vs B

The pipeline is a boot-built, `Arc`-shared graph; the sink is per-turn. The normative morphism
`async fn(AgentPayload) -> Result<AgentPayload, AgentError>` (payload contract) **forbids** adding a
`sink` parameter to `SubAgent::run` — that would break the contract. So the sink cannot flow through
`run()`. **Stage-level** events sidestep this entirely: the `Orchestrator` receives the sink as an
explicit argument (exactly as `run_agent_turn` receives `TurnEmit`) and emits `Stage*` from *outside*
each `stage.run(acc)` call — no morphism change. Only the **inner** token/tool events must reach the
capabilities *without* passing through `run()`. Two mechanisms do that:

**Mechanism A — bake the sink into per-turn capabilities.** The port builds a per-turn terminal
stage whose LLM adapter and tool wrappers carry the turn's sink. The expensive parts (`reqwest`
client, `McpHandle`) are `Arc`-cheap, so this **re-wraps** shared transports, it does not rebuild
clients.

```rust
// inside PipelineAgentPort::stream_turn, per turn:
let sink: Arc<dyn EventSink> = Arc::new(ChannelSink(tx));
let llm  = Arc::new(StreamingOpenAiLlm::new(shared_transport.clone(), sink.clone()));  // holds sink
let tools = grant.iter()
    .map(|t| Box::new(StreamingTool::new(t.clone(), sink.clone())) as Box<dyn Tool>)
    .collect();
let terminal = ConfiguredAgent::new(&cfg, llm, tools, OutputShape::Final);
// SubAgent::run / run_llm_loop / ConfiguredAgent signatures are UNCHANGED — the morphism stays
// run(payload) -> Result<payload>; the effect rides on the injected capabilities.
orchestrator.run_emitting(&pipeline_id, payload, &*sink).await   // orch emits Stage* from outside

// the tool wrapper — delegates schema()/target() so dispatch-by-name is intact:
#[async_trait] impl Tool for StreamingTool {
    fn schema(&self) -> ToolSchema { self.inner.schema() }
    fn target(&self) -> ArtifactKey { self.inner.target() }
    async fn call(&self, args) -> Result<ToolOutcome, AgentError> {
        let name = self.name.clone();          // captured at construction (avoids re-deriving schema)
        self.sink.emit(AgentEvent::ToolStarted { name: name.clone() });
        let out = self.inner.call(args).await;
        match &out {
            Ok(ToolOutcome::Produced(_))     => self.sink.emit(ToolProduced { name, target: self.target }),
            Ok(ToolOutcome::Rejected{reason})=> self.sink.emit(ToolRejected { name, reason: reason.clone() }),
            Err(_) => {}                       // fatal error surfaces via `?` in the loop
        }
        out
    }
}
```

**Mechanism B — ambient sink via `tokio::task_local!`.** The port sets the sink once for the turn,
then runs the **shared, unmodified** boot-built pipeline; the adapter and a tiny `emit()` helper
read the ambient sink; unscoped → no-op. This is the LangChain-contextvars pattern.

```rust
tokio::task_local! { static SINK: Arc<dyn EventSink>; }

// port: set once, run the SHARED Arc graph as-is (no per-turn rebuild):
SINK.scope(Arc::new(ChannelSink(tx)), async move {
    orchestrator.run(&pipeline_id, payload).await
}).await

// adapter + loop call this directly — no sink field, no param, no signature change anywhere:
fn emit(ev: AgentEvent) { let _ = SINK.try_with(|s| s.emit(ev)); }   // Err when unscoped → drop
```

| Property | A — sink on capabilities | B — `task_local` ambient |
|---|---|---|
| Normative morphism `run(payload)->Result` | preserved | preserved |
| `chat` / `run_llm_loop` / `run` signatures | unchanged | unchanged |
| Per-turn work in the port | re-wrap Arc-cheap transports into a per-turn terminal stage | none — one shared pipeline + `SINK.scope` |
| Dataflow visibility | **explicit** (sink is a visible ctor arg) | **implicit** (ambient, contextvars-style) |
| Test emission | pass a `Vec`-collecting sink to the adapter ctor | run the unit inside `SINK.scope(collector, …)` |
| Failure if misused | forget to wrap a tool → *that* tool is silent, visible at the wiring site | forget `.scope`, or emit from a `tokio::spawn`ed sub-task → `try_with` Errs → silently no-op |
| Concurrency across `.await` | sink moves with the value; always correct | propagates within a task, but **spawned sub-tasks need explicit re-`scope`** (real gotcha) |
| House-style fit | matches the contract's capability-injection thesis (agents are pure fns of payload given injected caps) | new mechanism — the codebase uses explicit `TurnEmit`, not `task_local`, today |

**Decision: A.** It preserves *both* the normative morphism and the capability-injection thesis the
whole contract rests on; the per-turn cost is re-wrapping Arc-cheap transports (not rebuilding
clients); and its failure mode is a *visible* omission at the wiring site rather than a silent
ambient drop. Reserve **B** as the escape hatch **iff** per-turn terminal-stage assembly in the port
becomes a real burden (e.g. many stages each needing the sink) — accepting implicit dataflow and the
`spawn`-must-re-`scope` gotcha. Stage-level events are emitted by the `Orchestrator` under an
explicit sink arg in **either** case.

### 8.6 async-openai version — buffered adapter

- Build the buffered + streaming adapter against the **0.40** already in the tree. The contract's
  reference pins **0.41.1**, but a survey (§8.7) confirms **0.40 → 0.41.1 is near-zero blast radius**
  (module paths and every builder type unchanged) **and buys nothing** we need — so the bump stays a
  cosmetic, deferred, isolated step, not a prerequisite for streaming.

### 8.7 Reasoning-token streaming — survey result (decision: dropped for now)

**Decision: reasoning tokens are dropped for the first cut** (`ReasoningDelta` is defined but
unemitted). The survey answers *how* to unlock it later, and rules out the obvious-but-wrong path:

- **A version bump does NOT solve it.** Verified against source: even the latest **0.41.1**'s
  `ChatCompletionStreamResponseDelta` has fields `{ content, function_call(dep), tool_calls, role,
  refusal }` — **no `reasoning` / `reasoning_content`** in any 0.40–0.41.1 version. The struct has no
  `deny_unknown_fields` and no `#[serde(flatten)]` catch-all, so OpenRouter's `delta.reasoning` /
  `delta.reasoning_details[]` are **silently discarded**. `reasoning_effort` exists but is a
  *request* param, not a response field.
- **The real fix (no bump): the `byot` "Bring Your Own Types" feature — already present in 0.40.3.**
  Add `"byot"` to the crate's features (the required `async-openai-macros 0.3.0` is already resolved
  in `Cargo.lock`), then use `client.chat().create_stream_byot::<_, StreamChunk>(req)` with a
  flattened, extended delta to capture the OpenRouter fields, reusing the client's auth / base URL /
  SSE parsing:

  ```rust
  #[derive(Deserialize)]
  struct DeltaExt {
      #[serde(flatten)] base: async_openai::types::chat::ChatCompletionStreamResponseDelta,
      reasoning: Option<String>,                       // OpenRouter extension
      reasoning_details: Option<Vec<ReasoningDetail>>, // structured blocks (optional)
  }
  // + your StreamChunk { choices: Vec<Choice { delta: DeltaExt, finish_reason, .. }>, .. }
  ```

  Flatten works because the base delta derives `Deserialize` without `deny_unknown_fields`. Blast
  radius: one feature flag + one module; no version bump, no other file touched.
- **First-class alternative (larger change): the Responses API.** async-openai's `responses` module
  (present in 0.40.3) models `ResponseReasoningTextDelta` (`"response.reasoning_text.delta"`) etc. as
  first-class stream events — but that targets a `/responses` endpoint, a bigger architectural move
  and dependent on OpenRouter's Responses support. Reserve for if/when we leave chat-completions.

**Sequencing:** ship `ContentDelta` + `Stage*` + `Tool*` on stock 0.40 first; add `ReasoningDelta`
behind the `byot` delta when reasoning UX is wanted (and retire `Clear` at that point — §8.2).

### 8.8 Exit checks (streaming)

- A unit test drives the streaming adapter with a scripted chunk stream (via a fake transport or
  `byot` fixture) and asserts the emitted `AgentEvent` sequence into a `Vec`-collecting sink:
  interleaved `ContentDelta`s, then assembled `ToolCallProposed`, then `Finished`.
- A unit test drives a two-stage pipeline with a collecting sink and asserts `StageStarted(fetcher)`
  … `ToolProduced` … `StageFinished(fetcher)` … `StageStarted(finalizer)` … `ContentDelta`* …
  `Finished` arrive in one correctly-ordered stream (mechanism A: mock caps carry the sink).
- A live smoke test streams one real terminal-stage turn and confirms `ContentDelta`→`Token`→SSE
  parity with `/report/stream` on `main`; a mid-turn induced fault surfaces as a delivered
  `Error` frame, not a dropped connection.

## 9. Bridge the pipeline behind `AgentPort` (the runtime seam)

The sub-agent pipeline becomes the **agent transport** the existing runtime turn already
expects — guardrails, intent, memory, and audit are untouched.

- **`PipelineAgentPort`** (`src/agent/port.rs`) implements
  [`runtime::turn::AgentPort`](../../src/runtime/orchestrator.rs) (the module renamed in §2):
  `stream_turn(input) -> BoxStream<AgentTurnFrame>`. It:
  1. builds the `Initial` payload from `input.prompt` + `input.history`, stamping the boundary
     `now` from a `Clock` the port holds (`InitialPrompt.now = clock.now()`, a `SystemClock` in
     prod — §12.1), so the pipeline itself never reads a clock;
  2. runs the selected pipeline's **upstream** stages buffered (Kleisli `?`; a `Mismatch` or
     `UnknownTool` surfaces as an `AgentTurnFrame::Error`), accumulating artifacts;
  3. runs the **terminal** stage as a stream, translating the sub-agent layer's `AgentEvent`s
     (§8.2) into the runtime's `TurnEvent`s — this port is the **single** `AgentEvent → TurnEvent`
     translation point: user-facing frames (`ContentDelta→Token`, `Finished→Done`, `Error→Error`)
     surface; internal ones (`Stage*`, `Tool*`, `ReasoningDelta`) stay audit-only, exactly as the
     monolith marks `ToolCalled`/`ToolResult` "must not expose." The per-turn `EventSink` reaches
     the terminal stage's capabilities via mechanism A (§8.5).
- **Pipeline selection** is per-route (§10): the handler passes the `PipelineId` into the port.
  `AppState` holds the resolved sub-agent `Orchestrator` (replacing the single `tools` +
  `instructions` fields; `GenerationConfig` assembly moves inside the pipeline).
- **Report now flows through the runtime turn too.** Today `/report` bypasses `run_agent_turn`.
  Routing it through `PipelineAgentPort` brings it under audit — desirable — but also under
  intent/answer-policy, which are tuned for the analytics chat and may wrongly refuse a report
  request. **Mitigation:** allow the report route to run a reduced turn (skip
  intent-refusal/disclaimer, keep audit + input validation), or select a report-appropriate
  answer policy. Tracked as an open item; the analytics `/agent` turn is unchanged.
- **Exit check:** `/agent` returns the same shape (incl. `intent`) as today for the one-stage
  `agent` pipeline; the existing `runtime::turn` tests still pass with `PipelineAgentPort`
  substituted for `FakeAgentPort` in a new integration test.

## 10. The two endpoint pipelines (the conversion)

Both endpoints become named pipelines, selected per-route and run behind `PipelineAgentPort`.
Both also share one `[[mcp_server]]` block — today's single `DATACENTER_MCP_URL`, now named and
enumerated per §4's convention (bare strings; a `mcp_name` override only where a real tool's
raw name diverges from its `ToolId`):

```toml
[[mcp_server]]
id = "datacenter"
url = "http://127.0.0.1:8000/mcp"    # today's DATACENTER_MCP_URL
tools = [
  "bill_revenue", "station_revenue_ranking", "bill_charge", "member_analysis",
  # … the rest of the full data-tool grant, one entry per tool the server actually advertises
  # (§4: read the boot-time `discovered MCP tools` log to get the real names)
]
```

### `agent` — one streaming stage

The analytics chat fetches and answers in a single conversational, streamed turn, so it stays
one stage:

```toml
[[sub_agent]]
id = "analyst"
instruction = { file = "prompt_guide/agent_system.md" }
tools = [ /* the full data-tool grant */ ]
accepts = ["initial"]
# no `output` — "analyst" is terminal in every pipeline that references it (just `agent`
# below), so the shape derives to "final" (contract §2.4)
# inherits [llm.default] (OpenRouter, 4096 max_tokens)

[[pipeline]]
id = "agent"
stages = ["analyst"]
```

`analyst` is the terminal (and only) stage → it streams. Behavior-preserving vs today's
`/agent`.

### `report` — fetch → chart → finalize (three stages)

The report maker splits into three: a data `fetcher`, a `charter` that produces
**schema-enforced chart artifacts** via a `SchemaTool` sink, and a `finalizer` that **combines
all upstream artifacts** (data + charts) into the final HTML. The first two run **buffered**;
the `finalizer` is the **streaming terminal stage** with the raised token ceiling.

```toml
[[sub_agent]]
id = "fetcher"
instruction = { file = "prompt_guide/agent_system.md" }   # or a dedicated fetch prompt
tools = [ /* same data-tool grant */ ]
accepts = ["initial"]
# non-terminal ⇒ output derives to "intermediate"

[[sub_agent]]
id = "charter"
instruction = { file = "prompt_guide/charter_system.md" }
tools = ["emit_chart"]            # a code-backed SchemaTool sink (NOT an MCP tool)
accepts = ["intermediate"]        # reads the fetcher's data artifacts
# non-terminal ⇒ output derives to "intermediate"; the validated chart lands at `charts.spec`

[[sub_agent]]
id = "finalizer"
instruction = { file = "prompt_guide/report_system.md" }
tools = []                        # no tools — it combines, it doesn't fetch or invent
accepts = ["intermediate"]
# terminal ⇒ output derives to "final"
[sub_agent.llm]
max_tokens = 16384                # replaces the REPORT_MAX_TOKENS override

[[pipeline]]
id = "report"
stages = ["fetcher", "charter", "finalizer"]
```

- **`emit_chart` is a code-registered sink**, not an MCP tool — it has no `[[mcp_server]]`
  entry. `ToolId::EmitChart` is backed by a `SchemaTool::<ChartSpec>::sink` in `build_registry()`
  (§4), targeting a new `ArtifactKey::ChartsSpec` (`charts.spec`). The model calls it with a
  chart spec; a bad shape is `Rejected` and fed back until valid — "loop until valid" for free.
- **The `finalizer` is the "combine all stuff" stage** (your decision): its empty grant is the
  isolation guarantee made concrete (it can't fetch or invent), and it consumes the *merged*
  artifact map — the fetcher's data plus the charter's `charts.spec` — emitting one HTML report
  that embeds the validated chart data. This is why chart producers must be **`Intermediate`**:
  `OutputShape::Final` keeps prose and *drops* artifacts (contract §2.4), so a chart can never be
  a terminal stage's output — it must flow as an artifact into the finalizer.
- No agent sets `output`; none is shared across pipelines at different positions, so
  position-derivation resolves all three cleanly (contract §2.4).
- `REPORT_MAX_TOKENS` becomes the `finalizer`'s per-agent `llm.max_tokens`;
  `USER_PROMPT_LENGTH_CAP` stays a cheap HTTP-layer check in `handler.rs`.
- Streaming: `fetcher` + `charter` buffered, `finalizer` streams token-by-token — same wire
  contract as `/report/stream` today.
- **Exit check:** a unit test drives the three-stage pipeline with a mocked LLM + mock data
  tools + the real `SchemaTool` sink; assert the `charter` rejects a malformed chart then
  produces `charts.spec`, and the `finalizer` receives both the fetcher's data and the chart
  artifact and holds zero tools. `/report` yields a `falcon-report` HTML block whose chart data
  is exactly what the sink validated.

## 11. Migration & compatibility

- **Ship §2 first, on its own, behind its gate** — the vocabulary rename is a standalone,
  behavior-preserving commit that precedes all sub-agent code. Nothing downstream starts until
  its gate is green.
- Ship §10's `agent` (one-stage) and `report` (three-stage `fetcher → charter → finalizer`)
  pipelines as the default config.
- Keep the legacy direct path (`should_use_runtime == false` rollback) working until the
  pipeline path is proven in staging, then remove `LlmAgentPort` and the raw `tools` /
  `instructions` `AppState` fields.
- Update `config/config.toml` (including the §2 `[runtime.input]` key), add the `[llm.default]`
  / `[[mcp_server]]` / `[[sub_agent]]` / `[[pipeline]]` blocks, and refresh `README` +
  `docs/reference/endpoints/*` and `docs/reference/modules/runtime-turn.md`.

## 12. Time-awareness, open artifact keys, and message capture (landed in `src/agent/`)

Three design changes went into `src/agent/` while prototyping the pipeline, **ahead of the
`AgentPort` wiring (§9)**. Each is small, additive, and already tested + clippy-clean; this
section is the detailed record. The big-idea rationale — observability, reproducibility,
auditability — lives in the [payload contract](../contract/agent_payload/Contract.md) §2.6 and
the [sub-agent contract](../contract/sub_agent/Contract.md) §1.1/§4; this section is the *how*.

### 12.1 Time as payload data — the injected `Clock`, then time-as-data (option B)

**The bug (a regression, not a new feature).** An LLM has no clock. Handed revenue that ends in
the current, in-progress month, it reads the partial figure as a severe drop and warns about it.
The legacy serving path already fixed this — [`AppState::generation_config`](../../src/appstate.rs)
and the eval runner both prepend a `# Current Time` header — but the sub-agent port dropped that
injection, so this restores parity.

**Two designs were weighed:**
- **A — injected `Clock` capability.** A `Clock` held by `ConfiguredAgent`; each stage reads
  `clock.now()` as it runs.
- **B — time as payload data.** A `Clock` stamps `now` **once at the boundary** into
  `InitialPrompt.now`; it threads through `IntermediateData.now` / `FinalResult.now` unchanged;
  each stage *renders* the carried `now`, never reading a clock itself.

**Decision: B**, for deterministic functional style plus observability / reproducibility /
auditability — one turn has exactly one `now`, a fixture can pin it (byte-reproducible replay),
and the timestamp is visible in every serialized payload rather than hidden in a per-stage
side-read. The `Clock` survives, but only at the **boundary** (production) or in a **fixture**
(tests) — never inside a stage.

**Implementation** ([`clock.rs`](../../src/agent/clock.rs), [`payload.rs`](../../src/agent/payload.rs)):
- `Clock` trait — `fn now(&self) -> DateTime<FixedOffset>`.
- `SystemClock { offset }` — the real clock, `Utc::now().with_timezone(&offset)` with an
  **explicit** default offset of Asia/Taipei `+08:00` (`FixedOffset::east_opt(8*3600)`; Taiwan
  observes no DST, so a fixed offset is exact and needs no tz database). Reading UTC-then-offset
  rather than `Local::now()` keeps the reported day correct even when the container `TZ` is unset
  (containers default to UTC) — the near-midnight mislabel case.
- `FixedClock(DateTime<FixedOffset>)` — pins an instant for unit tests and eval replay.
- `current_time_header<Tz>(now) -> String` — the **one** shared formatter for the
  `"# Current Time\n{YYYY-MM-DD HH:MM:SS ±ZZ:ZZ}\n\n"` block, generic over the timezone so the
  legacy `DateTime<Local>` path (`appstate.rs`, `runtime/eval/runner.rs`) and the sub-agent
  stages all call it and cannot drift. The engine prepends it to the system prompt:
  `format!("{}{}", current_time_header(&now), instruction)`.
- `now: DateTime<FixedOffset>` added to all three payload variants; `chrono`'s `serde` feature is
  enabled so payloads still derive `Serialize`/`Deserialize` (no `Cargo.lock` churn).
- **Boundary stamp.** In production the `PipelineAgentPort` (§9) sets `InitialPrompt.now =
  clock.now()`; the integration tests do it explicitly (`now: SystemClock::default().now()`).

### 12.2 `ArtifactKey`: closed enum → open `{agent}.{name}` string

**Why.** While wiring the finalizer it became clear an `Intermediate → Final` reshape *drops* an
agent's message (only tool artifacts were keyed), and that the artifact contract should hold
**anything castable to a string** — a tool result, an LLM message, or any computed value. The
value side (`ArtifactValue`) was already string-castable via `Display`; the blocker was the
*key* side — a **closed compile-time enum** that could not name "the analyst's message" without a
contract edit per new wire.

**Change** ([`payload.rs`](../../src/agent/payload.rs)) — `ArtifactKey` becomes an open struct
`{ agent: String, name: String }` rendering `{agent}.{name}`:
- `Display` writes `"{agent}.{name}"`; `FromStr` splits on the **first** dot (the producer
  namespace has no dot; the slot name may), rejecting a dotless string; `serde(into/try_from =
  "String")` makes it a transparent JSON string key (JSON object keys must be strings).
- Named constructors keep the well-known wires in one place: `fetcher_records()`,
  `fetcher_schema()`, `charts_spec()`, and `message(agent)` (`{agent}.message`).
- **Costs, accepted deliberately.** `ArtifactKey` loses `Copy` (it owns `String`s) — call sites
  that relied on copy now `.clone()`/`.cloned()`, and `render_material` sorts by `Ord` — and a
  mistyped key is now a **runtime** key rather than a compile error. The named constructors
  contain the typo blast radius; **`ToolId` stays a closed enum**, so the *tool set* remains
  typo-checked even though the *artifact key space* opened.

### 12.3 Message auto-capture, controlled per stage

**The asymmetry it fixes.** With open keys, a stage's message can be a first-class artifact
(`{id}.message`). Capturing *every* stage's message unconditionally, though, floods the map with
throwaway notes (the fetcher's "已取得營收", the charter's "已產生圖表"). So capture is a
**per-stage control**, default-on:
- `SubAgentConfig.capture_message: bool` ([`config.rs`](../../src/agent/config.rs)) — documented
  **default on**; set `false` for a tool-only stage whose message is throwaway.
- `ConfiguredAgent` reads it and guards the insert
  ([`engine.rs`](../../src/agent/engine.rs)): after `run_llm_loop`, it merges tool artifacts
  append-only, then `if self.capture_message { artifacts.insert(ArtifactKey::message(&id),
  Text(text)) }`.
- **`FinalResult` now carries `artifacts`** too (it previously held only `user`/`assistant`), so
  a terminal stage keeps the full provenance map instead of dropping it. `FinalResult` drops
  `Eq` (an `f64` lives under `ArtifactValue::Number`).
- **Pipeline wiring** ([`pipeline.rs`](../../src/agent/pipeline.rs)) — the `/agent` pipeline
  (`fetcher → analyst → charter → finalizer`) sets `analyst` → `true` (its message *is* the
  report the pure-logic `finalizer` reads) and `fetcher` / `charter` → `false` (throwaway notes).
  The composed test asserts the *curated* provenance directly: `analyst.message` +
  `fetcher.records` + `charts.spec` present; `fetcher.message` / `charter.message` absent.

### 12.4 How this amends the earlier sections

- **§3 (land contracts as code)** — the ported `payload.rs` carries the open `ArtifactKey`, the
  `now` fields, and `FinalResult.artifacts`; `config.rs` carries `capture_message`. The reference
  `.spec/contract/*/*.rs` predate these; the live `src/agent/` modules are authoritative.
- **§6 (TOML loader)** — a missing `capture_message` maps to the documented default (`true`); add
  it to the raw `[[sub_agent]]` serde struct with a `#[serde(default = …true…)]`.
- **§9 (AgentPort wiring)** — the port is the boundary that stamps `InitialPrompt.now =
  clock.now()` (step 1); it holds the `Clock`, so the pipeline never reads one.
- **§10 (pipelines)** — each `[[sub_agent]]` gains an authored `capture_message` (prose stages
  on, tool-only stages off).

---

## Deferred beyond this plan

- **async-openai 0.40 → 0.41.1** — cosmetic bump only. §8.7 confirms it is near-zero blast radius
  (module paths + all builder types unchanged) **and buys nothing** — the streaming delta still
  lacks a `reasoning` field. Not a prerequisite for anything; do it in an isolated commit whenever
  convenient (or never). Reasoning streaming is unlocked by the **`byot`** feature, not this bump.
- **`#[tool]` proc-macro + `inventory` auto-registration** — ergonomic tool definition
  (schema derive + `Tool` impl + link-time collection), reconciled with the closed set by the
  boot completeness check (tool contract §3/§6). Explicit registration ships first.
- **Eval CLI tidy** `--pipeline-only → --input-only` — optional, cosmetic; deferred so §2 does
  not break a shipped flag.
- **Report-route guardrail tuning** — the reduced-turn / report-answer-policy decision (§9).
- **Namespace enforcement** (contract §2.5) — post-run validator that an agent wrote only keys
  under its own `id`. Cheap; add when a real collision risk appears.
- **Pipeline routing beyond per-route** — header/path/LLM-classifier selection once more than
  the two fixed pipelines exist.
- **Multi-stage *content* streaming** — every stage already emits **process** events (`Stage*` /
  `Tool*`) from the first cut (§8.2); what stays deferred is a *non-terminal* stage emitting
  user-visible **content tokens** (`ContentDelta` surfaced as `Token`). Terminal-only content
  streaming first.
- **Reasoning-token streaming** — `ReasoningDelta` is defined but unemitted; unlock via the `byot`
  extended delta (§8.7), and retire `LlmEvent::Clear` once reasoning has its own channel.
- **`AgentError::Capability` taxonomy** (payload §6) — retryable vs fatal, so the runtime can
  route around a failed stage.
- **Caller-requested intermediate projection** — a request-time knob (e.g. a query param or
  header) letting a caller ask for the pipeline's pre-terminal `Intermediate` payload instead
  of the terminal stage's `Final` one. This is where "return the fetcher's raw artifacts, not
  the finalizer's prose" now lives, now that `output` is derived from *structural* position rather
  than authored per request — it is a caller-side projection, not a per-agent config choice.

---

## Risk notes

- **The §2 rename is behavior-preserving — prove it, don't assume it.** It touches a shared
  module path and a config key; a missed *code* site fails the build (good), but a missed
  *config* migration (`[runtime.pipeline]` in a deployed override) fails at boot. The §2 gate
  (rename-only diff + green tests + `--pipeline-only` golden + endpoint smoke) is mandatory, and
  the config key ships with the binary. After §2, the "two orchestrator/pipeline concepts"
  confusion is gone by construction: bare terms are the sub-agent's, the runtime speaks of
  turns/input.
- **Report goes under guardrails for the first time** (§9). Verify intent/answer-policy don't
  refuse legitimate report requests before cutting `/report` over.
- **Closed `ToolId` vs live server drift** — a server that renames a tool breaks a grant at
  boot (intended fail-fast), but only if the drift guard (§4) is wired; otherwise it fails at
  first call. Wire the guard.
- **`ResolvedLlm` deduplication** — build one client per distinct config, not per agent, to
  avoid connection sprawl (§8).
- **Streaming/​buffered split** — the terminal stage reuses the 0.40 streaming loop while
  upstream stages use the abstract loop; keep the tool-dispatch guard in **both** so isolation
  never depends on which shape ran.
- **`output` derivation adds a load-order dependency** (§6) — every `[[pipeline]]` must be
  parsed before any `[[sub_agent]]` is resolved into a `ConfiguredAgent`, where previously
  agent resolution had no dependency on pipelines at all. Get this backwards and
  `effective_output` sees an incomplete pipeline set, silently picking a wrong default instead
  of correctly failing `AmbiguousOutput` — verified in this plan by compiling and testing the
  amended reference (`sub_agent.rs`) standalone before porting it (§3), and by the second
  loader fixture in §6's exit check.
