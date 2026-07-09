# SubAgent Contract

The contract governing how a **sub-agent is configured, resolved, and composed into
pipelines**. It layers *on top of* the [`AgentPayload` contract](../agent_payload/Contract.md):
that contract owns the value flowing between agents and the runtime morphism; **this** one
owns the *configuration surface* an author writes, the *resolution rules* that turn it into a
runnable agent, and the *composition* of agents into one or more pipelines.

The reference implementation is [`sub_agent.rs`](./sub_agent.rs), which compiles and is
clippy-clean against the sibling [`agent_payload.rs`](../agent_payload/agent_payload.rs) (it
includes it by relative path), and whose async test suite encodes the rules below.

---

## 0. What this contract binds

Same load-bearing distinction as the payload contract.

**Normative (binding).** The **config data model** (§1) — `LlmConfig`/`Provider`, `ToolId`,
`SubAgentConfig`, `PipelineConfig` — and the **resolution & composition rules** (§2):
default-LLM *field* merge, tool-grant isolation resolved at boot / fail-fast, k8s-style
secret binding, and the **self-checking composition rule**. Any loader that consumes these
configs MUST honor these rules, or its configs are not portable. The config schema *is* the
interface between the author and the system.

**Advisory (a suggestion).** *How* resolution and execution are implemented (§3): the
`ResolvedLlm`, the `ToolRegistry`, the generic `ConfiguredAgent`, the `Orchestrator`, and the
mapping onto the payload contract's `Tool`/`LlmCapability`. One encoding that satisfies §1–§2;
swap freely.

**Inherited, unchanged.** Everything the payload contract binds still binds here: the
`AgentPayload` sum type, `AgentError`, the fallible-morphism shape, the falling convention,
produce-don't-mutate, and producer-namespaced keys. This contract *adds a layer*; it does not
relax the payload contract.

---

## 1. Data model (normative)

### 1.1 The sub-agent is abstract; config drives the default implementation

A sub-agent is anything satisfying the payload morphism and the falling convention. The
**default** way to obtain one is *data*: a `SubAgentConfig` fed to one generic engine, so the
payload contract's `DataFetcher` / `ReportWriter` become *configs*, not bespoke Rust types.
Hand-written agents remain possible; both are the same abstract `SubAgent`.

`SubAgentConfig`:

| Field | Type | Meaning |
|---|---|---|
| `id` | `SubAgentId` | Stable identity. Namespaces the `ArtifactKey`s it produces (payload §2.5) and labels its logs. |
| `instruction` | `String` (from a prompt ref) | The system prompt this agent carries (payload §1). |
| `llm` | `Option<LlmConfig>` (all fields optional) | Per-agent LLM; unset fields inherit the default LLM (§2.1). `None` ⇒ the default verbatim. |
| `tools` | `Vec<ToolId>` | The **granted tool set** — the isolation boundary (payload §2.3). Closed set, resolved at boot (§2.2). Empty = a no-tool agent. |
| `accepts` | `Vec<PayloadKind>` | Which payload variants this agent consumes; drives its self-check (§2.4). |
| `output` | `Option<OutputShape>` | Execution-time shaping of the outgoing payload. `None` (the common case) derives the shape from the agent's position in whichever pipeline(s) reference it — see §2.4. An explicit value overrides the derivation. **Not** a static `produces` — see §2.4. |

There is **no `produces` field**. Static produces→accepts wiring validation is removed on
purpose (§2.4): composition safety is a runtime property, which is what lets a sub-agent be
recombined across several pipelines.

### 1.2 `LlmConfig` and the provider model

`LlmConfig` is **all-optional**, so a sub-agent states only what differs from the default LLM.
`provider` is *atomic* (overridden or inherited whole); `model` and each `GenerationParams`
field (`temperature`, `top_p`, `max_tokens`) merge independently.

`Provider` is a **known enum with a `Custom` escape hatch** — known variants carry their
default base URL and auth style:

| Variant | Base URL | Auth |
|---|---|---|
| `OpenRouter` | default hosted URL | key via the `OPENROUTER_API_KEY` secret |
| `Ollama { endpoint? }` | `http://localhost:11434/v1` (default) | keyless |
| `Custom { name, endpoint, api_key? }` | author-supplied | optional secret ref |

### 1.3 `ToolId` and the registry — a closed set, resolved at boot

