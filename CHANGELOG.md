# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Fixed

- **Intent lexicon: 客單價 / 折扣率 / 用戶 questions no longer fall off-scope.** `intents.toml` only knew
  `營收 / 收入 / 賺多少` for revenue, so 「8 月的平均客單價與折扣率是多少？」 scored zero keywords and
  hit the rule-layer refusal (observed 2026-09-14: 4 of 30 acceptance questions refused this way).
  `revenue` gains 客單價 / 帳單 / 單價 / 折扣 / 折扣率, `charging` gains 充電次數 / 充電時數 / 使用率,
  `member` gains 用戶 / 充電用戶 / 新用戶.
- **`site-build` intent now requires `business_metrics` instead of nothing.** With an empty
  required-tool row the authz gate (`boot ∩ permission ∩ intent`) was always empty, so every
  站點建置 question — even from an admin — answered `authz.insufficient`, while `/greeting`
  advertised `site-build` as a capability to everyone (a tool-free intent "survived" the
  intersection). `business_metrics` carries the station / pile build-out series, so the intent
  now works for any role holding the opperf permission, and a page-only role is offered no topic
  it would then be refused. Marked CONFIRM? in `config.toml` for the domain owner.
- **Prompts: growth-rate labels and member counts.** `analyst` / `report_composer` /
  `greeting_analyst` gain a second figure-semantics section: `wow_growth` on monthly data is MoM
  (never 「WoW」 / 「YoY」), and the three member counts (cumulative, new, unique charging members)
  must not be conflated — a July report had printed 27,436 member-type orders as 活躍會員 while the
  August report used the 9,568 unique count.

### Fixed

- **streaming LLM: an empty, finish-less stream is now a stage failure, not a blank answer.**
  `StreamingOpenAiLlm::chat` returned `LlmResponse::Message("")` when the provider closed the
  stream with no content, no tool call and no `finish_reason` (how an upstream failure looks when
  the gateway keeps the HTTP 200 — keep-alive comments then close, a provider error never turned
  into a choice, an aborted relay). Every stage then "succeeded" on the blank and the browser got
  `token ""` + `done`; observed on dev 2026-09-14 as 0/10 data questions answered while all stage
  dots were green. `ensure_stream_completed` now rejects that shape with
  `AgentError::Capability("chat stream ended without a completion …")`, so the stage closes with
  `failure` and an `error` frame the UI can act on. A finish-less turn that did stream content or
  a tool call still passes (partial relay); an explicit `stop` on an empty turn is left alone.
### Changed
- **`/greeting` is scoped by Falcon permissions when `X-Falcon-Authorization` is sent.** The
  pre-generated welcome line quotes revenue, member and station figures; a role whose permissions
  do not unlock every greeting-fetcher tool (`[insight.grants].fetcher`) — e.g. a page-only
  `engproj` role — now receives a neutral, figure-free greeting instead, with `"scope": "neutral"`
  in the response (`"full"` otherwise). Callers that send no header keep the legacy global
  greeting. Found during the 2026-09-11 acceptance walk-through: a restricted account was refused
  `revenue` data on its first question but had already been told the month's revenue by the
  greeting. Decision helper `authz::greeting_scope_allows`; falcon-client forwards the user's
  access token on its greeting route from the same change set. The response also carries
  `capabilities` — the intents the caller can actually run (`authz::greeting_capabilities`) plus
  `ss-chat` when held — so the client can hide topics that would only be refused.
- **Report template no longer renders missing data as `0`.** When a report's periods carry no
  member (`newMembers` / `totalMembers` / `activeMembers`), infrastructure (`stations` /
  `chargers`) or charge (`kwh` / `sessions`) figures at all — the composer received no material
  for them — the KPI card shows `—` with `本期資料未提供` and the corresponding chart frame shows
  a text note instead of a flat zero line. Previously `累積會員數 0 人` and `0 站 / 0 樁` read as
  real figures next to a narrative that said the data was not provided.
  - `examples/render_report.rs` is now `examples/render_report_st.rs` (its output files are
  `report-st-N.*`), pairing with the new `render_report_ss`.
- `Renderer` takes the artifact key it injects, so both report pipelines share one pure-logic
  terminal stage. The distinct keys (`report.data` / `ss_report.data`) are the isolation guarantee:
  the two payload schemas are incompatible, so a mis-wired pipeline fails as a missing artifact
  rather than handing one template the other's data.

### Added
- **`POST /ss-report` — the 星星電力 HTML report agent.** The SS counterpart to `/agent/stream`'s
  report branch: the same four-stage `fetcher → analyst → composer → renderer` pipeline, over the
  six `ss_*` investor-platform tools, terminating in a `falcon-report` HTML block. It carries the
  identical identity layer, Falcon permission gate (`[authz].ss_chat_permissions`), rate limiters
  and answer-policy override as `/ss-chat/stream` — both routes reach the same data — and shares its
  memory-replay family, so multi-turn context crosses between the two. New `[ss_report]` config
  section (template + its own `[ss_report.grants]` ceiling, separate from `[ss_chat.grants]` so a
  report can widen without widening chat).
