# SubAgent — Implementation Plan

Turns the [SubAgent contract](../contract/sub_agent/Contract.md) and the
[AgentPayload contract](../contract/agent_payload/Contract.md) into working code inside
`datacenter-agent`. The contracts fix the *what* (payload sum type + config model +
resolution/composition rules); this plan covers the *how*, the items the contracts deferred
(sub-agent §6, payload §6), and the concrete migration of **today's monolithic endpoints into
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
| `/report` shape | **Two-stage fetch→write** (`fetcher` → `writer`), the contract's canonical example. |
| Sub-agent orchestrator ↔ runtime | **Pipeline sits *behind* `AgentPort`** — a `PipelineAgentPort` replaces `LlmAgentPort`; guardrails/intent/memory/audit are preserved. |
| Tool set | **Closed, hand-authored `ToolId` enum** + registry, resolved at boot (contract §1.3, §2.2). Auto-discovery is replaced. Each grant additionally binds an explicit wire name (`mcp_name`) when it differs from the canonical `ToolId` string (§4). |
| Streaming | Terminal stage streams (reuse the existing loop, narrowed to the agent's tool grant); upstream stages run buffered via the abstract non-streaming loop. |
| async-openai | Build the capability adapter against the **0.40** already in the tree; treat the contract's 0.41.1 pin as a separate, deferred bump (see §8). |
| `output` default | **Position-derived when unset**, per the amended contract (§1.1/§2.4/§4 of `Contract.md`): a stage's `Final` vs `Intermediate` shape is computed from whether it is terminal in every pipeline that references it. Ambiguous position (terminal in one pipeline, not in another) fails resolution and demands an explicit value. This removed a real footgun — `output` and pipeline position could previously disagree — and, for our two pipelines below, means **no agent needs to set `output` at all**. |

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

## 4. Closed `ToolId` enum, multi-server MCP pool + tool registry

Replaces auto-discovery with a closed, boot-resolved set (contract §1.3, §2.2).

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
- **Registry population.** Build one `ToolFactory` per parsed `(ToolId, mcp_name, server)`
  triple, each capturing the correct server's handle. Resolve every `SubAgentConfig.tools`
  grant at boot; abort with a clear error listing the offending `(sub_agent, ToolId)`.
- **Optional drift guard.** After discovery, assert every `[[mcp_server]].tools` entry's
  `mcp_name` maps to a name the server actually advertised, and warn on advertised names with
  no `ToolId` — so the closed set never silently diverges from the live server. Compare against
  `mcp_name` here, not the `ToolId` string: the server only ever advertises the former.
- **Exit check:** a unit test resolves a grant spanning two mock servers, rejects an
  unregistered `ToolId`, asserts two same-named raw tools get distinct advertised names, and
  covers the `mcp_name` override — a `ToolId` configured with an explicit `mcp_name` dispatches
  *that* name to the mock server while the LLM-visible schema name stays the canonical `ToolId`
  string.

## 5. Per-server MCP instruction routing

Today the single server's `instructions` are appended globally to every prompt
([`appstate.rs::generation_config`](../../src/appstate.rs)). With several servers and agents
granted tool subsets, that is wrong.

- Attach each server's `instructions` to its pool entry.
- When building a `ConfiguredAgent`, compute the **distinct set of servers backing its granted
  tools** and compose *those* instruction blocks (deduplicated) into its system prompt,
  alongside the agent's own `instruction`. A no-tool agent (e.g. the report `writer`) gets none.
- Keep the existing "Current Time" + base-prompt assembly (`generation_config`); only the
  instructions source changes from one global block to the per-agent server set.
- **Exit check:** an agent granted tools from server A only never sees server B's instructions;
  an agent spanning A+B sees both, once each; the no-tool `writer` sees neither.

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
  (relative to the manifest dir) — the report `writer` reuses `report_system`, the analytics
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

## 8. LLM factory + streaming reconciliation

- Implement `src/agent/llm.rs`: one `OpenAiLlm` per **distinct** `ResolvedLlm` (dedup — many
  agents may share one config; build one client per distinct config, not per agent).
- **Two execution shapes, one isolation boundary:**
  - **Buffered stages** (all non-terminal, e.g. report `fetcher`) use the contract's abstract,
    non-streaming `run_llm_loop` over `LlmCapability::chat` — clean and unit-testable.
  - **Terminal stage** (the user-visible one) must stream tokens. Reuse the existing streaming
    loop in [`llm_connector/agent.rs`](../../src/llm_connector/agent.rs), **generalized** to
    take the terminal agent's *resolved tool subset* (`Arc<Vec<ChatCompletionTool>>` narrowed to
    its grant) + its `ResolvedLlm` params, instead of the global tool list. The dispatch guard
    still rejects out-of-grant calls, so isolation holds in both shapes.
- **async-openai version.** The contract's reference adapter pins **0.41.1**; the live crate is
  on **0.40** and the streaming loop we are reusing is 0.40. Build the adapter against **0.40**
  to avoid a crate-wide churn during migration. The 0.41.1 bump (which also touches
  `model.rs`, `appstate.rs`, and the existing loop) is a **separate, isolated, deferred step** —
  the payload/behavioral rules are what bind; the adapter version is advisory (payload §0, §3.7).
- **Global attribution block.** OpenRouter `HTTP-Referer` / `X-Title` identify the *app*, not a
  per-agent provider — keep them as one app-level block (they already live in
  [`client.rs::build_client`](../../src/llm_connector/client.rs)), not per-`ResolvedLlm`.
