# `POST /v1/chat/completions` — 現況契約

> ← [Endpoints](./index.md)  
> **Source**：[`src/server/openai.rs`](../../../src/server/openai.rs)（DTO + mapping）、[`src/server/handler.rs`](../../../src/server/handler.rs) `chat_completions` / `openai_buffered_response` / `openai_stream_response` / `fold_history_into_prompt`、[`src/server/auth.rs`](../../../src/server/auth.rs) `require_bearer_openai`、[`src/server/route.rs`](../../../src/server/route.rs)  
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
  "messages": [
    {"role": "user", "content": "本月充電量？"},
    {"role": "assistant", "content": "..."},
    {"role": "user", "content": "那 AC 佔比呢？"}
  ],
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

- 只有 `user` / `assistant` 兩種 role 帶對話輪次；**其他 role 全部丟棄**，包含
  `system` / `developer`（pipeline 沒有 system slot，各 stage 自帶設計好的 instruction），
  以及 `tool` / `function`（本端點不宣告任何 tool）。
- 丟棄而非合併的理由：重播帶 tool message 的 transcript 時，不會污染 user/assistant history。
- 必須有結尾的 `user` message 當 prompt。沒有任何 message、或結尾不是 `user`（例如以
  `assistant` 收尾）→ `400 invalid_request_error`。

### History 折入時機（重要）

history 在**呼叫 runtime prelude 之前**就折入 prompt（`fold_history_into_prompt`），
格式為：

```
以下是先前的對話紀錄:
User: ...
Assistant: ...

目前的問題:
<最後一則 user message>
```

這個順序讓 **intent 分類與 answer policy 也看得到對話上下文**——多輪 follow-up
（例如前輪問營收、本輪問「那 AC 佔比呢？」）能正確分類，不會被誤判成 unknown 而拒答。

副作用：折入後的 prompt 一併受 prelude 的 4 000-char cap 約束。適度多輪無虞，
極長對話可能觸 cap，屬既有輸入保護。

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

全部使用 OpenAI error envelope `{"error":{"message","type"}}`。

| 情況 | HTTP | `type` |
|---|---|---|
| 認證失敗 | 401 | `invalid_request_error` |
| `messages` 無 user message / 結尾非 user | 400 | `invalid_request_error` |
| malformed JSON | 400 | `invalid_request_error` |
| 缺少或錯誤 content-type | 415 | `invalid_request_error` |
| body > 64 KiB | 413 | `invalid_request_error` |
| `RUNTIME_ENABLED=false` | 503 | `server_error` |
| 逾時（600 s） | 504 | `server_error` |
| 其他 middleware 錯誤 | 500 | `server_error` |
| burst limiter 拒絕（`[server.rate_limit]` opt-in，預設關閉） | 429 | `rate_limit_error` |

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
  -H "Content-Type: application/json" \
  -d '{"model":"datacenter-agent","messages":[{"role":"user","content":"本月充電量？"}]}'
```

## Test evidence 與缺口

- cargo lib 207/0、clippy 綠；`route.rs` 有 timeout envelope 與 merge-survival test。
- **未完成的端到端驗證**（見 work item 的 qa-report）：
  - 真實上游 `DATACENTER_API_BASE`（正確 host）的成功查詢 e2e——目前只用本機 mock stub 驗過。
  - 平台端 agentgateway 對接煙霧測試。
- 刻意保留：斷線 abort（共用多工 rmcp peer，mid-request 取消安全無法離線確證）、
  真 pipeline 兩路徑整合測試（`McpHandle` 無 fake 接縫）。
