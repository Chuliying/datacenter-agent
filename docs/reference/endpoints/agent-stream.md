# `POST /agent/stream` — 現況契約

> ← [Endpoints](./index.md)  
> **Source**：[`src/server/handler.rs`](../../../src/server/handler.rs) `agent_stream` / `wants_report_pipeline` / `insight_frames` / `status_to_app_error`；[`src/runtime/turn.rs`](../../../src/runtime/turn.rs) `plan_stream_turn` / `append_memory_turn_if_enabled`；[`src/server/dto.rs`](../../../src/server/dto.rs) `StreamFrame`

**production 路由路徑**：跑完整 runtime turn（guardrails → intent → answer policy → memory → audit），
再依 resolved intent 挑選 sub-agent pipeline 串流。

## 與直接 pipeline 端點的差別

[`/insight/stream`](./insight-stream.md) 與 [`/report/stream`](./report-stream.md) 各自固定驅動
**一條** pipeline 並繞過 runtime。本端點是**被路由的**那一條：它原樣重用 runtime 的
`plan_stream_turn` prelude（沒有複製任何 guardrail / intent 邏輯），然後把選中的 pipeline
透過**同一個** `insight_frames` 映射串出去——所以 stage-aware SSE 契約三者完全一致。

### Pipeline 選擇規則

`wants_report_pipeline`：只要 `report` intent **被提及**——是 resolved top intent，或出現在
candidate intents 之中——就走 `/report` pipeline，否則走 `/insight`。

用「被提及」而非嚴格 top-1 的理由：像「營收報告」這種 topic + report 的 prompt 仍會路由到
report pipeline，同時它的 topic intent（`revenue`）繼續驅動 answer policy 與 memory。

### Runtime 必要

需要 runtime 啟用（`RUNTIME_ENABLED`，預設 on）。rollback 時回 `503`，
呼叫端應改用 `/insight/stream` 或 `/report/stream`。

> 舊的 legacy 路徑（`llm_connector::agent_stream` 直呼）**已不存在於本端點**。
> `llm_connector` 現在只在 runtime turn 的 `LlmAgentPort` 與 eval runner 上，
> 見 [llm_connector](../modules/llm-connector.md)。

## Request / response

- Request body 為 `AgentRequest`，與 [`/insight`](./insight.md) 同形。
- Bearer required（失敗 418）。
- 成功建立 stream 後為 `text/event-stream`，keep-alive interval 15 秒。
- prompt cap 為 runtime config `thresholds.input.max_prompt_chars`（目前 4 000），
  非直接 pipeline 端點的 2 000。

## Prelude 的三種結果

`plan_stream_turn` 回傳的 `StreamPlan` 決定後續：

| Plan | 外部行為 |
|---|---|
| `Error{code,status}` | **在建立 stream 前**回 HTTP status（4xx → 400 系，其餘 → 503）。audit 已記錄 |
| `Refused{copy}` | 回 200 SSE，串出 refusal copy 當作整個答案，然後 `done`。audit + memory 已寫入 |
| `Proceed` | 進入 pipeline 串流 |

> **這是與舊文件不同之處。** pre-stream validation 錯誤現在回**正確的 HTTP status**，
> 不再是「先回 200 再送 error frame」。空／超長 prompt 的行為因此與 REST 路徑一致。

## SSE frame

與 [`/insight/stream`](./insight-stream.md#sse-frame) 相同的 `StreamFrame` 集合
（`stage` / `token` / `tool_call` / `tool_args` / `usage` / `clear` / `done` / `error`），
另加本端點獨有的一種：

| Event | JSON payload | 時機 |
|---|---|---|
| `intent.resolved` | `{"event":"intent.resolved","data":{"intent":"revenue","candidateIntents":["revenue"]}}` | **僅 `Proceed` 路徑**：任何 token 之前，送一次 |

> `Refused` 路徑**不送** `intent.resolved`——它直接 yield refusal copy 的 `token` 再 `done`。
> 消費端不可假設每次 200 回應都以 `intent.resolved` 開頭。

`IntentResolvedData` 的 `candidate_intents` 透過 `rename_all = "camelCase"` 序列化為
`candidateIntents`，對齊前端事件形狀。

終局協定同樣是 `clear` → `token`（完整答案）→ `done`。

## Post-stream 副作用

串流結束後複製 runtime turn 的兩個副作用：

1. **Audit**：成功寫 `ResponseCompleted`、失敗寫 `ResponseFailed`（帶 `error_code`、`duration_ms`）。
2. **Session memory**：`append_memory_turn_if_enabled`，用保留了 `raw_input` 的 `agent_input`。

兩者失敗都只 `warn!` 記錄，不影響已送出的串流。

## Lifecycle 現況與限制

- event channel 為有界 buffer（`INSIGHT_STREAM_BUFFER = 8192`），`try_send` 投遞，**有損**。
- 120 秒 Router timeout 只限制 handler 建立 Response 之前；不限制之後的 SSE body/turn。
- client disconnect 會 drop response-side future；不代表 upstream pipeline 被取消，
  producer 可能繼續呼叫 LLM/MCP。
- provider natural EOF／token-limit truncation 由底層 connector 轉成 Error，不 emit Done。

PRD 的完成樣貌要求 bounded-and-lossless channel、disconnect cancellation、deadline 與單一
terminal outcome；見 [PRD FR-008](../prd.md) 與
[plan I01](../../../.agent/artifacts/plan/2026-06-29-runtime-correctness/implementation.md)。

## Example

```bash
curl -N http://localhost:8080/agent/stream \
  -H "Authorization: Bearer $GLOBAL_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"prompt":"列出本週充電量前五名","session_id":"abc"}'
```

## Test evidence

- DTO serialization：`tests/runtime_contract.rs`。
- event mapping、`wants_report_pipeline`、`fold_history_into_prompt`：handler module tests。
- prelude ordering：fake `AgentPort` component test。
- 未覆蓋：Router-level status、slow consumer 丟事件、disconnect、JoinError、
  真 provider transport truncation、pipeline 路由的端到端驗證。
