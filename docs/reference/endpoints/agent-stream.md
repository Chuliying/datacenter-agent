# `POST /agent/stream` — 現況契約

> ← [Endpoints](./index.md)  
> **Source**：[`src/server/handler.rs`](../../../src/server/handler.rs) `agent_stream` / `wants_report_pipeline` / `insight_frames` / `status_to_app_error`；[`src/runtime/turn.rs`](../../../src/runtime/turn.rs) `plan_stream_turn` / `append_memory_turn_if_enabled`；[`src/server/dto.rs`](../../../src/server/dto.rs) `StreamFrame`

**唯一的原生串流前門**：跑 runtime prelude（guardrails → intent → answer policy → memory
→ audit），再依 resolved intent 挑選 sub-agent pipeline 串流。

## Pipeline 選擇規則

`wants_report_pipeline`：只要 `report` intent **被提及**——是 resolved top intent，或出現在
candidate intents 之中——就走 **report pipeline**（`fetcher → analyst → composer → renderer`），
否則走 **insight pipeline**（`fetcher → analyst → charter → finalizer`）。

用「被提及」而非嚴格 top-1 的理由：像「營收報告」這種 topic + report 的 prompt 仍會路由到
report pipeline，同時它的 topic intent（`revenue`）繼續驅動 answer policy 與 memory。

兩條 pipeline 的階段組成見 [agent 模組](../modules/agent.md)。

> **曾經存在的強制 pipeline 端點已退役。** `/insight/stream` 與 `/report/stream` 各自固定驅動
> 一條 pipeline 且繞過 runtime；本端點以 intent 路由涵蓋兩者，功能為嚴格超集，因此那兩條
> 於 work item [`retire-superseded-agent-endpoints`](../../work/retire-superseded-agent-endpoints/prd.md)
> 移除。它們也是當時唯一沒有 guardrail 與 audit 的 prompt 入口。

### Runtime 必要

需要 runtime 啟用（`RUNTIME_ENABLED`，預設 on）。rollback 時回 `503`。

`RUNTIME_ENABLED=false` 下**沒有替代路徑**——只剩 `/health`、`/ready`、`/greeting` 可用。
該 flag 因此已不具備 rollback 意義，存廢見 work item 的 FU-003。

> 舊的 legacy 路徑（`llm_connector::agent_stream` 直呼）**已不存在於本端點**。
> `llm_connector` 現在只剩 eval CLI 使用，見 [llm_connector](../modules/llm-connector.md)。

## Request / response

- Request body 為 `AgentRequest`（`{prompt, history?, session_id?, option_id?}`）。
- Bearer required（失敗 418）。
- 成功建立 stream 後為 `text/event-stream`，keep-alive interval 15 秒。
- prompt cap 為 runtime config `thresholds.input.max_prompt_chars`（目前 4 000），
  由 prelude 執行。這是全服務唯一的 prompt cap。

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

每個 SSE `data:` 是一個 JSON object，discriminator 為 `event`。`StreamFrame` 共 **9 種** variant：

| Event | JSON payload | 意義 |
|---|---|---|
| `intent.resolved` | `{"event":"intent.resolved","data":{"intent":"revenue","candidateIntents":["revenue"]}}` | **僅 `Proceed` 路徑**：任何 token 之前，送一次 |
| `stage` | `{"event":"stage","data":{"agent":"fetcher","phase":"started"}}` | 某個 sub-agent 開始；完成時再送一筆 `success` / `failure` |
| `token` | `{"event":"token","data":"<片段>"}` | 目前 stage 輸出的片段 |
| `tool_call` | `{"event":"tool_call","data":{...}}` | 模型組完一次 tool call（帶 call id 與 tool 名） |
| `tool_args` | `{"event":"tool_args","data":{...}}` | tool call 的 JSON 參數片段（組裝中的即時進度） |
| `usage` | `{"event":"usage","data":{"prompt":N,"completion":N,"reasoning":N,"total":N}}` | 一次 LLM turn 的 token 用量；一個 stage 可能報多次 |
| `clear` | `{"event":"clear"}` | 清掉先前串流的預覽 |
| `done` | `{"event":"done"}` | 乾淨結束，關連線 |
| `error` | `{"event":"error","data":"<message>"}` | 終止性錯誤，關連線 |

`phase` 值為 `started` / `success` / `failure`（`rename_all = "lowercase"`），足以驅動
「轉圈 → 變綠/變紅」的每階段指示燈。`usage` 的 `reasoning` 是 `completion` 的子集，
模型沒回報時整個欄位省略（`skip_serializing_if`）。`IntentResolvedData` 的 `candidate_intents`
透過 `rename_all = "camelCase"` 序列化為 `candidateIntents`，對齊前端事件形狀。

> `Refused` 路徑**不送** `intent.resolved`——它直接 yield refusal copy 的 `token` 再 `done`。
> 消費端不可假設每次 200 回應都以 `intent.resolved` 開頭。

### 終局協定：clear 後重送完整答案

結束時的順序固定是 **`clear` → `token`（完整答案）→ `done`**（`insight_frames` 對終局
`Finished` 事件的映射）。中間串流的是各 stage 的預覽片段，可能不是最終內容；`clear` 之後
重送的那一筆 `token` 才是完整答案（insight pipeline 的報告 + charts，或 report pipeline 的
`falcon-report` HTML）。消費端只要遵守這個協定，**永遠會以正確的完整答案收尾**。

### 未外送的事件

`ToolStarted`、`ToolProduced`、`ReasoningDelta`、`StageProduced` 目前留在內部，不映射成 SSE frame。

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
