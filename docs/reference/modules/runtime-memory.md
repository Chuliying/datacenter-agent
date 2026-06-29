# 模組：`runtime::memory` — partial

> ← [Modules](./index.md)  
> **Source**：[`src/runtime/memory/store.rs`](../../../src/runtime/memory/store.rs)、[`src/runtime/memory/context.rs`](../../../src/runtime/memory/context.rs)、[`src/runtime/orchestrator.rs`](../../../src/runtime/orchestrator.rs) `apply_memory_context` / `append_memory_turn_if_enabled`

## 現況

- `SessionMemoryStore` trait 與 in-memory implementation。
- memory enabled 且 request 有 `session_id` 時，讀取 recent turns、組成 untrusted context、寫入 prompt，並清空 upstream client history。
- request 無 `session_id` 時不使用 server memory，保留 client history。
- store retention 受 `max_turns` 限制。

## 實際 scope / isolation

`SessionMemoryScope` 支援 `actor_id`，但 production load/append 都傳 None。key 實際是 anonymous + client-provided session id；目前不能宣稱 multi-user/tenant isolation。

## Context semantics

- sanitizer 只在 text 命中 `ignore previous instructions`、`system prompt`、`忽略先前指令` 時把整欄改成 `[filtered]`。
- formatted context 超過 `max_memory_context_chars` 時整段回 None；沒有 partial truncation。
- 沒有 system prompt mismatch 或 session-id mismatch check。
- `user_summary`/`answer_summary` 目前寫入 raw full prompt/response，不是真正摘要。

完成樣貌與 identity decision 見 [PRD FR-006](../prd.md)；工作見 [plan I05](../../../.agent/artifacts/plan/2026-06-29-runtime-correctness/implementation.md)。
