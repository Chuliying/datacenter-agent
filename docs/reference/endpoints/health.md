# `GET /health`

> ← 回 [端點總覽](./index.md) ｜ [專案主體](../index.md)
>
> **Source**：[`src/server/handler.rs`](../../../src/server/handler.rs) → `health()`

## 用途
Liveness 探針：程序活著就回 200，**不檢查任何下游依賴**（不碰 LLM、不碰 MCP）。

## 契約
| 項目 | 值 |
|---|---|
| 方法 / 路徑 | `GET /health` |
| 認證 | **需要**（bearer；現況 — 見下方註記） |
| 請求 | 無 body |
| 成功 | `200 OK`，無 body |

> 現況 `/health` 走 bearer，缺 token 回 `418`。Kubernetes HTTP probe 可配置 headers，因此是否相容取決於 deployment profile；repo 沒有部署 manifest可驗證。完成樣貌要求明確決定並測試 probe auth policy，見 [PRD FR-011](../prd.md)。

## 行為
`async fn health() -> StatusCode { StatusCode::OK }` —— 最小成本，給 orchestrator/LB 判斷 container 是否該被重啟。

## 範例（curl）
```bash
curl -i http://localhost:8080/health \
  -H "Authorization: Bearer $GLOBAL_TOKEN"
# → HTTP/1.1 200 OK
# 缺 token → 418 I'm a teapot（現況；見上方註記）
```

## 相關
- 想知道依賴是否就緒（API key、LLM 可達）→ [`/ready`](./ready.md)
