## Project Identity

- `name`: `datacenter-agent`
- `type`: `Rust HTTP API Service / Analytics Agent`
- `description`: `透過 MCP 連接資料中心工具，搭配 OpenRouter 相容 LLM 回答自然語言查詢的 analytics agent`

## skill-commons bootstrap

- submodule_path: .agent/skills/_shared
- platforms: claude-code, codex
- profile: team-sprint optional

## Skill Roots

- `shared_skills_root`: `.agent/skills/_shared`
- `project_skills_root`: `.agent/skills/project`

## Core Documents

- `guardrails`: `.agent/guardrails.md`
- `system_context`: `.agent/knowledge/system-context.md`
- `api_reference`: `docs/reference/index.md`
- `architecture_map`: `.agent/knowledge/system-context.md`

## Core Code Entrypoints

- `types_entry`: `src/model.rs`
- `api_client_entry`: `src/mcp_client.rs`, `src/llm_connector/client.rs`

## Paths

- `source_roots`: src, config
- `tests_root`: tests
- `test_glob`: tests/**/*.rs
- `work_root`: docs/work
- `docs_root`: docs
- `legacy_artifacts_root`: .agent/artifacts（歷史保留；新 work item 使用 docs/work）

## Stack

- `test_cmd`: cargo test
- `typecheck_cmd`: cargo check
- `lint_cmd`: cargo clippy -- -D warnings
- `framework`: axum 0.8 / tokio / rmcp
- `package_manager`: cargo
- `source_extensions`: rs, toml, json, md
- `has_ui`: false
- `has_api`: true
- `typed_contracts`: true
- `has_e2e`: false
- `build_cmd`: cargo build --release
- `format_cmd`: cargo fmt

## Git Workflow

- `base_branch`: `main`
- `remote`: `origin`
- `branch_pattern`: `codex/<description>`
- `ticket_pattern`: N/A
- `commit_format`: conventional commits
- `integration_flow`: PR -> `main`
- `sprint_tracking`: `false`

## Domain Skill Names

目前尚未建立 project-level domain skills。

## Domain Skill Readiness

- 狀態：未建立；若後續要把 Rust handler、runtime pipeline、eval runner 等重複實作模式沉澱成 project skills，需先從目前程式碼取樣並引用真實檔案。
