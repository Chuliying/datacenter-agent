# Datacenter agent

<p align="center">
<img src="do-you-have-agent.jpeg" width="500" />
</p>

An analytics agent that answers questions about a datacenter by orchestrating an
LLM against live data with the power of MCP server.

## Endpoints

- `/agent`: one-shot answer
- `/agent/stream`: SSE oken stream
- `/greeting`: a random pre-generated, data-aware welcome message
- `/health`: liveness probe
- `/ready`: readiness probe

All non-probe routes require a bearer token, see Authentication section below.

## Authentication

A single `GLOBAL_TOKEN` loaded at startup gates every request via an `Authorization: Bearer <token>` header, provides basic safety so the upstream LLM API key won't be abused by some random weirdos.

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
