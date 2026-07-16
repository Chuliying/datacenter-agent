# 模組：`runtime::audit` — partial

> ← [Modules](./index.md)  
> **Source**：[`src/runtime/audit.rs`](../../../src/runtime/audit.rs)、[`src/runtime/turn.rs`](../../../src/runtime/turn.rs)、[`src/server/handler.rs`](../../../src/server/handler.rs)

## 已實作

- `AuditSink` trait、`NoopAuditSink`、stdout JSON-lines sink。
- `AuditWriter` 提供 per-request monotonic seq 與 fail-open/fail-closed policy。
- event model 包含 request、normalized/rejected、refused、memory、tool、clear、completed/failed。
- actor IP/user-agent 在 `AuditCtx.actor` 有值時做 SHA-256 hash。
- `redact_secrets` helper 可遮罩 bearer 與已知 API-key assignment pattern。

## Production reality

- REST/SSE handler 都建立 `AuditCtx { actor: None }`，所以 production 沒有 actor hash source。
- `AuditRecord` 保留 raw `session_id` 與 event fields。
- `StdoutAuditSink` 直接 `serde_json` + `println!`；沒有呼叫 `redact_secrets`。
- 沒有 preview env gate。
- injection refusal 會留下 `input_normalized`、`refused` 與 terminal `response_completed(status=refused)` audit，且不呼叫 upstream。
- `Aborted` return path 與 client cancellation 沒有完整 terminal audit contract。

因此目前不能宣稱「每個 audit event 都已 PII hash / secret redacted」。Target 見 [PRD FR-007](../prd.md)；工作見 [plan I05](../../../.agent/artifacts/plan/2026-06-29-runtime-correctness/implementation.md)。
