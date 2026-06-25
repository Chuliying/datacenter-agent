# Agent Runtime Rust Port — Complete Migration Plan

**Story ID**: S-RUNTIME-01  
**Date**: 2026-06-25  
**Status**: draft  
**Target repo**: `datacenter-agent`  
**Source repo**: `falcon-client`  
**QA plan**: `.agent/artifacts/qa/2026-06-25-agent-runtime-rust-port/qa-plan.md`

---

## 1. Migration Goals

1. Move portable agent runtime authority from `falcon-client` TypeScript into `datacenter-agent` Rust.
2. Preserve external client contracts: `/agent` JSON and `/agent/stream` SSE frames remain compatible.
3. Preserve behavior parity for the EV Chief-of-Staff capability pack before changing user-facing flow.
4. Leave `falcon-client` as UI/client layer only, with no duplicated runtime authority.
5. Keep a rollback path until Rust runtime passes parity, QA gates, and live smoke.

---

## 2. Source Repo Handling Strategy (`falcon-client`)

The original repo must not be deleted or rewritten first. It is the behavioral oracle during migration.

### 2.1 Freeze Scope

| Area | Action | Owner | Exit Criteria |
|------|--------|-------|---------------|
| `src/lib/chief-of-staff/agent-runtime/` | Feature freeze except bug fixes needed for characterization | Source repo owner | baseline tag created |
| `docs/plans/chief-of-staff-agent-runtime/*` | Freeze as migration source docs | Source repo owner | copied/linked in migration notes |
| eval fixtures | Export as parity seed fixtures | Runtime team | fixtures available in target repo |
| UI agent client | Allow endpoint-switch work only | Frontend owner | feature flag can point to Rust runtime |

### 2.2 Baseline Tag

Create a source repo tag or immutable commit reference:

```bash
git -C ../falcon-client rev-parse HEAD
git -C ../falcon-client tag agent-runtime-rust-port-source-2026-06-25
```

Record the commit in `datacenter-agent/docs/agent-runtime-rust-port/migration-log.md` when implementation begins.

### 2.3 Characterization Before Port

Before porting logic, extract behavior from TS into machine-readable fixtures:

| Source TS Area | Characterization Output | Target Location |
|----------------|-------------------------|-----------------|
| `input-normalizer.ts` | raw input -> clean input cases | `config/runtime/evals/inputs.json` + unit fixtures |
| `intent-classifier.ts` / constants | option/text -> intent/confidence/source | `config/runtime/evals/inputs.json` |
| `slot-extractor.ts` | text -> slots/warnings | `config/runtime/evals/inputs.json` |
| `injection-patterns.ts` | text -> injection hit/no-hit | unit fixtures |
| `answer-policy.ts` | normalized input -> action | unit fixtures |
| `memory-context.ts` | memory payload -> prompt context/drop reason | unit fixtures |
| `audit-log.ts` | event -> redacted output | unit fixtures |
| `run-agent-turn.ts` | refusal/disclaimer/history behavior | orchestrator fixtures |

### 2.4 Dual Ownership Rule

During migration:

- TypeScript runtime remains production fallback until Rust parity is green.
- New runtime features land only in Rust unless they are emergency bug fixes.
- Bug fixes discovered in TS during characterization are either:
  - ported as expected behavior if intentional, or
  - documented as TS bug and fixed only in Rust with approved parity diff.

### 2.5 Source Repo End State

After Rust rollout:

1. Replace TS runtime calls with `datacenter-agent` API calls.
2. Keep TS runtime fixtures/tests archived for historical parity for one release cycle.
3. Mark TS runtime implementation deprecated in source comments.
4. Remove TS runtime implementation only after:
   - Rust runtime has served production traffic for the agreed soak period.
   - No rollback to TS has happened.
   - QA report is validated.

---

## 3. Target Repo Phase Plan (`datacenter-agent`)

### Phase 0: Readiness / Guard Rails

