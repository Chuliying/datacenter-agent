# 模組：`runtime::memory` — partial

> ← [Modules](./index.md)  
> **Source**：[`src/runtime/memory/store.rs`](../../../src/runtime/memory/store.rs)、[`src/runtime/memory/context.rs`](../../../src/runtime/memory/context.rs)、[`src/runtime/turn.rs`](../../../src/runtime/turn.rs) `apply_memory_context` / `append_memory_turn_if_enabled`

## 現況

- `SessionMemoryStore` trait 與 in-memory implementation。
- memory enabled 且 request 有 `session_id` 時，讀取 recent turns、組成 untrusted context、寫入 prompt，並清空 upstream client history。
- request 無 `session_id` 時不使用 server memory，保留 client history。
- store retention 受 `max_turns` 限制。

## 實際 scope / isolation

`SessionMemoryScope` 支援 `actor_id`，但 production load/append 都傳 None。key 實際是 anonymous + client-provided session id；目前不能宣稱 multi-user/tenant isolation。

## Context semantics

- sanitizer 重用 capability config 編譯出的 `InjectionDetector`，命中任一規則時把整欄改成 `[filtered]`；injection refusal 本身不寫入 memory。
- formatted context 超過 `max_memory_context_chars` 時整段回 None；沒有 partial truncation。
- 沒有 system prompt mismatch 或 session-id mismatch check。
- `user_summary`/`answer_summary` 目前寫入 raw full prompt/response，不是真正摘要。

完成樣貌與 identity decision 見 [PRD FR-006](../prd.md)；工作見 [plan I05](../../../.agent/artifacts/plan/2026-06-29-runtime-correctness/implementation.md)。

## 已知死碼
- `context.rs::build_memory_context`（client-history-based context builder）與 `SessionMemoryStore::load`/`append`（`store.rs` 的 legacy trait method，走 `legacy_history` map）目前在 `src/` 內沒有任何呼叫端，是較早設計遺留的死碼，非目前生效的 memory 機制。