- **Exit check:** a live smoke test against one real model per available provider kind; a unit
  test asserting one client is built for two agents sharing a `ResolvedLlm`.

## 9. Bridge the pipeline behind `AgentPort` (the runtime seam)

The sub-agent pipeline becomes the **agent transport** the existing runtime turn already
expects — guardrails, intent, memory, and audit are untouched.

- **`PipelineAgentPort`** (`src/agent/port.rs`) implements
  [`runtime::turn::AgentPort`](../../src/runtime/orchestrator.rs) (the module renamed in §2):
  `stream_turn(input) -> BoxStream<AgentTurnFrame>`. It:
  1. builds the `Initial` payload from `input.prompt` + `input.history`;
  2. runs the selected pipeline's **upstream** stages buffered (Kleisli `?`; a `Mismatch` or
     `UnknownTool` surfaces as an `AgentTurnFrame::Error`), accumulating artifacts;
  3. runs the **terminal** stage as a stream, mapping its `LlmEvent`s to `AgentTurnFrame`
     (`Token` / `Clear` / `ToolCalled` / `ToolResult` / `Done` / `Error`) exactly as
     `LlmAgentPort` does today.
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

### `report` — two-stage fetch→write

The report maker splits into a data `fetcher` (holds the data tools, emits `Intermediate`
artifacts, runs **buffered**) and a `writer` (no tools — it *cannot* fetch or invent — consumes
the artifacts, emits the final HTML, runs as the **streaming terminal stage** with the raised
token ceiling):

```toml
[[sub_agent]]
id = "fetcher"
instruction = { file = "prompt_guide/agent_system.md" }   # or a dedicated fetch prompt
tools = [ /* same data-tool grant */ ]
accepts = ["initial"]
# no `output` — "fetcher" is non-terminal in the only pipeline that references it (`report`
# below), so the shape derives to "intermediate"

[[sub_agent]]
id = "writer"
instruction = { file = "prompt_guide/report_system.md" }
tools = []                        # no-tool agent → isolation boundary is empty
accepts = ["intermediate"]
# no `output` — "writer" is terminal in the only pipeline that references it, so the shape
# derives to "final"
[sub_agent.llm]
max_tokens = 16384                # replaces the REPORT_MAX_TOKENS override

[[pipeline]]
id = "report"
stages = ["fetcher", "writer"]
```

- Neither example above sets `output`: with only two pipelines and no agent shared between
  them at different positions, position-derivation (contract §2.4) resolves both cleanly. The
  contract's *own* canonical example — the same `fetcher` reused terminally in a
  `quick_fetch = ["fetcher"]` pipeline — is exactly the case that would force `output =
  "intermediate"` back onto `fetcher` explicitly; we don't have that pipeline, so we don't pay
  for it.
- The `writer`'s empty grant is the contract's isolation guarantee made concrete: its LLM has
  no fetch tool, so it writes only from the `fetcher`'s artifacts (§2.3).
- `REPORT_MAX_TOKENS` (handler const) becomes the `writer`'s per-agent `llm.max_tokens`;
  `USER_PROMPT_LENGTH_CAP` stays a cheap HTTP-layer check in `handler.rs`.
- Streaming: `fetcher` buffered, `writer` streams token-by-token — same wire contract as
  `/report/stream` today.
- **Exit check:** `/report` yields a `falcon-report` HTML block whose content references only
  artifacts the `fetcher` produced; a unit test drives the two-stage pipeline with a mocked LLM
  + mock tools and asserts the `writer` receives the `fetcher`'s artifacts and holds zero tools.

## 11. Migration & compatibility

- **Ship §2 first, on its own, behind its gate** — the vocabulary rename is a standalone,
  behavior-preserving commit that precedes all sub-agent code. Nothing downstream starts until
  its gate is green.
- Ship §10's `agent` (one-stage) and `report` (two-stage) pipelines as the default config.
- Keep the legacy direct path (`should_use_runtime == false` rollback) working until the
  pipeline path is proven in staging, then remove `LlmAgentPort` and the raw `tools` /
  `instructions` `AppState` fields.
- Update `config/config.toml` (including the §2 `[runtime.input]` key), add the `[llm.default]`
  / `[[mcp_server]]` / `[[sub_agent]]` / `[[pipeline]]` blocks, and refresh `README` +
  `docs/reference/endpoints/*` and `docs/reference/modules/runtime-turn.md`.

---

## Deferred beyond this plan

- **async-openai 0.40 → 0.41.1** — crate-wide bump; do it in an isolated commit after the
  pipeline path is stable (§8).
- **Eval CLI tidy** `--pipeline-only → --input-only` — optional, cosmetic; deferred so §2 does
  not break a shipped flag.
- **Report-route guardrail tuning** — the reduced-turn / report-answer-policy decision (§9).
- **Namespace enforcement** (contract §2.5) — post-run validator that an agent wrote only keys
  under its own `id`. Cheap; add when a real collision risk appears.
- **Pipeline routing beyond per-route** — header/path/LLM-classifier selection once more than
  the two fixed pipelines exist.
- **Multi-stage streaming** — token semantics if a non-terminal stage should also emit
  user-visible tokens. Terminal-only streaming first.
- **`AgentError::Capability` taxonomy** (payload §6) — retryable vs fatal, so the runtime can
  route around a failed stage.
- **Caller-requested intermediate projection** — a request-time knob (e.g. a query param or
  header) letting a caller ask for the pipeline's pre-terminal `Intermediate` payload instead
  of the terminal stage's `Final` one. This is where "return the fetcher's raw artifacts, not
  the writer's prose" now lives, now that `output` is derived from *structural* position rather
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
