# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

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
