# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

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
