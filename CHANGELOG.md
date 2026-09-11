# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Changed
- **`/greeting` is scoped by Falcon permissions when `X-Falcon-Authorization` is sent.** The
  pre-generated welcome line quotes revenue, member and station figures; a role whose permissions
  do not unlock every greeting-fetcher tool (`[insight.grants].fetcher`) — e.g. a page-only
  `engproj` role — now receives a neutral, figure-free greeting instead, with `"scope": "neutral"`
  in the response (`"full"` otherwise). Callers that send no header keep the legacy global
  greeting. Found during the 2026-09-11 acceptance walk-through: a restricted account was refused
  `revenue` data on its first question but had already been told the month's revenue by the
  greeting. Decision helper `authz::greeting_scope_allows`; falcon-client forwards the user's
  access token on its greeting route from the same change set.
- **Report template no longer renders missing data as `0`.** When a report's periods carry no
  member (`newMembers` / `totalMembers` / `activeMembers`), infrastructure (`stations` /
  `chargers`) or charge (`kwh` / `sessions`) figures at all — the composer received no material
  for them — the KPI card shows `—` with `本期資料未提供` and the corresponding chart frame shows
  a text note instead of a flat zero line. Previously `累積會員數 0 人` and `0 站 / 0 樁` read as
  real figures next to a narrative that said the data was not provided.

### Fixed
- **Reports no longer render with blank charts.** `emit_report` only validated the JSON
  *shape*, so a payload whose `summary.latestCompletedPeriod` did not match any
  `periods[].period` (e.g. `2026-6` vs `2026-05`), or whose `periods` / `stationRanking`
  was empty, was accepted and injected into the template, where the client script threw
  before drawing a chart — header filled in, KPIs and all four charts blank.
  `ReportData::validate` now enforces the cross-field invariants the template depends on
  (non-empty arrays, `YYYY-MM` months oldest first, only the trailing month partial, the
  anchor month copied verbatim from a non-partial period, consecutive ranks) and rejects
  with a field-naming reason the model can act on; rejections are logged at `info` (still
  under the `agent::probe` target the repro harness filters on). `report.locale` must be a
  BCP-47 tag and the anchor must be the *most recent* complete month. The template shows a
  visible error banner instead of blank canvases if any invariant still fails, and no
  longer fabricates a "較上月 0.0%" kWh comparison when the anchor is the first month.
- **A report whose fetched data has no monthly or station figures now fails clearly instead
  of being fabricated or rendered blank.** The composer prompt tells the model not to
  invent entries; when it therefore never produces `report.data`, the report pipeline
  surfaces a stable `report.data_unavailable` code with user-facing copy — `/agent/stream`
  `error` frame `{"data": "<copy>", "code": "report.data_unavailable"}`, OpenAI endpoint
  `502` envelope `code: report.data_unavailable` — rather than the raw
  `missing artifact: report.data` upstream error. Falcon maps the code to its own copy
  (falcon-client branch `fix/report-data-unavailable-copy`). The required-output nudge in
  `run_llm_loop` no longer orders the model to call the tool unconditionally: it now says
  to restate what is missing rather than invent data when the material genuinely lacks it.

## [0.5.0] - 2026-09-07

### Added

