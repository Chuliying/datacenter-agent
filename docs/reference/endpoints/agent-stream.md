# `POST /agent/stream` — 現況契約

> ← [Endpoints](./index.md)  
> **Source**：[`src/server/handler.rs`](../../../src/server/handler.rs) `agent_stream` / `agent_stream_runtime`；[`src/server/dto.rs`](../../../src/server/dto.rs) `StreamFrame`

## Request / response

- Request body 與 [`POST /agent`](./agent.md) 相同。
- Bearer required。
- 成功建立 stream 後為 `text/event-stream`。
- keep-alive interval 為 15 秒。

## SSE frame

每個 SSE `data:` 是一個 JSON object，discriminator 為 `event`。

| Event | JSON payload | Producer |
|---|---|---|
| `token` | `{"event":"token","data":"..."}` | legacy/runtime |
| `clear` | `{"event":"clear"}` | legacy/runtime tool preamble reset |
| `done` | `{"event":"done"}` | legacy/runtime clean completion |
| `error` | `{"event":"error","data":"..."}` | legacy/runtime |
| `intent.resolved` | `{"event":"intent.resolved","data":{"intent":"charging","candidateIntents":["charging"]}}` | runtime only，token 前 |

`ToolCalled`/`ToolResult` 不外送。`IntentResolvedData` 的 `candidate_intents` 透過 `rename_all = "camelCase"` 序列化為 `candidateIntents`。

## Legacy path

1. handler 先用 2000-char helper 驗證 prompt。
2. 建立 `llm_connector::agent_stream`。
3. `LlmEvent::{Token,Done,Error,Clear}` 映射成 `StreamFrame`；tool metadata 過濾。

空／超長 prompt 在 stream 建立前回 HTTP 400。

## Runtime path

1. handler 建立 `tokio::sync::mpsc::unbounded_channel`。
2. `tokio::spawn` 執行 `run_agent_turn`。
3. emit closure 將 `TurnEvent` 送進 channel，send result 被忽略。
4. SSE body drain receiver。
5. channel 關閉後，只在 join 結果為 `Ok(Err(err))` 時補 error frame；`Err(JoinError)` 沒有映射。

因 prompt validation 位於 spawned orchestrator 內，runtime 空／>4000 prompt 已經取得 HTTP 200 SSE response，之後才收到 `error` frame。這與 legacy 及 runtime REST 不同。

## Lifecycle limitations

- channel 無界，slow consumer 沒有 backpressure。
- client disconnect 會 drop response-side future/JoinHandle；drop JoinHandle 只 detach task，不代表 producer/upstream 被取消。
- send failure 被忽略，producer 可繼續呼叫 LLM/MCP、memory、audit。
- 120 秒 Router timeout 只限制 handler 建立 Response 前；不限制後續 SSE body/turn。
- provider stream natural EOF without finish reason 可能被底層 connector emit 為 Done。
- `Aborted` outcome 沒有專屬外部 event；stream 是否已有 terminal frame取決於 upstream frames。

這些是現況限制。PRD 的完成樣貌要求 bounded channel、disconnect cancellation、deadline 與單一 terminal outcome；見 [PRD FR-008](../prd.md) 與 [plan I01](../../../.agent/artifacts/plan/2026-06-29-runtime-correctness/implementation.md)。

## Example

```bash
curl -N http://localhost:8080/agent/stream \
  -H "Authorization: Bearer $GLOBAL_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"prompt":"列出本週充電量前五名","session_id":"abc"}'
```

## Test evidence

- DTO serialization：`tests/runtime_contract.rs`。
- event mapping：handler module tests。
- orchestrator ordering：fake `AgentPort` component test。
- 未覆蓋：Router-level status、slow consumer、disconnect、JoinError、live adapter truncation。
