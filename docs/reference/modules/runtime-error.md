# 模組：`runtime::error`

> ← 回 [模組總覽](./index.md) ｜ [專案主體](../index.md)
>
> **Source**：[`src/runtime/error.rs`](../../../src/runtime/error.rs)（“Runtime error model”）

## 職責
runtime 的錯誤模型：用 `thiserror` 列舉，分流 config 載入錯誤與 per-request 錯誤。

## 關鍵型別
- `RuntimeError`（`thiserror`），`RuntimeResult<T>` 別名。
- 變體（節錄）：`InputRequired`、`InputTooLong`、`Upstream`、`AuditSink`、`Config`、`UnknownModule`、`IntentNotAllowed`、`PipelineContract`、`Internal`、`Request`。

## 錯誤策略
- request path 一律 `?` 傳遞；`runtime/` request path **禁用 `unwrap`/`expect`**。
- config 載入失敗 → 中止開機。
- per-request 失敗 → 由 [server](./server.md) 的 `runtime_error_to_app_error` 映射成 `AppError`：
  - `InputRequired` / `InputTooLong` → `400`
  - `Upstream` → `502`
  - 其餘（config/registry/internal）→ `503`

## 相關
- HTTP 對外錯誤 → [server · error](./server.md)
- 上游 LLM 錯誤來源 → [llm_connector](./llm-connector.md)
