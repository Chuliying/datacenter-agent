# AgentPayload Contract

The contract governing values that flow between LLM sub-agents. The reference
implementation is [`agent_payload.rs`](./agent_payload.rs), which compiles, is
clippy-clean, and whose async test suite encodes the rules below. Its async-openai adapter
compiles against async-openai **0.41.1** behind `--features openai`.

> **Amendment — time-as-data · open artifact keys · lossless provenance.** §1/§2/§4 below carry
> three changes made during the port, all in service of **observability, reproducibility, and
> auditability**: (a) every payload variant carries a turn timestamp `now`, stamped once at the
> boundary and threaded as data (§1, §2.6); (b) `ArtifactKey` is an **open `{agent}.{name}`
> string** rather than a closed compile-time enum (§1, §4); and (c) `FinalResult` carries the
> full `artifacts` map, so a stage's message and every upstream product survive the terminal
> boundary rather than being dropped (§2.6). The authoritative implementation of these three is
> the live port under [`src/agent/`](../../../src/agent/) (`payload.rs`, `clock.rs`); the
> reference [`agent_payload.rs`](./agent_payload.rs) in this folder predates them. The **detailed
> design** (the `Clock`, the A→B decision, the open-key tradeoffs, the capture control) lives in
> the [implementation plan](../../plan/sub_agent.md) §12.

---

## 0. What this contract binds

This is the load-bearing distinction, so it comes first.

**Normative (binding).** The `AgentPayload` data model (§1) and the behavioral rules on
agents (§2). Anything that consumes or produces payloads MUST speak exactly these types and
obey these behaviors, or it is not part of this system. The payload is the interface.

**Advisory (a suggestion, not an enforcement).** *How* you build an agent (§3): the
`SubAgent` trait, the LLM/tool capabilities, the tool-use loop, the dependency-inversion
adapter for async-openai. This is one Rust encoding that satisfies §1–§2. Replace any of it
— a different trait shape, a different DI mechanism, no trait at all — as long as the
payload and the behavioral rules still hold. The compiler enforces the *payload*; it does
not, and is not asked to, enforce that you followed the recommended agent design.

---

## 1. Data model (normative)

### `AgentPayload` — the sum type

The single value every agent consumes and produces. One of three variants:

| Variant | Payload | Purpose |
|---|---|---|
| `Initial` | `InitialPrompt` | Pipeline entry — the user's request. |
| `Intermediate` | `IntermediateData` | Working data between agents; **never** the user-facing output. |
| `Final` | `FinalResult` | The user-facing result. |

A cheap tag, `PayloadKind`, mirrors the three cases so acceptance checks and error values
can reason about a payload without carrying its whole body.

### The variant bodies

Every variant additionally carries a **turn timestamp** `now: DateTime<FixedOffset>` (see
*Supporting types* below).

- **`InitialPrompt`** — `prompt: String`, `history: Vec<Exchange>`, and `now`. There is no
  system prompt: each agent carries its own designed instruction. History is a plain `Vec` — a
  `Vec` already models "empty", so no `Option`.
- **`IntermediateData`** — `prompt: String` (the instruction, carried forward), `artifacts:
  HashMap<ArtifactKey, ArtifactValue>` (the KV surface wired between agents), and `now`.
- **`FinalResult`** — `user: String`, `assistant: String` (the terminal *projection* the user
  sees), `now`, **and `artifacts: HashMap<ArtifactKey, ArtifactValue>`** — the full accumulated
  provenance, carried *through* the terminal boundary rather than dropped (§2.6). Its **own
  type** (not `Exchange`) so metadata (stop reason, token usage, latency) can accrete here
  later. It derives `PartialEq` but **not `Eq`** (an `ArtifactValue::Number` holds an `f64`).

### Supporting types

- **`Exchange`** — one `user`/`assistant` turn inside history. Structurally `FinalResult`
  minus metadata, kept **distinct on purpose**: history is plain chat; `FinalResult` is an
  extension point.
- **`ArtifactValue`** — a **closed enum** (`Text`, `Json`, `Number`, …). `Display` is the
  "cast to string if you don't care about the type" view; a `match` is the checked
  "downcast". Being closed is what lets the whole payload derive `Clone` + `Serialize`, and it
  is already what makes an artifact "anything castable to a string" (§4).
