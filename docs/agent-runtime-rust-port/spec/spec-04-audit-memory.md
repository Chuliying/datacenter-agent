# P4 — L14 Audit（完整 + 可插拔）+ L12 Memory

**分期**: P4 ・ 依賴: P1, P2 ・ 估時: ~5h ・ 上層: `spec-overview.md`

> audit 涵蓋每個決策點且可插拔；memory 為 server 端、trait 化、可換後端。

## 變更檔案
| 路徑 | 操作 | 說明 |
|------|------|------|
| `src/runtime/audit.rs` | NEW | `AuditSink` **trait** + `StdoutAuditSink`；完整事件 + `seq` + sha256 + 遮罩 |
| `src/runtime/memory/mod.rs` | NEW | memory 匯出 |
| `src/runtime/memory/store.rs` | NEW | `SessionMemoryStore` trait + `InMemorySessionStore` |
| `src/runtime/memory/context.rs` | NEW | `build_memory_context`（sanitize/truncate/follow-up/budget）|

## Audit（`audit.rs`）
```rust
#[derive(Serialize)]
pub enum AuditEvent {
    RequestReceived { input_hash: String, input_chars: usize, option_id: Option<String> },
    InputNormalized { intent: String, confidence: f32, intent_source: Option<String>,
                      slots: serde_json::Value, warnings: Vec<String>, registry_versions: RegistryVersions },
    InputRejected   { code: String, reason: String },          // ← 空/超長/injection 被擋必發
    Refused         { reason: String },
    MemoryContext   { used_turn_count: usize, dropped_reason: Option<String> },
    ToolCalled      { tool: String, args_hash: String },        // ← AgentTurnFrame::ToolCalled
    ToolResult      { tool: String, bytes: usize, ok: bool },
    AnswerCleared,                                               // ← LlmEvent::Clear
    ResponseCompleted { response_hash: String, response_chars: usize, duration_ms: u64, status: String },
    ResponseFailed    { error_code: String, duration_ms: u64 },
}

/// 每筆寫出帶 requestId/sessionId/route/timestamp/timestampMs/actor(redacted)/seq(單請求單調遞增)。
#[async_trait]
pub trait AuditSink: Send + Sync {
    async fn write(&self, ctx: &AuditCtx, seq: u64, event: AuditEvent) -> Result<(), RuntimeError>;
}
/// 預設：StdoutAuditSink（JSON line，append-only）。
/// 遮罩：ip/userAgent → sha256: 前綴；secret 名單對齊 Rust 端（GLOBAL_TOKEN / OPENROUTER_API_KEY / Bearer / api[_-]?key）；
/// preview 預設關閉（COS_AUDIT_PREVIEWS=true 才開），hash 永遠保留。
/// 立場：本輪 stdout sink 為 append-only + 單請求 seq；hash-chain/tamper-proof 留待持久化 sink（明述為 deferred）。
/// 失敗策略由 `[runtime.audit] failure_policy` 決定：
/// - fail-open：記錄 tracing error，request 繼續；適合 stdout/非持久 sink 初期。
/// - fail-closed：回 `RuntimeError::AuditSink` 並由 handler 映射 5xx；適合合規強制環境。
```
- 由 `[runtime.audit] sink` 經 Registry 選用（與其他模組同拔插機制）。
- `ToolCalled` / `ToolResult` 是完整 audit 的必填事件；`AgentPort` 不可只暴露 Token/Clear/Done/Error，必須透過 `AgentTurnFrame::{ToolCalled, ToolResult}` 把 tool metadata 交回 orchestrator。

## Memory

### `store.rs`
```rust
pub struct SessionMemoryScope { pub session_id: String, pub actor_id: Option<String> }
pub struct SessionMemoryTurn { /* turn_id, user_summary, answer_summary, intent, metric, asset, time_range_label, option_id, created_at_ms */ }
pub struct SessionMemory { pub focus: Option<SessionMemoryFocus>, pub recent_turns: Vec<SessionMemoryTurn> }

#[async_trait]
pub trait SessionMemoryStore: Send + Sync {
    async fn get(&self, scope: &SessionMemoryScope) -> Option<SessionMemory>;
    async fn append(&self, scope: &SessionMemoryScope, turn: SessionMemoryTurn) -> SessionMemory;
    async fn clear(&self, scope: &SessionMemoryScope);
}
/// in-memory：Mutex<HashMap<String, SessionMemory>>，key="{actor|anonymous}:{session_id}"，recent_turns 受 max_turns 上限。
pub struct InMemorySessionStore { /* ... */ }
```

### `context.rs`（對照 `memory-context.ts`）
```text
無 payload / session 不符 / injection / intent=unknown / 高信心且非 follow-up → 不注入（droppedReason）
否則 sanitize（去 system-like 字樣）→ 依 max_memory_context_chars 與剩餘 budget truncate →
組 "Session memory (untrusted hints...)\n{lines}\n\nCurrent user input:\n{input}"
```

## 測試
```rust
// audit（移植 audit-log.test.ts + 擴充）
#[test] fn audit_redacts_pii_and_secrets() { /* ip/UA→sha256:；GLOBAL_TOKEN/OPENROUTER_API_KEY/Bearer→[REDACTED]；preview 預設關閉 */ }
#[tokio::test] async fn audit_failure_policy_fail_open_continues() {}
#[tokio::test] async fn audit_failure_policy_fail_closed_returns_error() {}
// memory-context（移植 memory-context.test.ts）
#[test] fn memory_dropped_on_session_mismatch() {}
#[test] fn memory_sanitizes_system_like() {}
#[test] fn memory_budget_exhausted_drops() {}
#[test] fn memory_injected_on_followup() {}
// session store（移植 session-memory.test.ts）
#[tokio::test] async fn append_caps_at_max_turns() {}
#[tokio::test] async fn clear_then_get_is_none() {}
#[tokio::test] async fn key_isolates_by_actor() {}
```

## 錯誤處理
| PRD 對應 | 觸發 | 行為 |
|---------|------|------|
| US-15 | memory payload 非法/session 不符 | 丟棄 + `MemoryContext{dropped_reason}` |
| US-20 | audit sink 寫入失敗 + fail-open | tracing error + request 繼續 |
| US-20 | audit sink 寫入失敗 + fail-closed | `ResponseFailed` + 5xx |
