# 模組：`runtime::schema`

> ← 回 [模組總覽](./index.md) ｜ [專案主體](../index.md)
>
> **Source**：[`src/runtime/schema.rs`](../../../src/runtime/schema.rs)（“Shared runtime schema”）

## 職責
runtime 內部共享的資料型別 —— 連接 input、orchestrator、guardrails、memory 的共同語彙。

## 關鍵型別

| 型別 | 說明 |
|---|---|
| `AgentTurnInput` | 一個 turn 的輸入：`request_id` / `raw_input` / `prompt` / `history` / `session_id` / `option_id` |
| `AgentTurnFrame` | orchestrator 內部串流幀（tool call/result 以此交付） |
| `NormalizedInput` | [input pipeline](./runtime-input.md) 輸出 |
| `NormalizedSlots` | 抽取出的 `time_range` / `metric` / `asset` / `rank_limit` optional fields；沒有獨立 `TimeRangeSlot` 型別 |

## 設計註記
- `intent` 為對 config allowlist 驗證過的 `String`，**非 enum**（config 驅動平台要求）。
- 型別同時 derive `Serialize` + `Deserialize`，供測試/fixtures 使用。

## 相關
- 產生者 → [input](./runtime-input.md)
- 消費者 → [orchestrator](./runtime-orchestrator.md)
- 對外 DTO（HTTP wire）→ [server · dto](./server.md)
