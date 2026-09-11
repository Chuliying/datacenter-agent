# `GET /greeting`

> ← 回 [端點總覽](./index.md) ｜ [專案主體](../index.md)
>
> **Source**：[`src/server/handler.rs`](../../../src/server/handler.rs) → `greeting()`；預生成 [`src/server/greeting.rs`](../../../src/server/greeting.rs)；回應型別 [`dto.rs`](../../../src/server/dto.rs) → `GreetingResponse`

## 用途
回傳一句**預生成、具資料感知**的歡迎詞。開機時數個背景任務各跑一次**兩階段 greeting
pipeline**（`fetcher → analyst`），把結果存進 `AppState::greetings`；本端點隨機挑一句。

> **接線已改變。** greeting 過去走 `llm_connector::generate` 的 tool-calling 迴圈，
> 現在改用 [sub-agent 層](../modules/agent.md)的 `build_greeting_pipeline`，
> 用 `greeting_fetcher_system` / `greeting_analyst_system` 兩份 prompt
> （原本的單一 `greeting_system` 已移除）。

## 契約
| 項目 | 值 |
|---|---|
| 方法 / 路徑 | `GET /greeting` |
| 認證 | **需要**（bearer） |
| 成功 | `200 OK` |
| 失敗 | `503 Service Unavailable`（greetings 尚未就緒，請稍後重試） |

### 回應 body
```json
{ "greeting": "...", "scope": "full" }
```

`scope` 為 `"full"`（資料感知問候）或 `"neutral"`（權限不足時的中性問候，不含任何數字）。

### 依權限縮放（`X-Falcon-Authorization` 選用）
本端點不在 identity middleware 之內（探測與歡迎詞必須在解析使用者前就能用），但**若請求帶
`X-Falcon-Authorization: Bearer <FALCON_ACCESS_TOKEN>`**，handler 會向 Falcon 解析權限並決定：

| 情況 | 回應 |
|---|---|
| 未帶 header | 全域資料感知問候（舊行為） |
| 權限解鎖 `[insight.grants].fetcher` 的**每一個**工具 | 全域資料感知問候，`scope: "full"` |
| 權限只解鎖部分工具、無工具（如僅 `engproj`）、header 格式錯誤、Falcon 無法驗證 | 中性問候 `請選擇事業單位，或直接輸入想了解的營運問題。`，`scope: "neutral"`，仍是 `200` |

判定為 `authz::greeting_scope_allows`。保守設計：預生成的問候可能同時引用營收、會員與站點數字，
只要有一類工具不在使用者權限內就整句換掉，避免使用者在歡迎詞先看到自己提問會被拒絕的數字。

## 行為註記
- 從 `state.greetings`（`Mutex<Vec<String>>`）隨機 `choose`。
- 若 vector 還空（背景任務未完成）→ `AppError::ServiceUnavailable`。
- 生成邏輯與 prompt 來源見 [server 模組 · greeting](../modules/server.md#greeting) 與
  [agent 模組](../modules/agent.md)。本端點**不經過** `llm_connector`。

## 範例（curl）
```bash
curl -s http://localhost:8080/greeting \
  -H "Authorization: Bearer $GLOBAL_TOKEN"
# → {"greeting":"...","scope":"full"}
# 尚未就緒 → 503 Service Unavailable

curl -s http://localhost:8080/greeting \
  -H "Authorization: Bearer $GLOBAL_TOKEN" \
  -H "X-Falcon-Authorization: Bearer $FALCON_ACCESS_TOKEN"
# 權限不足的角色 → {"greeting":"請選擇事業單位，或直接輸入想了解的營運問題。","scope":"neutral"}
```

## 相關
- 預生成任務 → [server 模組](../modules/server.md)
- prompt 載入（`greeting_fetcher_system` / `greeting_analyst_system` / `greeting_user`）
  → [專案主體 · 啟動與組裝](../index.md#4-啟動與組裝top-level-接線)
