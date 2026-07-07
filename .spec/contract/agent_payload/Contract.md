# AgentPayload Contract

The contract governing values that flow between LLM sub-agents. The reference
implementation is [`agent_payload.rs`](./agent_payload.rs), which compiles, is
clippy-clean, and whose async test suite encodes the rules below. Its async-openai adapter
compiles against async-openai **0.41.1** behind `--features openai`.

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

- **`InitialPrompt`** — `prompt: String` plus `history: Vec<Exchange>`. There is no system
  prompt: each agent carries its own designed instruction. History is a plain `Vec` — a
  `Vec` already models "empty", so no `Option`.
- **`IntermediateData`** — `prompt: String` (the instruction, carried forward) plus
  `artifacts: HashMap<ArtifactKey, ArtifactValue>`, the KV surface wired between agents.
- **`FinalResult`** — `user: String` and `assistant: String`. Its **own type** (not
  `Exchange`) so metadata (stop reason, token usage, latency) can accrete here later.

### Supporting types

- **`Exchange`** — one `user`/`assistant` turn inside history. Structurally `FinalResult`
  minus metadata, kept **distinct on purpose**: history is plain chat; `FinalResult` is an
  extension point.
- **`ArtifactValue`** — a **closed enum** (`Text`, `Json`, `Number`, …). `Display` is the
  "cast to string if you don't care about the type" view; a `match` is the checked
  "downcast". Being closed is what lets the whole payload derive `Clone` + `Serialize`.
- **`ArtifactKey`** — a **compile-time enum**, so a mistyped key is a compile error. Each
  variant is namespaced by producer and renders to a canonical dotted string
  (`fetcher.records`) used as *both* its log form and its serialized form (JSON object keys
  must be strings).

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
one, because it has no capability to choose a key name.

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
key names** via `target`. The LLM never names a key or invents a tool.

### 3.3 The tool-use loop

`run_llm_loop` sends the agent's tool schemas to the LLM, dispatches each requested call to
the matching `Tool`, feeds results back, and repeats until the LLM returns a final message.
Tool results are collected into the artifact map, keyed by each tool's `target`. A call to a
tool the agent does not own is rejected here — the isolation boundary of §2.3, guarded at
dispatch. A step cap prevents a non-terminating loop.

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
| Artifact value representation | **Closed enum** with `Display` | Retains the typed value *and* keeps the payload `Clone` + `Serialize`. |
| Artifact key type | **Compile-time enum** | A mistyped key is a compile error; keys are minted only by the designer. |
| Prior turns field | **`Vec<Exchange>`** (no `Option`) | A `Vec` already models empty. |
| Final result type | **Distinct `FinalResult`** | Reserves room for future metadata. |
| Error model | **Fallible morphism** → `Result<_, AgentError>` | Kleisli arrows; error-threaded composition; the falling convention. |
| LLM communication | **Common capability across all agents** | The fetcher is itself an LLM-with-tools. |
| Isolation mechanism | **The agent's granted tool set** (not its capability type) | Follows from LLM being common; enforced at construction + dispatch guard. |
| Async / SDK | **`async`, targeting async-openai 0.41.1 via an adapter** | Domain stays SDK-agnostic; the vendor sits at the edge. |

---

## 5. Responsibility boundaries

- **Orchestration designer** — owns the `ArtifactKey`/`ArtifactValue` surfaces, mints every
  key, grants each agent its tool set, wires each tool's result to an artifact slot (via
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
  orchestrator routes around a failed stage.
- **Output-key validator** — the concrete "output references only provided keys" check (the
  enforceable half of §2.3) is not yet written.
- **`ArtifactValue` / `ArtifactKey` variants** — expected to grow as agents and wires are
  added.
- **async-openai version** — pinned to 0.41.1; revisit the adapter on upgrade.
