# Datacenter agent

<p align="center">
<img src="do-you-have-agent.jpeg" width="500" />
</p>

An analytics agent that answers questions about a datacenter by orchestrating an
LLM against live data with the power of MCP server.

> 📚 **System reference docs** (architecture, every endpoint and module, with source anchors): [`docs/reference/index.md`](docs/reference/index.md).

## Endpoints

- `/agent/stream`: SSE stream — the native front door for the EV-charging (EOMC) tools. Runs the
  runtime prelude, then routes to the insight or report sub-agent pipeline by resolved intent.
- `/ss-chat/stream`: SSE stream — the same four-stage chat pipeline over the 星星電力 investor-platform
  (`ss_*`) tools, with its own stage prompts and tool grant. Same request/frame contract as
  `/agent/stream`; intent filtering is off (the intent pack is EV-charging-specific), prompt-injection
  refusal, audit and prompt caps are unchanged; session memory is written with a per-pipeline
  tag so multi-turn context survives the identity slice's replay filter. See
  [ss-chat-stream](docs/reference/endpoints/ss-chat-stream.md).
- `/ss-report`: SSE stream — the 星星電力 **HTML report** front door. Same six `ss_*` tools and the
  same Falcon permission gate as `/ss-chat/stream`, but always the four-stage
  `fetcher → analyst → composer → renderer` report pipeline, terminating in a `falcon-report` HTML
  block. Its payload is section-shaped (one section per tool), so a tool the caller's permissions do
  not cover simply yields no section rather than a chart of zeros. See
  [ss-report](docs/reference/endpoints/ss-report.md).
- `/v1/chat/completions`: OpenAI-compatible (agentgateway Path C), streaming and non-streaming.
- `/greeting`: a random pre-generated, data-aware welcome message. Send `X-Falcon-Authorization`
  as well and the greeting is scoped to that user: roles whose permissions do not unlock every
  greeting-fetcher tool get a neutral greeting with no figures (`"scope": "neutral"`).
- `/health`: liveness probe
- `/ready`: readiness probe

`POST /agent`, `/insight`, `/insight/stream`, `/report` and `/report/stream` have been retired;
they return `404`. Use `/agent/stream` (streaming) or `/v1/chat/completions` (non-streaming).

All routes, including `/health` and `/ready`, require the service bearer token. The four prompt
routes (`/agent/stream`, `/ss-chat/stream`, `/ss-report`, `/v1/chat/completions`) additionally
require a Falcon end-user token — see [Authentication](#authentication). See
[the endpoint contract](docs/reference/endpoints/index.md) for middleware and probe caveats.

## Authentication

Two layers, checked in order.

**1. Service bearer — every route.** A single `GLOBAL_TOKEN` loaded at startup gates every request
via an `Authorization: Bearer <token>` header, provides basic safety so the upstream LLM API key
won't be abused by some random weirdos. The failure response is `418 I'm a teapot` on the standard
routes, and `401` with the OpenAI error envelope on `/v1/chat/completions`.

**2. Falcon end-user identity — the four prompt routes.** `/agent/stream`, `/ss-chat/stream`,
`/ss-report` and `/v1/chat/completions` additionally require `X-Falcon-Authorization: Bearer <FALCON_ACCESS_TOKEN>`.
The runtime verifies it against Falcon's permissions endpoint, derives a pseudonymous `actor_key`
from it, and narrows the pipeline's tool grant to `boot ∩ permission ∩ intent-required` **before**
any LLM or MCP call — so a caller only ever reaches the data their Falcon permissions cover. A
missing header is `401 identity.header_missing`; probes are unaffected, and `/greeting` treats the
header as optional (present → permission-scoped greeting, absent → the global one).

**Which Falcon?** One runtime serves every Falcon environment (the dev / stage / prod front-ends
all call the same agent), but a token only verifies against the Falcon that issued it. The BFF
therefore also sends `X-Falcon-Api-Base-Url: <its own API_BASE_URL>` on those routes and on
`/greeting`. The value must normalize to an entry of the boot-time allowlist —
`[identity].allowed_base_urls` in `config/config.toml` (shipped with dev / stage / prod), or
`FALCON_API_ALLOWED_BASE_URLS` as a comma-separated override — otherwise the request is refused
with `401 identity.falcon_host_unknown` **before** the token is sent anywhere (`/greeting` falls
back to the neutral greeting). Without the header the default host is used, so older consumers
keep working. One permissions client and cache is built per host, and the `actor_key` is derived
per host (`v2:` scheme), so the same `user_id` on two Falcons never shares memory or a rate-limit
bucket.

This means the service bearer alone is no longer enough to reach a pipeline. Boot also now requires
`ACTOR_KEY_PEPPER` (≥32 bytes) and a default Falcon host from `FALCON_API_BASE_URL` or
`[identity].base_url` — there is no default host on purpose. For local testing, `scripts/dev-falcon-stub.md` stands up a
loopback permissions endpoint.

The target authentication/CORS/probe policy is tracked, with build status, in the
[runtime platform PRD](docs/reference/prd.md).

## Runtime status

The config-driven runtime is the request authority for both prompt endpoints. `RUNTIME_ENABLED=false` (or `0`) no longer selects an alternative serving path — it leaves only `/health`, `/ready` and `/greeting` serviceable, since `/agent/stream` and `/v1/chat/completions` both return `503` without the runtime. Request-path injection detection, config-driven answer thresholds, and regression exit gating are wired, while configurable stage dispatch, reliable SSE cancellation, and evaluator implementations still have gaps. The [system reference](docs/reference/index.md) is the current implementation truth; the [PRD](docs/reference/prd.md) describes the completed target and marks unfinished requirements.

## Config & modularized system prompts

We designed a single top-level `config.toml`, which binds prompt ids to actual Markdown files (e.g. `config/prompt_guide/*.md`).


The whole `config/` folder designed to be self-contained, with paths resolved relative to the config, so container mounting will be lot more easier -- just mount the volume and use `--config` argument to point to the top-level config.

Prompts are loaded once into a shared prompt bank.

## ~~Heartwarming~~ greeting

A few background tasks spawn in boot time will run the greeting prompt through the same tool-calling loop to pre-generate data-aware welcome messages.

`/greeting` picks a random one and return.

## Acknowledgments

Portions of this codebase were generated with the assistance of Claude Opus 4.8. The human developers maintain full authorship and have conducted rigorous testing, refactoring, and validation of the final codebase.

## Changelog

See [CHANGELOG.md](CHANGELOG.md) for project change history and release notes.

## License

Licensed under the Apache License, Version 2.0. See [LICENSE](LICENSE) for details.
