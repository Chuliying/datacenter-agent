# Tool Contract

The contract governing **what a tool is, how the LLM invokes it, how it is named and resolved,
and how its structured output is enforced**. It sits between the
[`AgentPayload` contract](../agent_payload/Contract.md) (which owns the `Tool` trait, the
`ToolOutcome`, and the tool-use loop) and the [`SubAgent` contract](../sub_agent/Contract.md)
(which owns the *grant* — which tools an agent is given, the isolation boundary). This contract
**consolidates the tool decisions** that were previously scattered across those two.

The reference implementation is [`tool.rs`](./tool.rs), which compiles and is clippy-clean
against [`agent_payload.rs`](../agent_payload/agent_payload.rs) (included by relative path), and
whose async test suite encodes the rules below.

---

## 0. What this contract binds

**Normative (binding).**
- The **tool abstraction** (§1.1): a tool is *a named capability the LLM invokes, whose result
  fills an artifact slot*. Data fetches, output sinks, and validators/compute are all the same
  `Tool`.
- The **outcome & retry-feedback rule** (§2.1): `Tool::call` yields a `ToolOutcome` — `Produced`
  fills the tool's `target`; `Rejected { reason }` is fed back to the model and **no artifact is
  recorded**; a fatal `Err(AgentError)` aborts. A rejected call is **retryable within the loop**.
- The **closed-set / boot-resolution rule** (§2.2): `ToolId` is a closed set; every grant and
  every backend registration is resolved/checked at boot, and a gap fails fast.
- The **advertised-name rule** (§2.3): the name shown to the LLM is the canonical `ToolId`
  string, never a raw backend name.

**Advisory (a suggestion).** The `ToolRegistry`, the generic `SchemaTool<T>` adapter, the
`#[tool]`-macro / `inventory` ergonomics (§3), and the schema library (`schemars`). One encoding
that satisfies §1–§2; swap freely.

**Inherited, unchanged.** The `AgentPayload` sum type, `ArtifactKey`/`ArtifactValue`,
producer-namespaced keys, and the fallible-morphism shape all still bind.

---

## 1. Data model (normative)

### 1.1 A tool is one abstraction over three kinds of backend

A **tool** is *a named capability the LLM invokes, whose result fills an artifact slot* — the
`Tool` trait (`schema` + `target: ArtifactKey` + `call`). This one abstraction spans:

| Kind | Backend | `call` does | Example |
|---|---|---|---|
| **Data fetch** | MCP server (or HTTP) | reach the outside world, return data | `bill_revenue` |
| **Output sink** | code | *validate* the model's own structured output against a protocol, emit it | `emit_chart` |
| **Validator / compute** | code | validate inputs, compute a derived value | `calculate` |

The earlier framing ("a tool interacts with the real world") is **broadened**: a sink and a
validator touch nothing external — they exist so the model's *own* output is schema-checked
before it becomes an artifact. All three are dispatched identically by the tool-use loop and
isolated identically by the grant.

### 1.2 `ToolOutcome` — rejection is a retryable outcome, not an error

`Tool::call` returns `Result<ToolOutcome, AgentError>`:

- **`ToolOutcome::Produced(ArtifactValue)`** — success; the value fills the tool's `target`.
- **`ToolOutcome::Rejected { reason }`** — the call could not be honoured (bad arguments, failed
  schema validation, a domain rule like divide-by-zero). **Not an error.** The loop feeds
  `reason` back to the model as a tool message and records **no** artifact, so the model
  corrects and calls again. This is what makes a validating tool *loop until valid* for free.
- **`Err(AgentError)`** — a *fatal* failure (transport down, wiring bug). Aborts the run.

The distinction is load-bearing: rejection is the normal, expected path for schema enforcement;
`Err` is reserved for the genuinely unrecoverable.

### 1.3 `ToolId` — a closed logical name, decoupled from the backend

