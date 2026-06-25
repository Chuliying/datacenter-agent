# P5 — Orchestrator + 接線

**分期**: P5 ・ 依賴: P1–P4 ・ 估時: ~6h ・ 上層: `spec-overview.md`

> 一輪 turn 的編排權威；只依賴 trait；包覆現有 `llm_connector` agent loop。把 P1–P4 串成完整流程並接上 axum host。

## 變更檔案
| 路徑 | 操作 | 說明 |
|------|------|------|
| `src/runtime/orchestrator.rs` | NEW | `run_agent_turn`；只依賴 trait；包現有 agent loop |
| `src/appstate.rs` | MODIFY | 加 `runtime`/`sessions`/`audit`/組裝好的 `input_pipeline` + `answer_policy` |
| `src/server/dto.rs` | MODIFY | `AgentRequest` 加 `session_id` / `option_id` |
| `src/server/handler.rs` | MODIFY | 兩 route 改走 orchestrator |

## 型別（`orchestrator.rs`）
```rust
pub enum AgentTurnFrame {
    Token(String),
    Clear,
    ToolCalled { tool: String, args_hash: String },
    ToolResult { tool: String, bytes: usize, ok: bool },
    Done,
    Error(String),
}

#[async_trait]
pub trait AgentPort: Send + Sync {
    /// server-memory 模式：history 傳空，記憶已折進 prompt（對齊 TS run-agent-turn.ts:192）。
    async fn stream(&self, prompt: String, history: Vec<History>)
        -> BoxStream<'static, AgentTurnFrame>;
}

pub enum AgentTurnOutcome {
    Final { response: String, intent: String },
    Refused { reason: RefuseReason, copy: String },
    Error { code: String, status: u16 },
    Aborted,
}

pub struct AgentTurnInput {
    pub prompt: String,
    pub history: Vec<History>,
    pub session_id: Option<String>,
    pub option_id: Option<String>,
}

pub async fn run_agent_turn(
    input: AgentTurnInput,                         // raw request；orchestrator 擁有 pipeline + audit
    ctx: AgentTurnContext,                         // request_id, route, actor, started_at
    deps: AgentTurnDeps<'_>,                       // runtime/pipeline/policy/llm_normalizer?/agent/sessions?/audit/emit
) -> AgentTurnOutcome;
```

**`LlmEvent` → `AgentTurnFrame` 映射（adapter 包 `llm_connector::agent_stream`）**：
`Token(t)`→`Token(t)`；`Clear`→`Clear`（orchestrator 收到時 `buffer.clear()`，對齊 `generate()` agent.rs:452）；`Done`→`Done`；`Error(e)`→`Error(e)`。tool 呼叫**唯一契約**是內部 `AgentTurnFrame::{ToolCalled, ToolResult}`；adapter 必須改 `llm_connector` 讓 tool metadata 可觀測，不能以其他旁路機制寫入 audit。adapter 把 stream 以 `.boxed()` 轉 `BoxStream<'static, AgentTurnFrame>`（owned data 餵入，滿足 `'static`）。

## DTO（`dto.rs` MODIFY）
```rust
#[derive(Debug, Clone, Deserialize)]
pub struct AgentRequest {
    #[serde(default)] pub history: Vec<History>,
    pub prompt: String,
    #[serde(default)] pub session_id: Option<String>,   // NEW
    #[serde(default)] pub option_id: Option<String>,    // NEW：option-path 分類 + audit
}
```

## 串流契約（不變）
```text
data: {"event":"token","data":"今年"}
data: {"event":"clear"}            // 工具迴圈 preamble → 清 buffer（Rust-only）→ audit AnswerCleared
data: {"event":"token","data":"今年累計營收為…"}
data: {"event":"done"}
```

## 一輪 turn 資料流
```text
AgentRequest (prompt, history, session_id, option_id)
    ↓ audit: RequestReceived                       ← 入口即留痕
    ↓ request guard（requestId / actor）
    ↓ input pipeline（P1 組裝）依序 run enabled stage：normalize → input_guard → injection → intent → slots
        - input_guard 失敗（空/超長）→ audit: InputRejected → 400（不呼叫 LLM）
    ↓ audit: InputNormalized
    ↓ optional LLM normalizer（若 [runtime.llm_normalizer].enabled 且命中低信心/灰區條件）
    ↓ AnswerPolicy::decide:
        Refuse      → audit: Refused → 串 refusal token + done（HTTP 200，不呼叫 LLM）
        Disclaimer  → 先送 disclaimer token，再續行
        Answer      → 續行
    ↓ memory（若 [runtime.memory].enabled 且有 session_id）: sessions.get → build_memory_context → audit: MemoryContext
        → server-memory：prompt=折入記憶、upstream history=[]；無 session 或 memory disabled：history=client history
    ↓ AgentPort.stream → llm_connector agent loop（現有）
        - Token → buffer/emit
        - ToolCalled/ToolResult → audit（必填，不上外部 SSE wire）
        - Clear → buffer.clear() → audit: AnswerCleared
        - Done  → 收尾；Error → audit: ResponseFailed + emit error
    ↓ sessions.append + audit: ResponseCompleted
```

## 測試（Rust 獨有，必測；用注入 fake）
```rust
#[tokio::test] async fn clear_frame_clears_buffer() { /* fake AgentPort 發 Token,Clear,Token,Done → 最終答案不含首段 */ }
#[tokio::test] async fn tool_frames_are_audited() { /* fake AgentPort 發 ToolCalled/ToolResult → capturing AuditSink 收到事件 */ }
#[tokio::test] async fn refusal_emits_token_then_done() { /* Refuse → emitted frames = [Token(refusal), Done]，不開 upstream */ }
#[tokio::test] async fn disclaimer_prepended_as_first_token() { /* gray → 第一個 Token 為 disclaimer */ }
#[tokio::test] async fn memory_disabled_uses_client_history() { /* memory_enabled=false → 不讀寫 store，history 原樣 upstream */ }
#[tokio::test] async fn llm_normalizer_disabled_by_default() { /* disabled → 不呼叫 fallback */ }
#[tokio::test] async fn aborted_with_buffer_vs_empty() { /* abort：有 buffer→completed(aborted)；空→failed */ }
#[tokio::test] async fn rejected_request_is_audited() { /* 空輸入 → capturing AuditSink 收到 InputRejected */ }
```

## 錯誤處理
| PRD 對應 | 觸發 | 行為 |
|---------|------|------|
| US-23 | upstream `Clear` | 清 buffer + `AnswerCleared` |
| — | upstream stream error | stable code + `ResponseFailed` |
| US-20 | audit sink fail-open | tracing error + request 繼續 |
| US-20 | audit sink fail-closed | stop turn + `ResponseFailed` / 5xx |