| Task | Output | Gate |
|------|--------|------|
| Confirm source commit/tag | migration log entry | source ref immutable |
| Confirm current Rust endpoint behavior | characterization tests | `/agent` and `/agent/stream` baseline captured |
| Add dependencies | `Cargo.toml` | build passes |
| Create file structure | `src/runtime/*`, config pack skeleton | compiles with empty modules |
| Decide feature flag | env/config flag, e.g. `RUNTIME_ENABLED` | default can be off during build |

Gate:

```bash
cargo test
```

### Source Request → Rust DTO Mapping

`falcon-client` source request shape must be translated explicitly before fixtures or endpoint cutover.

| TS source field | Rust target field | Migration rule |
|-----------------|-------------------|----------------|
| `input` | `prompt` | rename; trim/empty validation happens in Rust runtime |
| `sessionId` | `session_id` | rename; controls server memory scope |
| `optionId` | `option_id` | rename; known prefix maps option-path; unknown prefix warning + fallback, not 400 |
| `optionLabel` | none in DTO | UI-only display metadata; do not send to Rust runtime |
| `locale` | future extension | not used in this migration; record in fixture metadata only |
| `timezone` | future extension | not used in this migration; Rust server already injects current time in prompt |
| `memoryPayload` | deprecated | not forwarded in server-memory mode; characterization may use it only to build source parity fixtures |
| prior turns | `history` | sent only when no `session_id`; server-memory mode sends upstream `history: []` |

Add a small fixture transformer in source repo or migration scripts before Phase 2 so TS characterization fixtures can be consumed by Rust eval without hand editing.

### Phase 1: Config + Registry + Schema + Error

Implements `spec-01`.

| Task | Details |
|------|---------|
| Add `RuntimeConfig` loader | Resolve pack refs relative to manifest root |
| Add `Assembly` | `input_stages`, `answer_policy_backend`, memory/audit/eval ids |
| Add `Registry` | build input pipeline, answer policy, memory, audit, evaluators |
| Add schema | `NormalizedInput`, slots, registry versions |
| Add errors | config vs per-request `RuntimeError` |

Gate:

```bash
cargo test runtime::config
cargo test runtime::registry
cargo test runtime::schema
```

Rollback: no route uses runtime yet; disable module exports if needed.

### Phase 2: L5 Input Pipeline

Implements `spec-02`.

| Task | Details |
|------|---------|
| Normalizer | NFKC + explicit punctuation map |
| Intent | `option_id`, lexicon scoring, margins, text override |
| Slots | time/metric/asset/rank extractors |
| Pipeline | ordered sync `PipelineStage` execution |
| Parity fixtures | TS cases imported from source repo |

Gate:

```bash
cargo test runtime::input
cargo run --bin eval -- --pipeline-only
```

Rollback: runtime still not wired to handlers.

### Phase 3: L6 Guardrails + Answer Policy

Implements `spec-03`.

| Task | Details |
|------|---------|
| Input guard | required/length checks |
| Injection | versioned regex set; JS regex semantics reviewed |
| Answer policy | rule backend with refusal/disclaimer/action |
| Tests | injection/off-scope/gray/answer coverage |

Gate:

```bash
cargo test runtime::guardrails
cargo run --bin eval -- --pipeline-only
```

Rollback: keep feature flag off.

### Phase 4: Audit + Memory

Implements `spec-04`.

| Task | Details |
|------|---------|
| `AuditSink` | stdout sink, event enum, seq, redaction |
| Audit failure policy | `[runtime.audit] failure_policy = "fail-open"|"fail-closed"`; `AuditSink::write` returns `Result` |
| Tool audit contract | ensure tool called/result can be represented |
| `SessionMemoryStore` | in-memory store |
| Memory context | sanitize/truncate/budget/drop reason |

Gate:

```bash
cargo test runtime::audit runtime::memory
```

Risk: ensure both audit failure policies are covered by unit and integration tests before release.

### Phase 5: Orchestrator + Handler Wiring

Implements `spec-05`.

| Task | Details |
|------|---------|
| `AgentTurnInput` | raw request includes `prompt`, `history`, `session_id`, `option_id` |
| `AgentPort` adapter | wraps `llm_connector`; exposes ToolCalled/ToolResult |
| Buffer semantics | `Clear` clears buffer and audits |
| Handler wiring | `/agent` and `/agent/stream` use orchestrator |
| Wire compatibility | only external frames: token/done/error/clear |