Tools are named by a **`ToolId`** — a closed enum, so a mistyped tool name is a compile/parse
error. (The **tool set stays closed** even though the payload contract opened `ArtifactKey` to a
`{agent}.{name}` string — a typo'd *artifact key* is now a runtime key, but a typo'd *tool name*
is still caught at compile time.) A `ToolId` is distinct from:
- its **backend** (which server, or which code type serves it), and
- its **`target: ArtifactKey`** (the slot its result fills).

One `ToolId` may be re-backed (MCP → HTTP → sink → mock) without touching any grant. The
**grant** — which `ToolId`s an agent may call — is the sub-agent contract's isolation boundary
([sub_agent §2.2](../sub_agent/Contract.md)); this contract owns the *naming, backend, and
resolution*.

---

## 2. Rules (normative)

### 2.1 The tool-use loop feeds rejections back, bounded

The loop (payload contract's `run_llm_loop`) dispatches each requested call to the matching
`Tool` and:
- on `Produced(v)` — records `target → v` and feeds `v` back as the tool result;
- on `Rejected { reason }` — records **nothing** and feeds `REJECTED: {reason}` back;
- on `Err` — aborts.

The overall **step cap** bounds retries: a model that can never satisfy a schema fails when the
cap is hit, rather than looping forever. A rejected attempt must never leave a half-valid
artifact in the map.

### 2.2 Closed set, resolved at boot, fail-fast

`ToolId` is closed. Two boot checks, both fail-fast:
- **Grant resolution** — every `ToolId` an agent is granted must resolve to a registered
  backend, or boot fails naming the offending `(agent, ToolId)`.
- **Completeness** — every `ToolId` in the closed set has exactly one backend (no gap, no
  duplicate). This is what lets ergonomic auto-registration (§3) stay honest: however backends
  are *collected*, the closed set remains the source of truth and boot verifies coverage.

### 2.3 The advertised name is the canonical `ToolId` string

The tool name shown to the LLM derives from the `ToolId`'s canonical string, **not** a raw
backend name (an MCP server's `mcp_name`, a function name). So two backends exposing the same
raw name never collide within an agent's exposed set. A backend whose raw name differs carries
that raw name internally (for MCP, the `mcp_name` override — see [plan §4](../../plan/sub_agent.md)).

### 2.4 Structured output is enforced by validation, not trusted

A sink/validator's schema is advertised to the LLM (best-effort constraint), but the guarantee
comes from **validating on receipt** (`call` deserializes into the protocol type; failure ⇒
`Rejected`). The producing agent thus emits only conforming artifacts. A **consumer** in a later
stage re-parses the artifact into the same protocol type (parse-don't-validate) — a malformed
upstream artifact becomes a typed error at the consumer, not a render-time crash. The protocol
is a shared Rust type (`serde` + `schemars`), the single source of truth for both ends.

---

## 3. Recommended implementation (advisory)

One encoding satisfying §1–§2; see [`tool.rs`](./tool.rs).

- **`ToolRegistry`** — `HashMap<ToolId, ToolFactory>` the designer populates at startup;
  `resolve(&grants)` fails fast on an unknown id, `assert_complete(ALL_TOOL_IDS)` fails fast on
  a gap, `register` rejects duplicates. The factory indirection re-backs a logical tool without
  touching a grant.
- **`SchemaTool<T>`** — the generic adapter that turns any `T: JsonSchema + DeserializeOwned`
  into a `Tool`: the advertised schema is derived from `T` (via `schemars`), and `call`
  validates by deserializing into `T`, mapping a failure to `Rejected` (never a crash). The
  `on_valid` step is the variable part:
  - `SchemaTool::sink` — identity: the validated `T`, serialized, *is* the artifact (charts);
  - `SchemaTool::new` — transform: map the validated `T` to an `ArtifactValue`, and optionally
    `Reject` on a domain rule (a calculator's divide-by-zero).
  One adapter covers charts, calculators, and any future protocol-checked capability.
- **MCP-backed tools** — the other backend; an `McpTool { handle, mcp_name, target, id }`
  delegates `call` to the server. Registered like any other `ToolId`. See
  [plan §4](../../plan/sub_agent.md) for the pool, the `mcp_name` convention, and per-server
  instruction routing.

### `#[tool]` macro & auto-registration (advisory, deferred)

A `#[tool(id = "emit_chart", target = "charts.spec")]` attribute proc-macro can remove the
boilerplate (derive the schema from the type, generate the `Tool` impl, wire the target). It may
also **auto-collect** backends via `inventory`/`linkme` (compile/link-time registration) so a
new tool is picked up without editing a central list.

The decision: **auto-collection is a convenience, never the source of truth.** Whatever the
macro collects is validated at boot against the closed `ToolId` set (§2.2's completeness check),
so the closed-set / fail-fast guarantee survives — a link-dropped or mis-tagged tool fails boot
rather than silently vanishing. The **default is explicit registration** (a designer-owned
`build_registry()` with one `register` per tool); adopt `inventory` collection only when the
tool count makes the explicit list a burden. The macro itself needs its own proc-macro crate, so
it is an implementation-plan item, not shipped in the reference.

---

## 4. Locked decisions

| Question | Decision | Rationale |
|---|---|---|
| What is a tool | **A named capability the LLM invokes, whose result fills an artifact slot** | One abstraction for fetch / sink / validator; broadens "interact with the world". |
| Rejection model | **`ToolOutcome::Rejected { reason }`, distinct from fatal `Err`** | Bad/invalid input is retryable and model-facing; only transport/wiring is fatal. |
| Retry | **Fed back to the model, not recorded, bounded by the loop's step cap** | "Loop until valid" for free; no half-valid artifacts; guaranteed termination. |
| Structured output | **Function-call-as-sink (validate on receipt), not `response_format`** | Reuses the existing tool seam and the retry loop; no new `LlmCapability` channel. |
| Generic adapter | **`SchemaTool<T>` over `serde` + `schemars`** | One type covers every protocol-checked tool (charts, calculators, …). |
| Schema source | **`schemars`-derived from the Rust type** | Schema and validation can't drift; the type is the single source of truth. |
| `ToolId` | **Closed enum; grant + backend resolved at boot, fail-fast; completeness-checked** | A typo/gap is a boot error, never a first-call surprise. |
| Registration | **Explicit by default; `#[tool]` + `inventory` optional, reconciled by the boot completeness check** | Ergonomics without losing the closed-set/auditable-registry guarantee. |
| Advertised name | **Canonical `ToolId` string** | No cross-backend name collisions within an agent's set. |

---

## 5. Responsibility boundaries

- **Orchestration designer** — mints `ToolId`s, writes/registers each backend (MCP handle, or a
  `SchemaTool` over a protocol type), wires each tool's `target` to an `ArtifactKey`, and
  assembles the registry (asserting completeness at boot).
- **Tool** — advertises its schema, validates its input, returns `Produced`/`Rejected`, and
  fills exactly its `target`. It never chooses a key name and never invents another tool.
- **The LLM** — decides which exposed tool to call and with what arguments; on a `Rejected`
  result it corrects and retries. It is never trusted by types to emit a conforming shape — the
  tool validates.

---

## 6. Open items / extension points

- **`#[tool]` proc-macro crate** — the attribute macro + optional `inventory` collection (§3).
- **Terminal-tool semantics** — optionally letting a successful sink call end the turn instead
  of requiring a wrap-up message. Not needed yet.
- **`response_format` channel** — a *second* structured-output path (constrain generation at the
  model) was considered and **not** chosen (function-call-as-sink covers it via the retry loop);
  revisit only if a provider path needs it.
- **`ToolId` / backend variants** — expected to grow as tools are added.
