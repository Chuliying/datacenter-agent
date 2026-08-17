# 模組功能（Modules）總覽

> ← 回 [專案主體](../index.md)
>
> **Source**：[`src/lib.rs`](../../../src/lib.rs)（crate 根）、[`src/runtime/mod.rs`](../../../src/runtime/mod.rs)（runtime 樹）、[`src/agent/mod.rs`](../../../src/agent/mod.rs)（sub-agent 樹）

## Crate 結構

```
src/
├── main.rs            # 進入點（見 專案主體）
├── lib.rs             # crate 根
├── appstate.rs        # 程序級共享狀態（見 專案主體）
├── config.rs          # 頂層 config.toml 載入（含 InsightGrants）
├── model.rs           # 根層資料模型
├── bin/
│   └── eval.rs        # 第二個 binary target：eval CLI（見 eval）
├── server/            # → server 模組（含 openai.rs）
├── agent/             # → agent 模組（sub-agent 層：pipeline 編排）
├── runtime/           # → runtime 各子模組（領域無關核心）
├── llm_connector/     # → llm_connector 模組
└── mcp_client.rs      # → mcp_client 模組
```

## 模組索引（每模組一頁）

### HTTP 層
| 模組 | 職責 | 子頁 |
|---|---|---|
| `server` | axum 路由、middleware、雙 auth、handler、DTO、錯誤、OpenAI 相容層、greeting 任務 | [server](./server.md) |

### Sub-agent 層（[`src/agent/`](../../../src/agent/mod.rs)）
| 模組 | 職責 | 子頁 |
|---|---|---|
| `agent` | payload / tool / sub-agent 三份 contract 的移植；`/insight`、`/report`、greeting 三條 pipeline 的組裝與 production wiring | [agent](./agent.md) |

### Runtime 核心（[`src/runtime/`](../../../src/runtime/mod.rs)，與 HTTP 解耦）
| 模組 | 職責 | 子頁 |
|---|---|---|
| `turn` | `plan_stream_turn`（同步 prelude，**production 唯一入口**）。完整的 `run_agent_turn` 連同 `AgentPort` / `TurnEvent` 目前 **dormant**，只有測試呼叫 | [turn](./runtime-turn.md) |
| `input` | 決定性輸入 pipeline（normalize/intent/slots） | [input](./runtime-input.md) |
| `llm_normalizer` | 可選 LLM-backed 輸入正規化 seam | [llm_normalizer](./runtime-llm-normalizer.md) |
| `guardrails` | input guard + injection detector + config-driven answer policy | [guardrails](./runtime-guardrails.md) |
| `memory` | partial：in-memory session store + context；actor 未接線 | [memory](./runtime-memory.md) |
| `audit` | partial：事件/sink；production redaction/actor 未接線 | [audit](./runtime-audit.md) |
| `registry` | partial：部分 config ID → trait object；多組 ID 只驗證 | [registry](./runtime-registry.md) |
| `config` | 能力包 config 載入 + validate | [config](./runtime-config.md) |
| `schema` | 共享 runtime 型別 | [schema](./runtime-schema.md) |
| `error` | runtime 錯誤模型 | [error](./runtime-error.md) |
| `eval` | partial：pipeline/replay/live runner；process gate 已接線，evaluator semantics 仍有缺口 | [eval](./runtime-eval.md) |

### 外部連接
| 模組 | 職責 | 子頁 |
|---|---|---|
| `llm_connector` | OpenRouter LLM + MCP tool-calling 迴圈。**現況只剩 eval CLI 使用**，不在任何 HTTP request path 上；`AgentPort` 接線（`LlmAgentPort`）dormant | [llm_connector](./llm-connector.md) |
| `mcp_client` | datacenter MCP server 的 rmcp client | [mcp_client](./mcp-client.md) |

## 依賴流向（高層）

目前有**兩套平行的編排層**，差別在有沒有經過 runtime turn。

```
                       ┌─ /insight, /insight/stream ─┐
                       │  /report,  /report/stream   │   直接驅動，繞過 runtime
server ────────────────┴─────────────────────────────┴──▶ agent::wiring
   │                                                          │
   │                                                          ├─▶ agent::engine (Orchestrator)
   │                                                          ├─▶ agent::pipeline (4 stages)
   │                                                          ├─▶ agent::tools ─▶ mcp_client
   │                                                          └─▶ agent::llm ─▶ OpenRouter
   │
   │  /agent/stream, /v1/chat/completions           經過 runtime prelude
   └──▶ runtime::turn::plan_stream_turn ──┬─▶ input ─▶ llm_normalizer(可選)
                │                          ├─▶ guardrails
                │                          ├─▶ memory
                │                          └─▶ audit
                └── 依 resolved intent 選 pipeline ──▶ agent::wiring（同上）

runtime::eval::runner ──▶ llm_connector::generate ──▶ agent_stream ──▶ mcp_client
                          （eval CLI，本模組唯一真實使用者；generate 是 agent_stream 的 wrapper）

[dormant] run_agent_turn ──▶ LlmAgentPort ──▶ llm_connector::agent_stream
          ↑ 只有測試呼叫      ↑ 從未被建構

AppState 於啟動時組裝：MCP handle、tools、LLM defaults、PromptBank、
InsightGrants、report_template、optional AppRuntime
```

重點：

1. **直接 pipeline 端點完全不碰 runtime**——沒有 guardrails、intent、memory、audit。
2. `/agent/stream` 與 `/v1/chat/completions` 用 runtime prelude 做前處理，**再**進 sub-agent 層；
   它們不使用 `run_agent_turn` 的完整 AgentPort 路徑。
3. `llm_connector` 已退出所有 HTTP request path，只剩 eval runner 一個使用者。
   `agent_stream` 的**直接**呼叫點 `LlmAgentPort` 從未被建構，但 `generate` 是它的 wrapper，
   所以迴圈本身在 eval 時仍會執行——不要當成純死碼。
   sub-agent 層有自己的 `agent::llm` adapter。
4. `run_agent_turn`（連同 `AgentPort` / `TurnEvent` / `AgentTurnOutcome`）**dormant**——
   production 只走它的同步前段 `plan_stream_turn`。

> runtime 核心**不依賴** axum / server DTO。trait seams 已存在，但「有 trait」不代表所有
> config module 已可拔插；成熟度以各 module page 的 production wiring 為準。
>
> 同理，sub-agent 層目前也**不在** runtime 的 trait seam 之後——把它收到 `AgentPort` 後面
> 是 plan §9 的未完成工作。