Gate:

```bash
cargo test runtime::orchestrator
cargo test
```

Rollback:

- Feature flag routes handlers back to current `prepare_config -> llm_connector` path.
- Keep old handler path until Phase 7 stable.

### Phase 6: Eval Subsystem

Implements `spec-06`.

| Task | Details |
|------|---------|
| `Evaluator` trait | pipeline vs response modes |
| Fixture loader | pack-relative `inputs.json` |
| Baseline | `response-baseline.json` is newly produced in this migration from recorded TS responses or approved live samples |
| CLI | `src/bin/eval.rs` |
| Replay support | response eval can run without live provider |

Gate:

```bash
cargo run --bin eval -- --pipeline-only
```

Optional live gate:

```bash
cargo run --bin eval -- --response --live
```

### Phase 7: Dual Run / Shadow Mode

Purpose: compare TS runtime and Rust runtime before production cutover.

| Mode | Behavior |
|------|----------|
| Offline parity | Run same fixtures against TS baseline and Rust pipeline |
| Shadow request | UI still uses TS path; mirror anonymized request to Rust if allowed |
| Response replay | Feed recorded observed turns to Rust response evaluators |

Required artifacts:

- parity report with pass/fail per fixture
- approved diff list for intentional changes
- no P0/P1 diff without owner sign-off

Gate:

```bash
cargo run --bin eval -- --pipeline-only
```

Plus source repo parity runner output, if available.

### Phase 8: Frontend Cutover (`falcon-client`)

| Task | Details |
|------|---------|
| Add endpoint flag | switch agent client to Rust `/agent` and `/agent/stream` |
| Send `session_id` | stable per conversation |
| Send `option_id` | when user chooses an option path |
| Stop sending duplicated runtime memory | client should rely on server memory when session is active |
| Preserve fallback | env flag can route back to TS runtime for one release |

Gate:

- UI smoke passes against Rust endpoint.
- SSE parser accepts unchanged token/done/error/clear.
- Refusal/disclaimer display remains acceptable.

### Phase 9: Production Rollout

| Stage | Traffic | Gate |
|-------|---------|------|
| Canary | internal users only | no P0/P1, audit present |
| 10% | small external slice | latency/refusal/fallback within baseline |
| 50% | broad slice | no parity regression |
| 100% | full Rust runtime | rollback flag still available |

Metrics:

- request count / error count
- refusal rate
- fallback/unknown rate
- latency p50/p95
- tool-call error rate
- audit write count vs request count

### Phase 10: Source Repo Deprecation / Cleanup

Only after soak period:

1. Mark TS runtime as deprecated.
2. Remove production references to TS runtime.
3. Keep fixtures and parity docs for one release cycle.
4. Archive TS runtime docs or move to `docs/archive`.
5. Delete TS runtime implementation only after rollback window closes.

---

## 4. Original Repo Change Plan (`falcon-client`)

### Required PRs

| PR | Purpose | Timing |
|----|---------|--------|
| FC-1 | Add source baseline tag / migration note | Before Rust Phase 1 |
| FC-2 | Export/normalize fixtures for parity | Before Rust Phase 2 |
| FC-3 | Add endpoint feature flag | Before Rust Phase 8 |
| FC-4 | Send `session_id` and `option_id` to Rust runtime | Phase 8 |
| FC-5 | Disable TS runtime by default | After Rust 100% rollout |
| FC-6 | Remove or archive TS runtime | After soak / rollback window |

### What Stays in `falcon-client`

- UI state and rendering.
- Conversation/session id generation.
- Option selection UX and `option_id` forwarding.
- API client for `/agent` and `/agent/stream`.
- Display handling for existing SSE frames.

### What Leaves `falcon-client`

- Intent classification authority.
- Slot extraction authority.
- Prompt injection decision authority.
- Server memory authority.
- Audit authority.
- Eval CI gate for runtime behavior, except source fixture export/parity checks.

### Temporary Compatibility Layer

Until TS runtime is removed:

```text
falcon-client
  ├─ runtime_client = rust | ts
  ├─ rust mode: call datacenter-agent /agent(/stream)
  └─ ts mode: existing local portable core fallback
```

Rules:

- Default remains current behavior until Rust Phase 7 passes.
- Once canary starts, default becomes Rust for selected users.
- TS fallback removal requires signed migration report.

---

## 5. Data / Config Migration

| Source | Target | Transform |
|--------|--------|-----------|
| TS intent allowlist / enum | `config/runtime/intents.toml` | enum -> string allowlist |
| TS lexicon constants | `config/runtime/lexicon.toml` | aliases and asset allowlist externalized |
| `COS_CLASSIFIER` | `config/runtime/thresholds.toml` | complete classifier values |
| injection patterns | `config/runtime/injection.toml` | JS regex -> Rust regex review |
| TS eval inputs | `config/runtime/evals/inputs.json` | add expected pipeline action |
| response baselines | `config/runtime/evals/response-baseline.json` | new Rust-side artifact from recorded TS responses or approved live samples |

No hard-coded domain literals may remain in Rust core.

---

## 6. QA Gates Mapped to Migration

| Migration Phase | Required QA |
|-----------------|-------------|
| Phase 1 | config/registry unit tests |
| Phase 2 | input parity fixtures |
| Phase 3 | guardrail and answer policy unit tests |
| Phase 4 | audit/memory contract tests |
| Phase 5 | orchestrator integration tests |
| Phase 6 | pipeline eval gate |
| Phase 7 | TS/Rust parity report |
| Phase 8 | frontend endpoint smoke |
| Phase 9 | production metrics gate |
| Phase 10 | deprecation checklist |

See `.agent/artifacts/qa/2026-06-25-agent-runtime-rust-port/qa-plan.md`.

---

## 7. Rollback Plan

| Failure | Rollback |
|---------|----------|
| Rust boot config failure | fail startup in non-prod; keep feature flag off in prod |
| Rust runtime request errors | route feature flag back to old handler or TS runtime |
| SSE compatibility issue | frontend flag back to TS runtime |
| unknown/refusal rate spike | disable Rust answer policy path or rollback traffic |
| audit sink instability | switch sink to stdout/noop policy only if approved |
| live MCP/LLM issue | fallback to current `llm_connector` path or TS runtime |

Rollback must preserve audit evidence for the failed path.

---

## 8. Release Checklist

- [ ] Source repo baseline tag recorded.
- [ ] TS characterization fixtures exported.
- [ ] Rust config pack validates.
- [ ] `cargo fmt --check` passes.
- [ ] `cargo clippy --all-targets -- -D warnings` passes.
- [ ] `cargo test` passes.
- [ ] `cargo run --bin eval -- --pipeline-only` passes.
- [ ] Live smoke either passes or is explicitly waived.
- [ ] falcon-client sends `session_id` and `option_id`.
- [ ] SSE contract verified against existing UI parser.
- [ ] Production rollback flag verified.
- [ ] Deprecation plan for TS runtime approved.

---

## 9. Open Risks

| Risk | Severity | Mitigation |
|------|----------|------------|
| `.agent/project-manifest.md` absent | Medium | Add project onboarding before QA validate |
| Tool audit requires changing `llm_connector` internals | High | Implement internal frames before claiming full audit |
| TS regex semantics differ from Rust regex | High | Characterization fixtures for every pattern |
| Eval response quality depends on live LLM variance | Medium | Split pipeline-only CI from live/replay response eval |
| In-memory session not production durable | Medium | Treat as phase-one backend; keep Redis/Postgres seam |
| Frontend sends only free text today | Medium | Add `option_id` in falcon-client endpoint PR |

---

## 10. Migration Artifacts

| Artifact | Path |
|----------|------|
| PRD | `docs/agent-runtime-rust-port/prd.md` |
| Spec | `docs/agent-runtime-rust-port/spec/spec-overview.md` |
| File structure | `docs/agent-runtime-rust-port/file-structure.md` |
| QA plan | `.agent/artifacts/qa/2026-06-25-agent-runtime-rust-port/qa-plan.md` |
| Migration plan | `docs/agent-runtime-rust-port/migration-plan.md` |
