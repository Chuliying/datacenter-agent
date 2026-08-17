# 模組：`runtime::turn`

> ← 回 [模組總覽](./index.md) ｜ [專案主體](../index.md)
>
> **Source**：[`src/runtime/turn.rs`](../../../src/runtime/turn.rs)

## 職責
一個 agent turn 的編排骨幹（`run_agent_turn`），把輸入處理 → 防護決策 → 記憶注入 → LLM/MCP 迴圈 → 回寫記憶/稽核串成單一流程。**只依賴 trait**，不綁具體實作。

> ## ⚠ `run_agent_turn` 目前 dormant
>
> **`run_agent_turn` 沒有任何 production 呼叫者**（2026-08-17 核對）。全 crate 的呼叫點都在
> `src/runtime/turn.rs` 的 `#[cfg(test)] mod tests` 內。
>
> 實際在 request path 上的只有它的同步前段 **`plan_stream_turn`**：
> [`/agent/stream`](../endpoints/agent-stream.md) 與
> [`/v1/chat/completions`](../endpoints/chat-completions.md) 呼叫它跑 prelude，
> 拿到 `StreamPlan` 後**自己**驅動 [sub-agent pipeline](./agent.md)，並自行複製 turn 的兩個
> post-stream 副作用（audit + memory append）。
>
> 連帶地 `trait AgentPort` / `LlmAgentPort` / `AgentTurnOutcome` / `TurnEvent` 在 production 也
> 不再被走到——`/agent/stream` 傳的是 `UnusedAgentPort` 與 no-op emit。
> 下面「流程（runtime 模式）」第 6、7 步描述的是 `run_agent_turn` 的設計，不是目前任何
> 端點的實際行為。
>
> 這是 sub-agent 層引入後留下的結構缺口，對應 plan §9（把 pipeline 收到 `AgentPort` 後面）。

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

## REST vs SSE host 差別（歷史；目前無 production REST turn）

原始設計：核心 `run_agent_turn` 相同，REST 傳 no-op emit 並讀 outcome，SSE 用 channel emit
live frames。

**現況已不同**。`POST /agent` 路由已移除，沒有任何端點走 REST turn。唯一的 SSE host
（`/agent/stream`）改用 `plan_stream_turn` prelude，且：

- validation 在 prelude 內、**建立 stream 之前**完成，錯誤回正確 HTTP status
  （不再是先回 200 再送 error frame）。
- channel 是**有界**的（`INSIGHT_STREAM_BUFFER = 8192`，`try_send`，有損），
  不是原設計的 unbounded。
- client disconnect / JoinError cancellation 仍未完整處理。

## 已知 terminal gaps

- `LlmAgentPort` 依賴 connector 的 Done/Error；connector 會把 natural EOF、length/content-filter 與不相容 finish reason 轉成 Error。
- `stream_agent_response` 收到 frames 全部結束且沒有 terminal frame時回 `Aborted`，但 `Aborted` return path 沒有 completed/failed terminal audit。
- SSE host drop/disconnect 不會明確取消 producer task。
- `TurnEvent` 沒有獨立 `Cancelled`/`Aborted` variant。

## 相關
- handler 如何呼叫 → [`/agent/stream`](../endpoints/agent-stream.md) ·
  [`/v1/chat/completions`](../endpoints/chat-completions.md)（兩者都只用 `plan_stream_turn`
  prelude，之後自己驅動 [sub-agent pipeline](./agent.md)，不走完整 `run_agent_turn`）
- 組裝來源 → [registry](./runtime-registry.md)
- 型別定義 → [schema](./runtime-schema.md)
- Target lifecycle → [PRD FR-008/FR-009](../prd.md) · [code change plan](../../../.agent/artifacts/plan/2026-06-29-runtime-correctness/implementation.md)
