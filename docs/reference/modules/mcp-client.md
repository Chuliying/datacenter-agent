# 模組：`mcp_client`

> ← 回 [模組總覽](./index.md) ｜ [專案主體](../index.md)
>
> **Source**：[`src/mcp_client.rs`](../../../src/mcp_client.rs)（“The rmcp **client** side of the agent”）

## 職責
datacenter MCP server 的 rmcp **client**：以 HTTP transport 連線、執行 MCP `initialize` 握手、探索可用工具、轉發 tool call。

## 關鍵項
- `McpClient` —— 連線 + 握手。
- `McpHandle` —— 共享給 handler/orchestrator 的 peer handle（存於 `AppState`）。
- 握手期間 server 回傳 `instructions`（跨工具慣例），被附加到每個 system prompt（見 [appstate · generation_config](../index.md#4-啟動與組裝top-level-接線)）。

## 連線設定
- `DATACENTER_MCP_URL`（環境變數，指向 server 的 `/mcp` 端點）。
- rmcp 0.17，client-only、HTTP-only transport，版本與 server 對齊。

## Tool result semantics

`call_tool_text` 只把 transport/protocol call failure回 `Err`。若 MCP response `is_error == true`，它會記 warning但仍回 `Ok(text)`，讓模型有機會用錯誤文字自我修正。下游 `llm_connector` 對所有 `Ok(text)` emit `ToolResult { ok: true }`，因此目前 audit 的 tool success 可能不符合 MCP semantic status。

另外，connect error/context 與 startup banner/log 會包含 raw MCP URL；URL 若含 credential/query parameter 可能洩漏。Target behavior見 [PRD FR-007/FR-009](../prd.md)。

在PRD FR-013的target architecture中，`McpHandle`只能由Capability Gateway/Tool Hub持有，不得注入Final LLM port或Evidence Pack；MCP結果先被Evidence Hub正規化、標provenance/freshness/classification後才進Prompt Builder。

## 相關
- 工具被誰呼叫 → [llm_connector · agent loop](./llm-connector.md)
- 工具集 / instructions 存放 → [appstate](../index.md#4-啟動與組裝top-level-接線)
