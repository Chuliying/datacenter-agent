# 模組：`llm_connector`

> ← 回 [模組總覽](./index.md) ｜ [專案主體](../index.md)
>
> **Source**：[`src/llm_connector/mod.rs`](../../../src/llm_connector/mod.rs)（“OpenRouter LLM connector with MCP tool-calling”）

## 職責
OpenRouter（OpenAI-compatible）LLM 連接 + MCP tool-calling 迴圈 —— agentic flow 的心臟。

## 子檔案

| 檔案 | 職責 |
|---|---|
| [`mod.rs`](../../../src/llm_connector/mod.rs) | 入口：`agent_stream` / `generate`；`LlmEvent` |
| [`agent.rs`](../../../src/llm_connector/agent.rs) | MCP tool-calling 迴圈（多輪：模型要工具→執行→回灌→再迴圈） |
| [`client.rs`](../../../src/llm_connector/client.rs) | 建 `async-openai` client（指向 OpenRouter） |

## 兩個入口

| 入口 | 說明 | Production 觸達方式（2026-08-17 核對） |
|---|---|---|
| `agent_stream(cfg, tools, mcp)` | 驅動迴圈，逐 token 串流最終答案 | 沒有直接呼叫者，但 `generate` 是它的 wrapper，所以**迴圈本身在 eval 時會跑** |
| `generate(cfg, tools, mcp)` | `agent_stream` 的便利包裝：收集所有 Token，`Done` 時回整段 Markdown | 只有 eval runner（`src/runtime/eval/runner.rs`） |

> ## ⚠ 本模組已不在任何 HTTP request path 上
>
> `agent_stream` 的**直接**呼叫點只有 [turn](./runtime-turn.md) 的 `LlmAgentPort::stream_turn`，
> 而 **`LlmAgentPort` 全 crate 從未被建構**（只有 struct 定義與 impl，沒有 `::new` 呼叫）；
> 加上 `run_agent_turn` 本身也 dormant，這條 `AgentPort` 路徑在 production 完全走不到。
>
> 但**不要因此把 `agent_stream` 當死碼**：`generate` 內部就是 `Box::pin(agent_stream(..))`
> （`src/llm_connector/agent.rs`），所以 eval 每跑一次，整個 tool-calling 迴圈與下面的
> terminal semantics 都會實際執行。
>
> 過去 `generate` 服務 `POST /agent` 與 greeting，兩者都已不再使用它：`/agent` 路由已移除，
> greeting 改走 [agent 層](./agent.md)的兩階段 pipeline（`build_greeting_pipeline`）。
> `/insight`、`/report`、`/agent/stream`、`/v1/chat/completions` 全部用 [agent 層](./agent.md)
> 自己的 `OpenAiLlm` / `StreamingOpenAiLlm` adapter。
>
> **結論：本模組目前只剩 eval CLI 一個真實使用者。** 下面的 terminal semantics 仍是正確的
> 程式行為，且確實會被執行——但只影響 eval，不影響任何線上端點。

## `LlmEvent`
`Token` / `Done` / `Error` / `Clear` / `ToolCalled{name,args_hash}` / `ToolResult{...}`。

`ToolCalled` / `ToolResult` 過去在 legacy 串流路徑被 handler 過濾不外送。該路徑已不存在——
現在沒有任何 handler 消費 `LlmEvent`，只有 `generate` 在 eval 內部把 `Token` 收集成字串。

## 與 runtime 的關係

設計上由 [turn](./runtime-turn.md) 的 `LlmAgentPort`（實作 `AgentPort`）包覆呼叫。
**該接線目前不存在**：`LlmAgentPort` 從未被建構，`run_agent_turn` 也沒有 production 呼叫者。
`server` 層完全不引用本模組（`src/server/` 內無任何 `llm_connector` 參照）。

## Terminal semantics（現況限制）

- provider final turn 只有明確 `finish_reason=stop` 才 emit Done；tool turn 只接受 `tool_calls`/deprecated `function_call` 且必須組出完整 tool call。
- natural EOF、`length`、`content_filter`、缺失/不相容 finish reason 都 emit Error，不保存 partial output。
- `generate` 若 event stream 結束但沒收到 Done/Error，仍 `Ok(out)`。
- tool call arguments 目前以 raw string寫入 info log；可能含敏感業務資料。
- MCP semantic error 的 `ok` 問題見 [mcp_client](./mcp-client.md)。

完成樣貌要求明確 finish/EOF contract、typed aborted/error 與去敏 log；見 [PRD FR-009](../prd.md)。

PRD FR-013 另要求把此tool-calling能力移到受控Capability Gateway/Evidence Hub階段；它不能原封不動作為Final LLM port。Final LLM只接收Prompt Builder產出的compiled prompt，不持有`ChatCompletionTool`或`McpHandle`。

## 相關
- 工具執行對象 → [mcp_client](./mcp-client.md)
- 設定來源 `GenerationConfig` → [appstate](../index.md#4-啟動與組裝top-level-接線)
