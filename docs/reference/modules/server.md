# 模組：`server`（HTTP 層）

> ← 回 [模組總覽](./index.md) ｜ [專案主體](../index.md)
>
> **Source**：[`src/server/mod.rs`](../../../src/server/mod.rs)

## 職責
封裝整個 HTTP 介面：路由組裝、middleware、認證、handler、DTO、錯誤映射、greeting 背景任務。是 runtime 核心與外界的唯一接觸面。

## 子檔案

| 檔案 | 職責 | 關鍵項 |
|---|---|---|
| [`route.rs`](../../../src/server/route.rs) | 路由 + middleware 組裝 | `build_router`；64KiB body cap、120s handler timeout、very-permissive CORS、`TraceLayer`、`CompressionLayer`、nosniff/no-referrer 安全 header、bearer layer |
| [`handler.rs`](../../../src/server/handler.rs) | 五個 handler | `health` / `ready` / `greeting` / `agent` / `agent_stream`；runtime/legacy 雙路徑 |
| [`dto.rs`](../../../src/server/dto.rs) | 請求／回應型別 | `AgentRequest` / `AgentResponse` / `StreamFrame` / `IntentResolvedData` / `GreetingResponse` / `ReadyBody` / `ReadyChecks` |
| <a id="auth"></a>[`auth.rs`](../../../src/server/auth.rs) | bearer 認證 middleware | `require_bearer`；constant-time 比對；失敗回 `418` |
| [`error.rs`](../../../src/server/error.rs) | HTTP 錯誤型別 | `AppError` / `ErrorBody` |
| <a id="greeting"></a>[`greeting.rs`](../../../src/server/greeting.rs) | 開機背景任務 | 跑 greeting prompt 過 tool-calling 迴圈，填 `AppState::greetings` |

## 認證細節（`require_bearer`）
- **每個請求（含 `/health`、`/ready`）**需 `Authorization: Bearer <GLOBAL_TOKEN>` —— 五條 route 全註冊在 `.layer(require_bearer)` 之前、`check()` 無 path 豁免。Kubernetes probe可配置 headers；實際相容性取決於 deployment profile，repo 無部署檔可判定。
- scheme 名稱大小寫不敏感（RFC 6750）。
- token 比對用 `constant_time_eq`（防 timing attack）。
- 失敗 → `418 I'm a teapot` + 茶壺訊息。

## 錯誤映射
`AppError`（`BadRequest 400` / `BadGateway 502` / `ServiceUnavailable 503`）為 HTTP 對外錯誤；runtime 內部的 `RuntimeError` 經 `runtime_error_to_app_error` 轉成 `AppError`。所有 `JsonRejection` 目前統一轉 400，可能掩蓋 body-limit extractor status。詳見 [runtime error](./runtime-error.md)。

`TimeoutLayer` 只限制 handler future，不限制已回傳 Response 的 SSE body。runtime SSE 另有 unbounded channel/cancellation gaps，見 [`/agent/stream`](../endpoints/agent-stream.md)。

## 相關
- 每個端點細節 → [端點總覽](../endpoints/index.md)
- runtime 編排 → [turn](./runtime-turn.md)
- 共享狀態 `AppState` / `AppRuntime` → [專案主體 · 啟動與組裝](../index.md#4-啟動與組裝top-level-接線)