- **`ArtifactKey`** — an **open, producer-namespaced string key**, `{agent}.{name}` (e.g.
  `fetcher.records`, `analyst.message`). Any agent freely names its outputs — there is no
  closed set to extend — so a tool result, an agent's own message, or any computed value are
  keyed uniformly. The `agent` half is the producer namespace (§2.5); the canonical dotted
  string is *both* the log form and the serialized form (JSON object keys must be strings), via
  `Display` / `FromStr` (split on the *first* dot) and a `serde(into/try_from = "String")`
  transparent representation. **This trades the earlier compile-time-enum guarantee** — a
  mistyped key is now a *runtime* key, not a compile error — for an open key space; the named
  constructors (`fetcher_records()`, `charts_spec()`, `report_data()`, `message(agent)`) keep the
  well-known wires in one place so a typo has a single blast radius. `report.data` is the newest
  such wire: a serialized `ReportData` from `/report`'s `emit_report` sink, the single artifact the
  pure-logic `renderer` injects into its HTML template — one more producer-namespaced key, no schema
  change needed for the open space to absorb it.
- **The turn timestamp `now`** — a `DateTime<FixedOffset>` stamped **once at the boundary** and
  threaded through every stage unchanged. It is *data*, not an ambient read: this is what makes
  a run reproducible (a fixture can pin it for byte-identical replay), observable (it shows up in
  every serialized payload), and auditable end to end (§2.6). Each LLM stage *renders* it into a
  `# Current Time` header so the model can tell an in-progress trailing period from a genuine
  drop. The mechanism (the `Clock` at the boundary, the shared formatter) is plan §12.1.

---

## 2. Behavioral rules (normative)

These bind every agent regardless of how it is implemented.

### 2.1 An agent is a fallible morphism (a Kleisli arrow)

Every agent behaves as `async fn(AgentPayload) -> Result<AgentPayload, AgentError>`. Because
it is fallible it is an arrow in the Kleisli category of `Result`: **identity** returns its
input unchanged; **composition** threads the error, so the first failure short-circuits the
rest of the pipeline. Orchestrators chain agents with `?`, not bare function composition.

### 2.2 The falling convention (type matching)

An agent handed a variant it does not accept — an intermediate agent given an `Initial`
prompt, or a `Final` arriving as input — MUST return `AgentError::Mismatch { expected, got }`,
a typed, routable value. **It never panics.**

### 2.3 Isolation is bounded by an agent's granted tools

Every agent talks to an LLM (§3.1). What differs, and what isolates one agent from another,
is the **set of tools its LLM may call**. An agent must expose to its LLM *only* the tools
it was granted, and must reject — not silently ignore — a call to any tool outside that set
(`AgentError::UnknownTool`). A report-writer granted no data tools therefore cannot fetch:
there is nothing for its LLM to call.

Two honest limits on this guarantee:

- **Behavioral, not token-level.** "Write only from provided material, don't invent the
  rest" is a claim about the tokens an LLM emits, and **no type constrains that**. The
  system's contribution is indirect: an agent's context is assembled only from its granted
  artifacts (less to invent from), and because every input is a named `ArtifactKey`, a
  validator can check afterward that the output references only keys that were present.
- **Construction-time, not compile-time (as implemented).** Because the reference tool set
  is a runtime collection, "the writer can't fetch" is enforced at *construction* (its set
  omits the tool) plus the dispatch guard above — not by the type system. See §3.5 for how
  to lift this back to a compile-time guarantee if a given deployment needs it.

### 2.4 Agents produce; they do not mutate

An agent returns a *new* payload rather than editing an upstream one. Artifacts are
effectively **append-only** across a pipeline: a downstream stage adds to the map it
received; it never rewrites an earlier stage's entries. This is what makes replay-debugging
and fan-out (one input through two branches) safe — no shared mutable state.

### 2.5 Artifact keys are namespaced by producer

Each `ArtifactKey` carries its producing agent and renders to a greppable dotted form. Two
agents cannot collide on a key, and every log line and serialized payload attributes each
entry to a producer. **Only the orchestration designer mints keys** — the LLM never invents
one, because it has no capability to choose a key name. (Opening the key type to a
`{agent}.{name}` string, §1, kept this rule: the string is minted in code by the designer; the
LLM still names nothing.)

