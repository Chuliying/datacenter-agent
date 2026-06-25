# datacenter-agent — System Context

## 專案概述

`datacenter-agent` 是一個 Rust 寫的 HTTP analytics API 服務。它透過 MCP（Model Context Protocol）連接到 datacenter 的 MCP server，取得 live 工具，再把工具呼叫轉給 LLM（OpenRouter），讓使用者可以用自然語言查詢資料中心的即時資料。

## 技術棧

| 層級 | 技術 |
|---|---|
| 語言 | Rust (Edition 2021) |
| HTTP 框架 | axum 0.8 |
| 非同步執行期 | tokio（multi-thread） |
| MCP client | rmcp 0.17（HTTP transport） |
| LLM | async-openai 0.40（OpenRouter API，支援 tool calling） |
| HTTP client | reqwest 0.13（rustls） |
| Middleware | tower + tower-http（CORS、tracing、gzip、timeout、rate limit） |
| 設定 | TOML（`config/config.toml`） + dotenv |
| CLI | clap 4（derive + env fallback） |
| Logging | tracing + tracing-subscriber（env-filter） |
| Allocator | mimalloc |
| 建置 | Cargo（release: `lto = "fat"`） |

## 專案架構

```
datacenter-agent/
├── src/
│   ├── main.rs              # 啟動流程：CLI 解析 → 載入 config → 連接 MCP → 建立 AppState → 啟動 axum
│   ├── lib.rs               # lib crate root，re-export 各模組
│   ├── appstate.rs          # AppState / LlmDefaults / PromptBank / token/url 載入
│   ├── config.rs            # AppConfig（TOML 反序列化）、prompt 路徑解析
│   ├── model.rs             # 資料模型（DTO 等）
│   ├── mcp_client.rs        # McpClient：連線、list_openrouter_tools、handle、shutdown
│   ├── llm_connector/       # LLM 呼叫邏輯（tool-calling loop、stream）
│   └── server/
│       ├── mod.rs           # build_router 組裝 axum Router + middleware
│       ├── route.rs         # 路由定義（/agent, /agent/stream, /greeting, /health, /ready）
│       ├── handler.rs       # 路由 handler 實作
│       ├── auth.rs          # Bearer token 驗證（constant_time_eq，418 on fail）
│       ├── dto.rs           # Request/Response DTO
│       ├── error.rs         # 錯誤型別與 IntoResponse
│       └── greeting.rs      # greeting 背景任務（spawn_greeting_tasks）
├── config/
│   ├── config.toml          # 頂層設定，prompt id → Markdown 路徑對應
│   └── prompt_guide/        # 實際 prompt Markdown 檔案
│       ├── agent_system.md
│       ├── greeting_system.md
│       └── greeting_user.md
├── tests/                   # 整合測試
├── docs/                    # 設計文件、migration plan、spec
├── .agent/                  # AI 開發流程設定
│   ├── project-manifest.md
│   ├── guardrails.md
│   ├── knowledge/           # 本目錄
│   └── skills/
│       ├── _shared/         # submodule: agent-playbook
│       └── project/         # 專案 domain skills
├── Cargo.toml
└── .env.example
```

## 核心設計決策

### MCP 整合
- 使用 `rmcp` crate 的 HTTP transport（不用 stdio），版本鎖定 0.17 以確保 handshake 相容性
- 啟動時透過 `list_openrouter_tools()` 取得工具清單，轉為 OpenRouter tool schema 後直接傳給 LLM
- `DATACENTER_MCP_URL` 指向本地或遠端 MCP server

### 認證
- 單一 `GLOBAL_TOKEN`，所有非 probe 路由都需要 `Authorization: Bearer <token>`
- 使用 `constant_time_eq` 防止 timing attack
- 418 (I'm a teapot) 作為 auth 失敗回應（刻意混淆）

### Prompt 系統
- `config.toml` 把 prompt id 對應到 Markdown 檔案
- 所有路徑相對於 `config.toml` 所在目錄，支援 container 掛載
- 啟動時載入到 `PromptBank`，之後 immutable 共享

### Greeting 系統
- 啟動時 spawn 3 個背景任務，各自跑完整的 tool-calling loop 產出 data-aware greeting
- `/greeting` 路由從預先產出的 greeting 中隨機選一個回傳

### Streaming
- `/agent/stream` 回傳 SSE（Server-Sent Events）token stream
- `async-stream` + `futures` 實作非同步 streaming

## 環境變數

| 變數 | 說明 |
|---|---|
| `OPENROUTER_API_KEY` | OpenRouter API 金鑰 |
| `OPENROUTER_BASE_URL` | API 基底 URL |
| `OPENROUTER_MODEL` | 模型名稱（必須支援 tool calling） |
| `OPENROUTER_APP_URL` | 回報給 OpenRouter 的 app URL |
| `OPENROUTER_APP_TITLE` | 回報給 OpenRouter 的 app 名稱 |
| `OPENROUTER_MAX_TOKENS` | LLM 最大 token |
| `OPENROUTER_TEMPERATURE` | LLM temperature |
| `DATACENTER_MCP_URL` | MCP server endpoint（含路徑，如 `/mcp`） |
| `HOST` | 綁定 host（預設 `0.0.0.0`） |
| `PORT` | 綁定 port（預設 `8080`） |
| `GLOBAL_TOKEN` | 全域認證 token |
| `RUST_LOG` | log filter（覆蓋 `--debug`） |

## API Endpoints

| Route | 說明 |
|---|---|
| `POST /agent` | 一次性 LLM 回答（完整 tool-calling loop） |
| `POST /agent/stream` | SSE token stream |
| `GET /greeting` | 隨機預產 greeting |
| `GET /health` | liveness probe |
| `GET /ready` | readiness probe |

非 probe 路由均需 Bearer token。
