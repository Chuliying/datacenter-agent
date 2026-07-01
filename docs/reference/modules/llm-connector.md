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
- `agent_stream(cfg, tools, mcp)` — 驅動迴圈，逐 token 串流最終答案（給 [`/agent/stream`](../endpoints/agent-stream.md)）。
- `generate(cfg, tools, mcp)` — 同迴圈但等整段 Markdown 回覆（給 [`/agent`](../endpoints/agent.md) 與 greeting）。

## `LlmEvent`
`Token` / `Done` / `Error` / `Clear` / `ToolCalled{name,args_hash}` / `ToolResult{...}`。
其中 `ToolCalled`/`ToolResult` 在 legacy 串流路徑被 handler 過濾，不外送到 SSE。

## 與 runtime 的關係
runtime 模式下由 [orchestrator](./runtime-orchestrator.md) 的 `LlmAgentPort`（實作 `AgentPort`）包覆呼叫；runtime 關閉時由 [handler](./server.md) 直接呼叫。

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