- **BREAKING — Falcon end-user identity and RBAC on the prompt routes.** `/agent/stream`,
  `/ss-chat/stream` and `/v1/chat/completions` now require `X-Falcon-Authorization: Bearer
  <FALCON_ACCESS_TOKEN>` in addition to the service bearer; the runtime verifies it against
  Falcon's permissions endpoint (positive/negative TTL cache, whitelist 401 classification:
  only `auth.token_invalid` is refreshable), derives a pseudonymous `actor_key`
  (HMAC-SHA256 over a boot-required pepper), narrows every pipeline's tool grant to
  `boot ∩ permission ∩ intent-required` before any LLM/MCP call, filters session-memory
  replay by the caller's *current* permissions, and adds an inner per-actor rate-limit
  layer inside the (now mandatory) global one. Stable machine-readable `code` fields on
  all three error envelopes. Client-supplied `history` is ignored on identity-protected
  routes; `/v1/chat/completions` is single-turn.
  - **Client migration**: every caller of the three prompt routes must now send a second header
    alongside the service bearer. A request with only `Authorization` is refused `401`
    (`identity.header_missing`) before the pipeline is built. Callers that relied on sending
    `history` for multi-turn context must move to `session_id`; on `/v1/chat/completions` there is
    no multi-turn path in this release.
  - **Operator migration**: two new boot requirements, both fail-fast. `ACTOR_KEY_PEPPER` must be
    set and at least 32 bytes. The Falcon host must come from `FALCON_API_BASE_URL` or
    `[identity].base_url` — there is deliberately no checked-in default, so a deployment that
    forgets it cannot silently verify production tokens against whatever host the repo shipped.
    The global burst limiter is no longer opt-in. See `scripts/dev-falcon-stub.md` for a
    loopback permissions stub to test against.
- **`POST /ss-chat/stream`** — a streaming front door for the 星星電力 (SS) investor platform. It
  runs the same four-stage chat pipeline as `/agent/stream`'s insight path
  (`fetcher → analyst → charter → finalizer`) over the six `ss_*` MCP tools, with its own stage
  prompts (`ss_fetcher_system` / `ss_analyst_system` / `ss_charter_system`) and its own tool grant
  (`[ss_chat.grants]`). Same `AgentRequest` body, same SSE frame contract, same bearer gate and
  burst limiter as `/agent/stream`. The SS report pipeline is a later step.
  - **No intent filtering.** The runtime intent pack is the EV-charging one, so SS questions
    resolve to `unknown` and the configured `RuleAnswerPolicy` would refuse them as `off_scope`
    before the pipeline ran. This route substitutes the new `AlwaysAnswerPolicy`, which keeps the
    prompt-injection refusal but drops the scope gate. Intent still resolves, still emits
    `intent.resolved`, and is still audited — it just no longer gates the answer. Prompt-length
    validation, injection detection and audit are unchanged. Session memory flows through the
    same store, with SS turns tagged `ss_pipeline` so the identity slice's replay filter can
    authorize them via the SS permission gate instead of dropping their always-`unknown` intent;
    replay is route-family-scoped (SS turns replay only on this route, EV turns never do).
  - The SS fetcher grant is an **explicit list, never `"*"`**: one MCP server advertises both the
    EV-charging tools and the `ss_*` ones, so a wildcard would let the SS pipeline reach EV data
    under 星星電力 branding. Pinned by a unit test over both the shipped config and the in-code
    default, and by a tool assertion in the live test.
- `tests/ss_chat_pipeline.rs` — live integration test driving the production
  `build_ss_chat_pipeline` with the real config, over the seven manager-level questions from
  `eomc-mcp/docs/ss_chatbot_agent_test.md`. `#[ignore]`d; selectable with `SS_CHAT_QUESTION`.

### Changed

- `build_insight_pipeline` and the new `build_ss_chat_pipeline` now share one private
  `build_chat_pipeline` assembly, so a change to one chat pipeline's shape cannot silently skip the
  other. `agent_stream` and `ss_chat_stream` likewise share one `run_chat_stream` streaming body;
  each route supplies only its audit label, answer policy, and pipeline selector.

### Known limitations

- **`/ss-chat/stream` answer arithmetic depends on the configured model.** Verified against the
  live investor-platform API on 2026-08-27 over the seven manager questions in
  `eomc-mcp/docs/ss_chatbot_agent_test.md`. With `google/gemini-3.1-flash-lite` five of seven are
  correct — including both adversarial checks in Q7 (it refuses to add kWh to kW, and corrects
  「星火50」to「星展50計畫」unprompted) — but two show summation errors: Q3 totalled 73 cumulative
  rows to 185,555,555 against a tool-verified 259,159,762, and Q5 mis-added three correctly listed
  payments. The same prompts on `anthropic/claude-opus-5` reproduce the reference transcript
  exactly (259,159,762; top-5 85.4%; 大福+茂泓 56.6%), so this is a model-capability ceiling, not a
  pipeline or prompt defect. Point this endpoint at a stronger model if managers will quote its
  figures.
