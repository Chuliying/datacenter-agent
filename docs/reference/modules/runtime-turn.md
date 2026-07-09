# 模組：`runtime::turn`

> ← 回 [模組總覽](./index.md) ｜ [專案主體](../index.md)
>
> **Source**：[`src/runtime/turn.rs`](../../../src/runtime/turn.rs)

## 職責
一個 agent turn 的編排骨幹（`run_agent_turn`），把輸入處理 → 防護決策 → 記憶注入 → LLM/MCP 迴圈 → 回寫記憶/稽核串成單一流程。**只依賴 trait**，不綁具體實作，故 REST 與 SSE 共用同一條編排。

## 關鍵型別／介面

| 項目 | 說明 |
|---|---|
| `run_agent_turn(input, ctx, deps)` | 編排入口 |
| `AgentTurnDeps` | 注入依賴：`runtime_config` / `input_pipeline` / `answer_policy` / `llm_normalizer` / `sessions` / `agent` / `audit` / `emit` |
| `trait AgentPort` | agent 傳輸 port：`stream_turn` → `BoxStream<AgentTurnFrame>` |
| `LlmAgentPort` | `AgentPort` 的實作，包覆 [llm_connector](./llm-connector.md) |
| `AgentTurnOutcome` | `Final{response,intent}` / `Refused{reason,copy}` / `Aborted{response}` / `Error{code,status}` |
| `TurnEvent` | live 串流事件：`IntentResolved` / `Token` / `Clear` / `Done` / `Error` |
| `StreamPlan` | turn 同步前段的結果（在送任何 token 前決定 error/refuse/放行） |

## 流程（runtime 模式）
1. 結構防護 `validate_prompt`（空／超長 → pre-stream `Error`）。此為 turn 的獨立前置步，**不在** input pipeline 內。
2. [input pipeline](./runtime-input.md) `run_with_config`：實際跑 `normalize → injection guard → intent → slots`（`injection` 偵測已接入此階段，命中時附加 `prompt_injection_detected` warning，見 [guardrails](./runtime-guardrails.md)）。注意：config 的 `input_stages` 仍是**宣告性 metadata**，pipeline 目前依然**未**依其動態分派階段順序——順序是寫死的，不是資料驅動的。
3. 可選 [llm_normalizer](./runtime-llm-normalizer.md) 補強低信心。
4. [answer policy](./runtime-guardrails.md) 決策：拒絕／提示／放行。injection warning 會被 answer policy 轉成 `Refuse("prompt_injection")`，在呼叫上游 LLM 前攔截；該類拒絕不會寫入 session memory。
5. [session memory](./runtime-memory.md) 注入 context。
6. 經 `AgentPort` 跑 LLM/MCP 迴圈，逐幀 `emit`（`Clear` → 清答案 buffer）。
7. 回寫 memory + 寫 [audit](./runtime-audit.md) 各決策點。

## REST vs SSE host 差別
核心 `run_agent_turn` 相同；REST 傳 no-op emit 並讀 outcome，SSE 用 channel emit live frames。但 host lifecycle 不只差 sink：runtime SSE 在 spawned task 內才做 validation、使用 unbounded channel，且 client disconnect/JoinError cancellation 沒有完整處理。外部 status 與資源生命週期因此和 REST 不同。

## 已知 terminal gaps

- `LlmAgentPort` 依賴 connector 的 Done/Error；connector 會把 natural EOF、length/content-filter 與不相容 finish reason 轉成 Error。
- `stream_agent_response` 收到 frames 全部結束且沒有 terminal frame時回 `Aborted`，但 `Aborted` return path 沒有 completed/failed terminal audit。
- SSE host drop/disconnect 不會明確取消 producer task。
- `TurnEvent` 沒有獨立 `Cancelled`/`Aborted` variant。

## 相關
- handler 如何呼叫 → [`/agent`](../endpoints/agent.md) · [`/agent/stream`](../endpoints/agent-stream.md)
- 組裝來源 → [registry](./runtime-registry.md)
- 型別定義 → [schema](./runtime-schema.md)
- Target lifecycle → [PRD FR-008/FR-009](../prd.md) · [code change plan](../../../.agent/artifacts/plan/2026-06-29-runtime-correctness/implementation.md)