- **A section-shaped report protocol** (`agent::ss_report`) and its template
  (`config/report_template/ss_report.html`, eight charts, sharing the starcharger design tokens
  verbatim). Deliberately *not* a generalization of `ReportData`: four of the six SS tools return a
  current-stage snapshot with no time axis, so a `periods[]` spine would have meant inventing a
  month axis they do not have. Instead there is one section per tool, each with its own period
  label, and `report.periodLabel` / `report.snapshotLabel` are separate fields because the six tools
  do not share one window.
  - **An unfetched tool yields an empty section, which the template hides** — applying the lesson
    from the 0.5.2 chart fix directly. The SS grant is narrowed per caller by their Falcon
    permissions, so a partial report is the normal case; the composer is forbidden from zero-filling
    an absent topic and must name it in `report.dataNote`, which is the reader's only signal that a
    section is missing. Verified live: a narrow ask ("只要日照資料") fetches one tool, renders one
    section, hides five, and discloses all five by name.
  - **FTM construction progress is three flat vectors joined by project name**, not a nested
    `projects[].stages[]` tree — a minimal-reasoning composer fills flat tables reliably and
    truncates nested ones. The template re-groups them, and lists an orphaned row under its own
    heading rather than dropping it.
  - `wheeling.sites` is the one capped section (`ss_power_wheeling` returns 100+ rows); the cap is
    disclosed by `wheeling.totalRows`, and the wheeling KPI labels itself as covering only the shown
    rows rather than claiming a platform-wide total it derived from a subset.
- `examples/render_report_ss.rs` — writes a real `/ss-report` run to an HTML file for inspection,
  reporting per section whether the tool was called *and* whether the section got rows (a tool
  called but a section left empty being the interesting failure). `tests/ss_report_render.rs` pins
  the render contract offline: every field the template reads is emitted, every id it addresses
  exists, and an empty section survives to the page as an empty section.

## [0.5.2] - 2026-09-09

### Fixed
- **Prompts no longer let the model present cumulative build-out counts as the size of the
  running network.** The greeting, insight analyst and report composer prompts now spell out the
  `business_metrics` field semantics: only `total_running_stations` may be described as
  「充電網絡」/「全網站點」; `total_stations` (all-time created, `status IN (0, 1)`) and
  `total_piles` must be worded as 「累計建置」. Found in the 2026-09-11 acceptance cross-check:
  the greeting said 「充電網絡已擴展至 269 座站點」 while the dashboard's running count was
  212 (950 vs 720 piles). Prompt-only change; needs a live greeting regeneration to confirm.
- **Report sections rendered empty because the fetcher skipped tools.** The `/report` fetcher was
  told to "fetch only what the question needs", with a report caveat it had to infer from the
  user's wording — so it routinely stopped after four or five of its six granted tools. The topics
  it skipped had no data for the composer to transcribe, landed as `0`, and rendered as flat
  charts and an empty station-ranking table. The report pipeline now tells the fetcher outright
  that this is a report turn and that every granted tool must be called, and names the coverage the
  fixed template needs (revenue; kWh + sessions; new/total/active members; stations + chargers; the
  station ranking) rather than tool names, so a permission-narrowed grant still binds. Verified
  live: the previous behaviour skipped `station_revenue_ranking` in 2 of 3 runs; 8 of 8 runs are
  now complete. A new `#[ignore]`d harness, `tests/report_pipeline_coverage.rs`, asserts this
  end to end.
- **The tool-use loop's step cap could discard a nearly-complete fetch.** `MAX_STEPS` was 8, but a
  model calling six tools one per step needs seven before any retry — so a single rejected call
  cost the report a whole topic. The cap is now 16, and hitting it no longer fails a stage that
  produced what it was asked for: the artifacts gathered so far are returned (logged at `warn`)
  instead of taking the pipeline down with them.

### Changed
- **Tool rejections now tell the model what to fix.** A schema failure reported only
  ``schema: missing field `kwh` ``, with a line/column offset into a buffer the model never saw.
  Rejections now carry the JSON path (`periods[3].kwh`), echo the value actually sent there, state
  that nothing was recorded, and say what to do next. The required-output nudge — sent when a stage
  ends in prose without calling its sink — likewise names the tool that fills the slot, quotes the
  last rejection verbatim, and says how many attempts remain.
- **A datacenter transport failure is recoverable once.** An MCP call that failed at the transport
  used to abort the whole pipeline; the first failure of a given tool is now fed back to the model
  as a rejection explaining that the call returned no data and that the missing figures must not be
  invented. A second failure of the same tool is still fatal, so a genuinely unreachable server can
  never yield a report quietly composed from partial data.
- The composer is now told that a zeroed field is not the same as missing data: an absent topic must
  be named in `partialPeriodNote` rather than silently rendering as an empty chart.

## [0.5.1] - 2026-09-07

### Fixed
- `[authz.permission_tools]` and `[authz].ss_chat_permissions` now use Falcon's actual
  permission `code` form (`hdrenewables:elecsvc:starcharger:opperf`, colon-separated).
  The shipped config carried slash-separated keys copied from the local stub, so
  `authorize_pipeline` never matched a real user's codes and every request was refused
  with `authz.insufficient`. A config test pins the colon form; the dev stub and test
  fixtures follow the same shape.

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