- `ss_analyst_system` carries hard rules for both failure classes (one aggregation basis per
  answer, a stated total that equals the rows shown, a single share denominator, and a per-row
  overdue procedure). These fixed a worse Q5 error — `gemini-3.1-flash-lite` had reported
  「無逾期項目」while 匯聚_柳營1.5MW was 27 days overdue — but cannot supply arithmetic the model
  lacks.
- An out-of-grant tool name aborts a stage outright rather than being fed back for correction
  (`run_llm_loop`, `src/agent/payload.rs`). Pre-existing and shared by both chat routes: one
  `/ss-chat/stream` run lost a complete analyst report because the *optional* charter stage
  invented a `default_chart` tool. Intermittent, and unchanged by this release.

## [0.4.0] - 2026-08-19

### Fixed

- Unmatched paths no longer reveal whether a bearer token is valid. `Router::merge` carries a
  sub-router's fallback with it, so the merged fallback was the OpenAI group's — wrapped in that
  group's auth layer. Any unmatched path therefore answered `401` without a token and `404` with a
  valid one, which made every path an oracle for token validity and made the retired paths' response
  depend on the `Authorization` header. An explicit outer `fallback` now answers a uniform `404`.
  Found by the first router-level test to reach the assembled router (see below).

### Added

- `src/test_support.rs`: test-only fixtures that stand a stub MCP server up over
  `tokio::io::duplex`, so tests can build a real `AppState` and serve `build_router` without a live
  MCP server. Behind `[dev-dependencies]`; the shipped binary still depends on rmcp as an HTTP
  client only.

### Removed

- **BREAKING**: Retired `POST /insight`, `POST /insight/stream`, `POST /report` and
  `POST /report/stream`. `/agent/stream` already reaches both sub-agent pipelines through
  intent routing, so the forced-pipeline variants were redundant; they were also the only
  prompt entry points that bypassed the runtime prelude (no guardrails, no injection
  detection, no audit trail). Migration: use `/agent/stream` for streaming or
  `/v1/chat/completions` for non-streaming. Retired paths now return `404`.
- **BREAKING**: Removed the `AgentResponse` DTO (`{user_prompt, model_response, intent}`) —
  it served only the retired non-streaming endpoints.
- Removed the handler-level 2 000-character prompt cap (`USER_PROMPT_LENGTH_CAP`). The prompt
  cap is now solely the runtime prelude's `thresholds.input.max_prompt_chars` (4 000).

### Changed

- `RUNTIME_ENABLED=false` no longer selects an alternative serving path: both prompt endpoints
  require the runtime and return `503`, leaving only `/health`, `/ready` and `/greeting`.
- Fixed the `/agent/stream` `503` body, which pointed callers at the now-retired endpoints.

## [0.3.0] - 2026-07-24

### Changed

- Add OpenAI-compatible endpoint in order to be wrapped inside unified agent platform.


## [0.2.2] - 2026-07-22

### Changed

- Prompt guides are now loaded at boot instead of being hard-coded at compile time.

## [0.2.1] - 2026-07-22

### Fixed

- Nested tool arguments no longer crash the argument parser, which previously
  broke the report pipeline.

## [0.2.0] - 2026-07-16

### Changed

- Separated the agent into dedicated subagents.

## [0.1.2] - 2026-06-06

### Added

- Modularize all system prompt templates.
- Add tool using capability, now the agent can fetch real data.
- Big refactoring, now the code has better quality.
- Set LICENSE.

## [0.1.1] - 2026-06-02

### Added

- Streaming mode.

## [0.1.0] - 2026-06-02

### Added

- First working example.
