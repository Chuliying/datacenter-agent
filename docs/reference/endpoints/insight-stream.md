# `POST /insight/stream` — 現況契約

> ← [Endpoints](./index.md)  
> **Source**：[`src/server/handler.rs`](../../../src/server/handler.rs) `insight_stream` / `insight_frames` / `INSIGHT_STREAM_BUFFER`；[`src/server/dto.rs`](../../../src/server/dto.rs) `StreamFrame`

[`POST /insight`](./insight.md) 的 SSE 版本。跑同一條四階段 pipeline，把過程即時串到一個
per-turn 共享 sink 上。

## Request / response

- Request body 與 [`/insight`](./insight.md) 相同（`AgentRequest`，2 000-char cap）。
- Bearer required（失敗 418）。
- 成功建立 stream 後為 `text/event-stream`。
- keep-alive interval 15 秒。
- **不經過 runtime turn**，與 `/insight` 一致。

空／超長 prompt 在建立 stream **之前**回 HTTP 400。
[`/agent/stream`](./agent-stream.md) 現在也是 pre-stream 回真實 HTTP status（只是 cap 為 4 000
且錯誤來自 runtime prelude），兩者行為一致。

## SSE frame

每個 SSE `data:` 是一個 JSON object，discriminator 為 `event`。

| Event | JSON payload | 意義 |
|---|---|---|
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
模型沒回報時整個欄位省略（`skip_serializing_if`）。

### 終局協定：clear 後重送完整答案

結束時的順序固定是 **`clear` → `token`（完整答案）→ `done`**。
中間串流的是各 stage 的預覽片段，可能不是最終內容；`clear` 之後重送的那一筆 `token` 才是
finalizer 的完整答案（報告 + charts）。因此消費端只要遵守這個協定，**永遠會以正確的完整
答案收尾**，不必自行拼接中間片段。

### 未外送的事件

`ToolStarted`、`ToolProduced`、`ReasoningDelta`、`StageProduced` 目前留在內部，不映射成 SSE frame。

## Lifecycle 現況與限制

- event channel 為**有界** buffer（`INSIGHT_STREAM_BUFFER = 8192`），由 `ChannelSink` 以
  `try_send` 投遞。正常一輪的事件量不會塞爆，但這是**有損**設計：滿了就丟，無 backpressure。
  無損 channel 列為後續工作。
- 120 秒 Router timeout 只限制 handler 建立 Response 之前；不限制之後的 SSE body。
- client disconnect 不會取消上游 pipeline（與 `/agent/stream` 同樣的限制）。

## Example

```bash
curl -N http://localhost:8080/insight/stream \
  -H "Authorization: Bearer $GLOBAL_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"prompt":"列出本週充電量前五名"}'
```

## Coverage gaps

- 沒有 Router-level test 固定 frame 順序與 `clear` → `token` → `done` 終局協定。
- buffer 滿載丟事件的行為沒有測試。
- disconnect 後 pipeline 續跑的成本沒有量測。
