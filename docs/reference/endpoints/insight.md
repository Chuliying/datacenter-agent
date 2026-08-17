# `POST /insight` — 現況契約

> ← [Endpoints](./index.md)  
> **Source**：[`src/server/handler.rs`](../../../src/server/handler.rs) `insight` / `insight_initial` / `final_answer` / `insight_error_to_app_error`；[`src/server/dto.rs`](../../../src/server/dto.rs) `AgentRequest` / `AgentResponse`；[`src/agent/wiring.rs`](../../../src/agent/wiring.rs) `build_insight_pipeline`

取代已移除的 `POST /agent`。非串流分析端點：跑完四階段 `/insight` pipeline，回 finalizer 的
完整答案（analyst 的報告，charts 以 `falcon-chart` fenced block 內嵌）。

## 編排：直接驅動 pipeline

本端點**不經過 runtime turn**——沒有 guardrails、intent 分類、memory 與 audit。
handler 直接呼叫 `build_insight_pipeline` 組出 `Orchestrator` 並 `run`（buffered，無 event sink）。

```
fetcher ──▶ analyst ──▶ charter ──▶ finalizer
（MCP tools）  （純 LLM）  （emit_chart）  （純邏輯組裝）
```

各階段職責見 [agent 模組](../modules/agent.md)。把 pipeline 收到 runtime `AgentPort`
之後是 plan §9 的工作，目前尚未進行。

## Request

```json
{
  "prompt": "本月充電量？",
  "history": [],
  "session_id": "abc",
  "option_id": "charging.monthly"
}
```

| Field | Required | Current behavior |
|---|---|---|
| `prompt` | yes | 非空且 ≤ 2 000 chars（Unicode char count） |
| `history` | no | serde default `[]`；每筆 `{user_prompt, model_response}` 映射成 `Exchange` 折入 `InitialPrompt.history` |
| `session_id` | no | **只進 tracing span，不影響行為**（沒有 runtime memory） |
| `option_id` | no | **只進 tracing span，不影響行為**（沒有 intent 分類） |

`now` 由 `SystemClock` 在 handler 邊界戳一次，之後原樣穿過每個 stage
（見 [agent 模組](../modules/agent.md) 的 clock 段）。

## Response

成功回 `200`：

```json
{
  "user_prompt": "本月充電量？",
  "model_response": "...報告內文，含 ```falcon-chart 區塊...",
  "intent": "unknown"
}
```

> `intent` **恆為 `"unknown"`**，且是硬寫的字面值——本路徑沒有 intent 分類器。
> 需要 resolved intent 請用 [`/agent/stream`](./agent-stream.md)。

## Error mapping

| 情況 | HTTP | 來源 |
|---|---|---|
| 空 prompt / >2 000 chars | 400 | handler `validate_prompt` |
| malformed / missing JSON | 400 | `JsonRejection` |
| body > 64 KiB | **400**（不是 413） | `DefaultBodyLimit` 產生的 rejection 被 `impl From<JsonRejection> for AppError` 一律壓成 `BadRequest` |
| pipeline 組裝失敗 | 502 | `build_insight_pipeline` 錯誤包成 `BadGateway` |
| `AgentError::Capability`（LLM transport、MCP tool 失敗） | 502 | `insight_error_to_app_error` |
| 其他 `AgentError`（internal mismatch、missing artifact、unknown tool） | 503 | 同上；視為 wiring fault |
| pipeline 沒產出 `Final` payload | 503 | `final_answer` |
| handler 未於 120 s 內回應 | 504 | Router `TimeoutLayer`（空 body） |

## Example

```bash
curl -s http://localhost:8080/insight \
  -H "Authorization: Bearer $GLOBAL_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"prompt":"本月充電量？"}'
```

> **與 `/v1/chat/completions` 的差異**：只有 OpenAI 端點會依 extractor 自己的 status 分流出
> 413 / 415；standard 端點群全部壓成 400，掩蓋了 body-limit 與 content-type 的區別。

## Coverage gaps

- 沒有 Router-level test 固定 400/502/503/504 的實際 status 與 body。
- `build_insight_pipeline` 的失敗分支（tool grant 不完整、LLM 解析失敗）沒有端點層測試。