Tools are named by a logical **`ToolId`** — a **closed enum** (parity with `ArtifactKey`: a
typo is a compile/parse error), decoupled from any backend. A designer-owned **`ToolRegistry`**
maps each `ToolId` to a concrete `Tool` (schema + `target: ArtifactKey` + call impl). **MCP is
one backend**: an MCP-backed tool wraps an `McpHandle` + a tool name. `ToolId` (which *grants*
a tool) stays distinct from `ArtifactKey` (the *slot* a tool's result fills).

**Multiple MCP servers are supported and require no type change.** The registry is
backend-agnostic — each `ToolId` binds to its own backend — so different `ToolId`s may be
backed by *different* MCP servers (or non-MCP backends). Two consequences bind any
implementation: (a) a tool's name **advertised to the LLM** derives from its canonical
`ToolId` string, not the raw per-server tool name, so tools never collide across servers
within an agent's exposed set; and (b) MCP handshake `instructions` are a **per-server**
concern, so the instructions composed into a `ConfiguredAgent`'s system prompt are those of
the servers backing *its granted tools* — not one global block.

### 1.4 Pipelines are first-class, and there may be many

Orchestration is a first-class concept. A **`PipelineConfig`** (`id` + ordered `stages` of
`SubAgentId`) is its own config entry, and a deployment may declare **several**. Because
compatibility is a runtime guarantee (§2.4), the same sub-agent may appear in more than one.

---

## 2. Resolution & composition rules (normative)

Resolution is **two-phase**: **Load** (parse config, no network) then **Resolve** (merge LLM,
bind secrets, resolve tools, build agents). Every resolution failure surfaces at **boot**,
before a single LLM call — fail fast.

### 2.1 Default-LLM field merge

The default LLM is a fully-resolved record. A sub-agent's `LlmConfig` merges over it **field
by field**: `provider` atomic; `model` and each param independent. A required field unset in
both agent and default (e.g. no `model` anywhere) is a **resolution error**.

### 2.2 Tool grant = isolation boundary (closed, boot-resolved)

The agent exposes **exactly** its resolved grant; any call outside the set is rejected at
dispatch (`AgentError::UnknownTool`, payload §2.3). `ToolId` is a closed set; every grant is
resolved against the registry **at boot**, and an unresolvable id **fails boot** — never a
deferred failure at first call. An empty grant is valid and means the agent cannot fetch.

### 2.3 Secrets are referenced (k8s-style), checked at boot

A provider that needs a key carries a **`SecretRef`**: a key name that matches a corresponding
**environment entry** (config names the key; the environment supplies the value). Config files
never carry raw secrets. Keyless providers omit it. A referenced key with no matching
environment entry **fails boot**.

### 2.4 Self-checking composition (the reason there is no `produces`)

Composition safety is a **runtime** property, not a static graph check. Every sub-agent checks
its own input against what it `accepts` and returns `AgentError::Mismatch` when handed a
variant it does not accept (payload §2.2); it never panics. Pipelines are therefore just
ordered lists, and a mismatch surfaces as a typed, routable error at run time. This is
deliberate: it lets sub-agents be recombined into different pipelines without re-deriving a
static wiring graph. `OutputShape` shapes an agent's *own* outgoing payload at execution time
and participates in no cross-agent validation.

**Default output is derived from pipeline position, not authored per agent.** Most sub-agents
never set `output`: at resolution time, the loader inspects every declared `PipelineConfig` and
determines, for a given `SubAgentId`, whether it is a *terminal* stage (nothing follows it) —
consistently across every pipeline it appears in. Terminal everywhere ⇒ the default is `Final`;
non-terminal everywhere ⇒ the default is `Intermediate`. If the same sub-agent is terminal in
one declared pipeline and non-terminal in another (the `quick_fetch`-style reuse case in §3) and
its `output` is unset, that is a **resolution error**: the author must set `output` explicitly
to disambiguate. This keeps `output` optional in the common single-role case while keeping the
field for the one case it exists to serve — an agent whose outgoing shape must diverge from its
structural position.

### 2.5 Identity namespaces production (no enforcement yet)

Each sub-agent's `id` namespaces the `ArtifactKey`s it produces (payload §2.5) and attributes
its logs. For now this is **convention, not enforced** — an agent is free to write any key.
Enforcing "an agent writes only under its own namespace" is a cheap later addition (a
validator) and is out of scope here.

### 2.6 A resolved sub-agent is a pure function

Given injected capabilities (LLM, tools), a resolved sub-agent is a pure async function of its
input payload; resolution is a *separate* step from execution. Each sub-agent is unit-tested
with mocked capabilities and a scripted LLM — no config, no network — exactly as the payload
contract's tests do.

---

## 3. Recommended implementation (advisory)

One encoding satisfying §1–§2; swap freely. See [`sub_agent.rs`](./sub_agent.rs).

- **`ResolvedLlm`** — the option-free product of §2.1 (provider, base URL, bound key, model,
  params). Constructs the payload contract's `LlmCapability`; an Ollama/Custom endpoint is the
  same OpenAI-compatible adapter with a different base URL + key. The `ResolvedLlm` →
  capability *factory* needs the vendor SDK and is an implementation-plan item; the reference
  injects the capability directly so the engine unit-tests without a network.
- **`ToolRegistry`** — `HashMap<ToolId, ToolFactory>` the designer populates at startup;
  `resolve(&grants)` indexes it per grant and **fails boot on a miss**. The factory indirection
  is the seam that lets one logical tool be re-backed (MCP → HTTP → mock).
- **`effective_output`** — implements the position-derived default above: the config's
  explicit value wins; otherwise it scans every declared `PipelineConfig` for the agent's id
  and returns `Final`/`Intermediate` when its terminal-ness is consistent, or a resolution
  error when it is not. Runs once per agent, before that agent's `ConfiguredAgent` is built —
  it needs the full pipeline set, which a single agent's own config does not carry.
- **`ConfiguredAgent`** — the generic engine implementing the abstract `SubAgent`: self-checks
  `accepts` (§2.4), seeds `run_llm_loop` with `instruction` + payload over the granted tools,
  and shapes the result via a **resolved** `OutputShape` (the config's explicit value, or
  `effective_output`'s derived default — never `SubAgentConfig.output` read directly), merging
  incoming artifacts append-only.
- **`Orchestrator`** — holds every resolved pipeline and threads a payload through a selected
  one with `?` (Kleisli composition; the first mismatch short-circuits). `resolve_pipeline`
  fails boot on an unknown stage reference.
- **`impl LlmCapability for Arc<dyn LlmCapability>`** — a small adapter so a `ConfiguredAgent`
  can hold a type-erased LLM yet still call the payload contract's `run_llm_loop` (whose type
  parameter is `Sized`).

### TOML surface (advisory)

Extends today's `config.toml` (see [`config.rs`](../../../src/config.rs)):

```toml
version = 1

[llm.default]
provider = "openrouter"          # key via secret ref → OPENROUTER_API_KEY
model = "google/gemini-flash"
temperature = 0.7

[[sub_agent]]
id = "fetcher"
instruction = { file = "prompts/fetcher.md" }
tools = ["bill_revenue", "station_revenue_ranking"]
accepts = ["initial"]
output = "intermediate"          # explicit: terminal in `quick_fetch` below but not in
                                  # `revenue_report` — position alone is ambiguous here
# no [sub_agent.llm] → inherits [llm.default] verbatim

[[sub_agent]]
id = "writer"
instruction = { file = "prompts/writer.md" }
tools = []                       # no-tool agent
accepts = ["intermediate"]
# no `output` → derived: terminal in every pipeline it appears in ⇒ "final"
[sub_agent.llm]
model = "anthropic/claude-sonnet-5"   # override only the model; inherit provider + params

[[pipeline]]
id = "revenue_report"
stages = ["fetcher", "writer"]

[[pipeline]]
id = "quick_fetch"               # multiple pipelines; reuse the same sub-agent
stages = ["fetcher"]
```

---

## 4. Locked decisions

| Question | Decision | Rationale |
|---|---|---|
| Provider representation | **Known enum + `Custom` escape hatch** | Type-safety and default auth/URL for known providers; open-ended for the rest. |
| Default-LLM fallback | **Field-level merge** (provider atomic) | State only what differs; provider is not meaningfully field-mergeable. |
| Tool identification | **Abstract `ToolId` registry**; MCP one backend | Decouples a grant from its backend; re-back a tool without touching agent config. |
| Tool set resolution | **Closed set, resolved at boot, fail-fast** | An unknown grant is a config error, caught before any LLM call. |
| Secrets | **k8s-style env reference**, checked at boot | Config commits safely; a missing secret fails boot. |
| `produces` / wiring | **Dropped**; runtime self-check instead | Enables free recombination and multiple pipelines. |
| `output` default | **Position-derived when unset**; ambiguous position requires an explicit value | Removes a footgun (a configured shape disagreeing with actual pipeline position) while keeping the field for the one case that needs it: reuse across pipelines at different positions. |
| Pipelines | **First-class, multiple** | Orchestration is a first-class, practical concern. |
| Namespace enforcement | **None yet** (convention only) | Cheap to add later; not worth the machinery now. |
| Sub-agent shape | **Abstract, config-driven default** | One generic engine; hand-written agents still possible. |

---

## 5. Responsibility boundaries

- **Orchestration designer** — owns the `ToolRegistry` (mints `ToolId`s, wires each to a
  backend + `ArtifactKey` target), authors `SubAgentConfig`s and `PipelineConfig`s, supplies
  the default LLM and the secret environment, and assembles the `Orchestrator`.
- **Sub-agent** — transforms one payload into another, exposing to its LLM *only* its granted
  tools, self-checking its input and returning a typed `Mismatch` when handed a variant it does
  not accept.
- **The LLM** — used by every agent; decides which exposed tool to call. It never names a key,
  never invents a tool, and is never trusted by types to refrain from inventing facts.

---

## 6. Open items / extension points

These are deferred to the [implementation plan](../../plan/sub_agent.md), not to the contract:

- **`ResolvedLlm` → `LlmCapability` factory** — the vendor-SDK construction (feature-gated).
- **Orchestration engine details** — how an incoming request selects a pipeline id.
- **Boot-time secret validation** — the concrete k8s-style key→env check and error surface.
- **Global attribution block** — where OpenRouter `HTTP-Referer` / `X-Title` live now that
  providers are per-agent.
- **Migration** — today's single `GenerationConfig` + shared `McpHandle` → `[llm.default]` +
  registry; the existing `/agent` path → a one-stage pipeline.
- **Multiple MCP servers** — the config surface for N servers, their connection lifecycle,
  per-server instruction routing, and `ToolId`-derived tool naming (plan steps 2–3).
- **Namespace enforcement (future)** — the cheap validator for §2.5.
- **`ToolId` / `Provider` variants** — expected to grow as tools and providers are added.
