## skill-commons bootstrap

- submodule_path: .agent/skills/_shared
- platforms: claude-code, codex
- delivery_mode: team-sprint
- capability_packs:

> `.agent/skills/_shared`（skill-commons v0.7.1）與生成的 `.claude/skills/`、`.codex/skills/` 依 ADR-001 由 `.gitignore` 排除，為本地 pinned clone；版本以此區塊與 onboarding 紀錄為準。

## Project Identity
- `name`: `datacenter-agent`
- `type`: `Rust HTTP API Service / Analytics Agent`
- `description`: `透過 MCP 連接資料中心工具，搭配 LLM（OpenRouter）回答自然語言查詢的 analytics agent`

## Skill Roots
- `shared_skills_root`: `.agent/skills/_shared`
- `project_skills_root`: `.agent/skills/project`

## Core Documents
- `guardrails`: `.agent/guardrails.md`
- `system_context`: `.agent/knowledge/system-context.md`
- `api_reference`: `.agent/knowledge/api-reference.md`（待補）
- `architecture_map`: `.agent/knowledge/system-context.md`（系統架構詳見 system-context.md）

## Core Code Entrypoints
- `types_entry`: `src/model.rs`（DTO 與資料模型）
- `api_client_entry`: `src/mcp_client.rs`（MCP client）、`src/llm_connector/client.rs`（LLM client）
- `design_tokens`: N/A（後端服務，無設計 token）

## Paths
- `tests_root`: `tests`
- `test_glob`: `tests/**/*.rs`
- `mockup_root`: N/A（後端服務）
- `prd_root`: `docs/reference`
- `spec_root`: `docs/reference/spec`
- `qa_root`: `docs/reference/tests`

## Stack
- `test_cmd`: `cargo test`
- `typecheck_cmd`: `cargo check`
- `lint_cmd`: `cargo clippy -- -D warnings`
- `e2e_cmd`: N/A
- `framework`: `axum 0.8 / tokio / rmcp 0.17`
- `build_cmd`: `cargo build --release`
- `format_cmd`: `cargo fmt`

## Capabilities
- `has_ui`: `false`（後端服務，無 UI）
- `has_api`: `true`（axum HTTP/SSE JSON API）
- `typed_contracts`: `true`（Rust DTO：`src/server/dto.rs`、`src/model.rs`）
- `has_e2e`: `false`（目前無 e2e harness）

## Git Workflow
- `base_branch`: `main`
- `remote`: `origin`
- `branch_pattern`: `<feature|fix|chore>/<description>`（待確認）
- `ticket_pattern`: N/A（待確認）
- `commit_format`: conventional commits（待確認）
- `integration_flow`: PR → main（待確認）

## Domain Skill Names
（目前尚未建立 domain skills，待 pattern discovery 完成後填入）

## Domain Skill Readiness
- 狀態：domain skills 尚未建立，需要先完成 pattern discovery
