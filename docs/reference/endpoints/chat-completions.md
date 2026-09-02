# `POST /v1/chat/completions` — 現況契約

> ← [Endpoints](./index.md)  
> **Source**：[`src/server/openai.rs`](../../../src/server/openai.rs)（DTO + mapping）、[`src/server/handler.rs`](../../../src/server/handler.rs) `chat_completions` / `openai_buffered_response` / `openai_stream_response`、[`src/server/auth.rs`](../../../src/server/auth.rs) `require_bearer_openai`、[`src/server/route.rs`](../../../src/server/route.rs)
> **交付紀錄**：[`docs/work/_archive/agentgateway-openai-endpoint/`](../../work/_archive/agentgateway-openai-endpoint/spec.md)（spec D1–D9、qa-report findings）

OpenAI 相容端點，讓本服務能被註冊成 **agentgateway Path C**（OpenAI-compatible LLM backend），
同時 [`/agent/stream`](./agent-stream.md) 及其豐富 SSE 事件維持不變。
wire 是標準 OpenAI；內部映射成 `AgentRequest`，跑**同一套** runtime prelude + sub-agent pipeline。

## 與其他端點的三個不同

| 面向 | 本端點 | 其他 8 條 |
|---|---|---|
| Auth 失敗 | `401` + OpenAI error envelope | `418` + 專案 error body |
| Request timeout | 600 s，逾時回 `504` + **非空** OpenAI envelope | 120 s，逾時回 `504` 空 body |
| Runtime | **必要**；`RUNTIME_ENABLED=false` 時回 `503` | `/agent/stream` 同樣必要；直接 pipeline 端點不需要 |

600 s 是必要的：非串流路徑要等整條 multi-stage pipeline 跑完才能回 `chat.completion`，
routinely 超過 120 s。

## Request

```json
{
  "model": "datacenter-agent",
  "messages": [{"role": "user", "content": "本月充電量？"}],
  "stream": true,
  "stream_options": {"include_usage": true}
}
```

| Field | Required | Current behavior |
|---|---|---|
| `model` | yes | 反映回 response，不影響選模（實際模型由服務端 config 決定） |
| `messages` | yes | 見下方 mapping 規則 |
| `stream` | no | serde default `false` |
| `stream_options.include_usage` | no | `true` 時在 `[DONE]` 前補一筆 usage-only chunk；其餘 `stream_options` 欄位忽略 |

### `messages` mapping 規則

授權模式固定為**單輪**：從 `messages` 尾端找最後一個 `user`，只把它作為 prompt；之前所有
`user`、`assistant`、`system`、`developer`、`tool`、`function` message 都丟棄。沒有任何
`user` message（空陣列、只有 system、或只有 assistant）才回 `400 invalid_request_error`。
因此沒有 transcript folding，也沒有 OpenAI 的 `session_id`／`option_id` 對應；server-side
memory 在本端點 inert。選中的單一 prompt 再經 runtime 的 4 000-char cap。

## Response

### `stream=false`（D2，buffered）

pipeline `run()` 跑完，包成單一 `chat.completion` choice。

### `stream=true`（D1，**偽串流**）

pipeline 的**完整**終局答案被切成多個 `chat.completion.chunk`，最後送 `data: [DONE]`。

> **這不是真 token 串流。** 終局階段（finalizer / renderer）是純邏輯組裝出完整答案的，
> 沒有一個「等同最終答案」的 token 流可轉發，因此中間的 `ContentDelta` 預覽**不外送**。
> 需要真正的逐階段即時事件請用 [`/agent/stream`](./agent-stream.md)。

答案與失敗都取自 pipeline task 的 awaited 回傳值（權威來源），**不是**從有損的 event channel
drain 出來的。answer policy 要求 disclaimer 時，disclaimer 會被 prepend 到答案前面。

## Error mapping

全部使用 OpenAI error envelope `{"error":{"message","type","code"}}`。

| 情況 | HTTP | `type` |
|---|---|---|
| service bearer 失敗 | 401 | `invalid_request_error` / `auth.service_token_invalid` |
| Falcon header 缺失或格式錯誤 | 401 | `invalid_request_error` / `identity.header_missing` |
| Falcon permissions 401，body `error_code` 為 `auth.token_invalid` | 401 | `invalid_request_error` / `identity.token_refreshable`（可花一次 refresh 並重送一次）|
| Falcon permissions 401，其餘／未知／無 `error_code` | 401 | `invalid_request_error` / `identity.token_terminal`（**不得** refresh）|
| Falcon timeout／連線失敗／5xx／畸形 200 | 503 | `server_error` / `identity.upstream_unavailable` |
| Falcon 400 `auth.conflicting_credentials` | 500 | `server_error` / `identity.upstream_conflict`（runtime 端請求建構錯誤；告警且不寫負向 cache）|
| `messages` 無 user message | 400 | `invalid_request_error` / `request.invalid` |
| malformed JSON | 400 | `invalid_request_error` |
| 缺少或錯誤 content-type | 415 | `invalid_request_error` |
| body > 64 KiB | 413 | `invalid_request_error` |
| `RUNTIME_ENABLED=false` | 503 | `server_error` |
| 逾時（600 s） | 504 | `server_error` |
| 其他 middleware 錯誤 | 500 | `server_error` |
| global／per-actor limiter 拒絕 | 429 | `rate_limit_error` / `rate_limit.global` 或 `rate_limit.actor` |

`authz.insufficient` 不是 HTTP error：它是 `200` 的正常 assistant refusal，buffered response
帶 `x_refusal_code`，stream response 在內容後以 `[DONE]` 結束；因此 client 不應把它當作可重試
的 upstream failure。部分 report 權限則仍回成功答案，但終端答案會聲明省略主題並寫入 audit。

`type` 由 `error_type_for_status` 決定：`400..=499` → `invalid_request_error`，
其餘 → `server_error`。envelope 永遠帶 `type`，不會省略。唯一例外是 opt-in
burst limiter 的 `429`：`type` 固定為 `rate_limit_error`，並帶整數秒
`Retry-After` 與 `Cache-Control: no-store`；bearer 驗證（401）先於 limiter，
不消耗 bucket。詳見 `docs/work/runtime-user-session-rate-limit/runbook.md`。

`JsonRejection` 依 extractor 自己的 status 分流成 413 / 415 / 400，不像其他端點一律壓成 400。

## 未接線的欄位

`session_id` / `option_id` 沒有 OpenAI 對應欄位，因此**server-side memory 在本端點是 inert 的**。

## Example

```bash
curl -s http://localhost:8080/v1/chat/completions \
  -H "Authorization: Bearer $GLOBAL_TOKEN" \
  -H "X-Falcon-Authorization: Bearer $FALCON_ACCESS_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"model":"datacenter-agent","messages":[{"role":"user","content":"本月充電量？"}]}'
```

## Test evidence 與缺口

- `openai.rs`、`handler.rs`、`identity.rs`、`route.rs` 與 `runtime_contract.rs` 的 crate／integration
  tests 覆蓋單輪 mapping、三種 identity 失敗、拒答 code、降級 helper、timeout envelope 與 route scope。
- Falcon client tests 覆蓋文件化 200 shape、generic 401、畸形 200／transport negative cache 與 LRU。
- 非本地範圍：真實上游與 agentgateway 對接、斷線 cancellation、slow consumer 與 live LLM/MCP
  pipeline；這些不會在 `cargo test` 中消耗憑證或外部服務。
