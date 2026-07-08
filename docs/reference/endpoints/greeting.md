# `GET /greeting`

> ← 回 [端點總覽](./index.md) ｜ [專案主體](../index.md)
>
> **Source**：[`src/server/handler.rs`](../../../src/server/handler.rs) → `greeting()`；預生成 [`src/server/greeting.rs`](../../../src/server/greeting.rs)；回應型別 [`dto.rs`](../../../src/server/dto.rs) → `GreetingResponse`

## 用途
回傳一句**預生成、具資料感知**的歡迎詞。開機時數個背景任務跑 greeting prompt 過同一條 tool-calling 迴圈，把結果存進 `AppState::greetings`；本端點隨機挑一句。

## 契約
| 項目 | 值 |
|---|---|
| 方法 / 路徑 | `GET /greeting` |
| 認證 | **需要**（bearer） |
| 成功 | `200 OK` |
| 失敗 | `503 Service Unavailable`（greetings 尚未就緒，請稍後重試） |

### 回應 body
```json
{ "greeting": "..." }
```

## 行為註記
- 從 `state.greetings`（`Mutex<Vec<String>>`）隨機 `choose`。
- 若 vector 還空（背景任務未完成）→ `AppError::ServiceUnavailable`。
- 生成邏輯與 prompt 來源見 [server 模組 · greeting](../modules/server.md#greeting) 與 [llm_connector](../modules/llm-connector.md)。

## 範例（curl）
```bash
curl -s http://localhost:8080/greeting \
  -H "Authorization: Bearer $GLOBAL_TOKEN"
# → {"greeting":"..."}
# 尚未就緒 → 503 Service Unavailable
```

## 相關
- 預生成任務 → [server 模組](../modules/server.md)
- prompt 載入（`greeting_system` / `greeting_user`）→ [專案主體 · 啟動與組裝](../index.md#4-啟動與組裝top-level-接線)
