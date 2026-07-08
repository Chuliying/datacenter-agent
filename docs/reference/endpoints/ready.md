# `GET /ready`

> ← 回 [端點總覽](./index.md) ｜ [專案主體](../index.md)
>
> **Source**：[`src/server/handler.rs`](../../../src/server/handler.rs) → `ready()`；回應型別 [`dto.rs`](../../../src/server/dto.rs) → `ReadyBody` / `ReadyChecks`

## 用途
Readiness 探針：服務是否**準備好接流量**。檢查兩件事：
1. `OPENROUTER_API_KEY` 非空（`api_key`）。
2. LLM `base_url` 可達 —— 對 base url 發 `HEAD`（2s timeout 的 probe client）（`base_url_reachable`）。

## 契約
| 項目 | 值 |
|---|---|
| 方法 / 路徑 | `GET /ready` |
| 認證 | **需要**（bearer；現況 — 見下方註記） |
| 成功 | `200 OK`（`ready=true`） |
| 失敗 | `503 Service Unavailable`（任一檢查失敗） |

> 現況 `/ready` 走 bearer，缺 token 回 `418`（不是 503）。Kubernetes HTTP probe 可配置 headers；實際相容性取決於 deployment profile，而 repo 沒有部署 manifest。完成樣貌要求明確決定並測試 probe auth policy，見 [PRD FR-011](../prd.md)。

### 回應 body
```json
{
  "ready": true,
  "checks": { "api_key": true, "base_url_reachable": true }
}
```

## 行為註記
- `ready = api_key && base_url_reachable`。
- base url probe 失敗只記 `warn`，不讓 handler 崩。
- probe client timeout 在 [`appstate.rs`](../../../src/appstate.rs) 建構（2 秒）。

## 範例（curl）
```bash
curl -i http://localhost:8080/ready \
  -H "Authorization: Bearer $GLOBAL_TOKEN"
# 就緒 → 200 OK，body: {"ready":true,"checks":{"api_key":true,"base_url_reachable":true}}
# 未就緒 → 503 Service Unavailable
# 缺 token → 418（現況；見上方註記）
```

## 相關
- 純存活檢查 → [`/health`](./health.md)
- API key / base url 來源 → [appstate（LlmDefaults）](../modules/server.md) 與環境變數