### 2.6 The pipeline is auditable: nothing load-bearing is silently dropped

Two properties make a run reconstructable after the fact — the **observability, reproducibility,
and auditability** the payload exists to support:

- **Provenance is lossless across the terminal boundary.** `FinalResult` carries the full
  `artifacts` map, not just the user-facing `assistant` string. An agent's own **message** may
  be captured as a first-class artifact keyed `{agent}.message` (the open key space of §1 is
  what lets prose sit beside tool results), so a stage whose product *is* prose — an analyst's
  report feeding a downstream finalizer — is preserved rather than lost when its `Intermediate`
  is reshaped, and the terminal result records what every stage contributed. *Whether* a given
  stage's message is captured is a producer-side control, not a payload rule (it lives in the
  [sub-agent contract](../sub_agent/Contract.md) §1.1); the payload rule is only that a captured
  message and the accumulated artifacts **survive to `Final`** rather than being dropped at the
  boundary.
- **Time is data, stamped once.** `now` is fixed at the boundary and threaded unchanged, so
  every stage agrees on one instant, an audit record of the response carries when the turn
  occurred, and a replay with a pinned clock is byte-reproducible. A stage **never reads an
  ambient clock** — that would make its output depend on hidden state and break the "pure
  function of its payload" property (§3.6).

---

## 3. Recommended implementation (advisory)

One Rust encoding satisfying §1–§2. Swap freely.

### 3.1 LLM communication is common to every agent

A data-fetcher is *itself* an LLM that decides which tool to call; a report-writer is an LLM
that writes prose. So "talk to an LLM" (`LlmCapability`) is a capability **shared by all
agents**, not a mark of one agent type. This corrects an earlier model in which isolation
came from *which capability an agent held* (fetch vs. LLM) — that model doesn't survive the
fact that the fetcher is an LLM too.

The capability is `async`, shaped to fit **async-openai**. It is expressed abstractly
(`chat(messages, tools) -> Message | ToolCalls`) so the domain stays SDK-agnostic; the
concrete async-openai implementation lives behind an adapter (dependency inversion), which
is why the core compiles and unit-tests without the SDK.

### 3.2 Tools, and who owns what

A `Tool` advertises a schema to the LLM and declares a `target: ArtifactKey` — the slot its
result fills. The **LLM decides which tool to call** at run time; the **designer decides the
key names** via `target`. The LLM never names a key or invents a tool. `Tool::call` returns a
`ToolOutcome` — `Produced(value)` or `Rejected { reason }` (a retryable, model-facing outcome,
distinct from a fatal `Err(AgentError)`). The full tool story — the fetch/sink/validator
taxonomy, `ToolId` naming/resolution, and the `SchemaTool` adapter — lives in the
[Tool contract](../tool/Contract.md).

### 3.3 The tool-use loop

`run_llm_loop` sends the agent's tool schemas to the LLM, dispatches each requested call to
the matching `Tool`, feeds results back, and repeats until the LLM returns a final message. On
`Produced`, the result is recorded in the artifact map keyed by the tool's `target` and fed
back; on **`Rejected { reason }`**, *no artifact is recorded* and the reason is fed back so the
model can correct and retry (a validating tool thus "loops until valid" for free); a fatal
`Err` aborts. A call to a tool the agent does not own is rejected here — the isolation boundary
of §2.3, guarded at dispatch. A step cap bounds retries and prevents a non-terminating loop.

### 3.4 Agents differ only by their tool set

`DataFetcher` holds data-access tools; `ReportWriter` holds none. Both share the same
`LlmCapability`. This is the tool-set isolation of §2.3 made concrete: the writer is
constructed without the fetch tool, so its LLM has nothing to fetch with.

### 3.5 Optional: compile-time tool isolation

To turn §2.3's construction-time guarantee into a compile-time one, make the tool set a
typed capability (a generic parameter) rather than a runtime `Vec`, so an agent's *type*
encodes its permitted tools and it is a compile error to hand it a tool outside that set.
This is a real complexity jump (every agent role becomes a distinct type parameterization);
adopt it only where the stricter guarantee is worth it.

### 3.6 Testability and wiring

Because capabilities are injected, an agent supplied with mocked capabilities is a **pure
async function of its input** — a genuine unit test, not an integration test (see the test
module). Separately, each agent declares the `PayloadKind`s it `accepts`/`produces`, so a
pipeline is validated at **graph-construction time**: a mis-ordered pipeline is caught before
a single LLM call is made.

### 3.7 The async-openai adapter

`OpenAiLlm` implements `LlmCapability` by translating the abstract message/tool protocol to
and from async-openai's chat-completions types, confined to one feature-gated module. It
targets async-openai **0.41.1** (chat types under `types::chat`; tools and tool-calls are the
`ChatCompletionTools` / `ChatCompletionMessageToolCalls` enums). Pin the version: this API
surface changes across releases.

---

## 4. Locked decisions

| Question | Decision | Rationale |
|---|---|---|
| What the contract binds | **The payload + behavioral rules**; agent code is advisory | The compiler enforces the shared value; implementation is free to vary. |
| Artifact value representation | **Closed enum** with `Display` | Retains the typed value *and* keeps the payload `Clone` + `Serialize`; `Display` makes it string-castable. |
| Artifact key type | **Open `{agent}.{name}` string key** (was a compile-time enum) | An open key space lets any produced value — tool result, agent message, computed value — be keyed uniformly; named constructors localize the well-known wires. Trades compile-time typo detection for openness. |
| Turn time | **`now: DateTime<FixedOffset>` on every variant**, stamped once at the boundary, threaded as data | Deterministic, observable, reproducible, auditable — vs. an ambient clock read inside a stage. |
| Final provenance | **`FinalResult` carries the full `artifacts` map** (+ a stage's message may be captured as `{agent}.message`) | Lossless provenance to the terminal boundary; the answer is auditable back to what produced it. No `Eq` (an `f64` lives under `ArtifactValue::Number`). |
| Prior turns field | **`Vec<Exchange>`** (no `Option`) | A `Vec` already models empty. |
| Final result type | **Distinct `FinalResult`** | Reserves room for future metadata. |
| Error model | **Fallible morphism** → `Result<_, AgentError>` | Kleisli arrows; error-threaded composition; the falling convention. |
| LLM communication | **Common capability across all agents** | The fetcher is itself an LLM-with-tools. |
| Isolation mechanism | **The agent's granted tool set** (not its capability type) | Follows from LLM being common; enforced at construction + dispatch guard. |
| Async / SDK | **`async`, targeting async-openai 0.41.1 via an adapter** | Domain stays SDK-agnostic; the vendor sits at the edge. |

---

## 5. Responsibility boundaries

- **Orchestration designer** — owns the `ArtifactKey`/`ArtifactValue` surfaces, mints every
  key (now an open `{agent}.{name}` string, by convention namespaced to the producing agent —
  §2.5), grants each agent its tool set, wires each tool's result to an artifact slot (via
  `Tool::target`), and assembles + validates the pipeline graph.
- **Sub-agent** — transforms one payload into another, exposing to its LLM *only* its granted
  tools, reading *only* granted artifacts, and returning a typed mismatch when handed a
  variant it does not accept.
- **The LLM** — used by *every* agent; decides which of the exposed tools to call. It never
  chooses a key name, never invents a tool, and is never trusted by types alone to refrain
  from inventing unprovided facts (that is the job of restricted context + output validation).

---

## 6. Open items / extension points

- **Compile-time tool isolation** (§3.5) — decide per deployment whether to encode tool sets
  in the type system.
- **`AgentError::Capability`** is a placeholder string. Give capability failures a taxonomy
  (retryable vs. fatal, which tool/transport) before finalizing — it shapes how the
  orchestrator routes around a failed stage. The *tool-input* half is now settled: a
  bad/invalid tool argument is a retryable `ToolOutcome::Rejected`, **not** an `AgentError`
  (see the [Tool contract](../tool/Contract.md) §1.2); `AgentError` is reserved for fatal
  transport/wiring failures.
- **Output-key validator** — the concrete "output references only provided keys" check (the
  enforceable half of §2.3) is not yet written.
- **`ArtifactValue` variants** — the value enum stays **closed** and is expected to grow as
  agents and wires are added (`Rows`, `Table`, `Bytes`, …). `ArtifactKey` is now **open** (§1),
  so key names grow freely without a contract edit.
- **async-openai version** — pinned to 0.41.1; revisit the adapter on upgrade.
