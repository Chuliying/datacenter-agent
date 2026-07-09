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
| `AgentTurnFrame` | orchestrator 內部串流幀，`Token`/`Clear`/`ToolCalled`/`ToolResult`/`Done`/`Error` 六種 variant（tool call/result 以此交付） |
| `NormalizedInput` | [input pipeline](./runtime-input.md) 輸出 |
| `NormalizedSlots` | 抽取出的 `time_range` / `metric` / `asset` / `rank_limit` optional fields；沒有獨立 `TimeRangeSlot` 型別 |
| `RuntimeWarning` | pipeline 各階段附加的警告（如 `prompt_injection_detected`），掛在 `NormalizedInput` 上 |
| `IntentSource` | intent 判定來源：`OptionPath` / `RuleLexicon` / `TextOverride` / `Unknown`，掛在 `AgentTurnFrame`／`NormalizedInput` 上 |

## 設計註記
- `intent` 為對 config allowlist 驗證過的 `String`，**非 enum**（config 驅動平台要求）。
- 多數型別同時 derive `Serialize` + `Deserialize`，供測試/fixtures 使用；例外是 `AgentTurnInput`，只 derive `Debug`/`Clone`/`PartialEq`（內含 `Uuid`/`History`，不需要走 wire 序列化）。

## 相關
- 產生者 → [input](./runtime-input.md)
- 消費者 → [turn](./runtime-turn.md)
- 對外 DTO（HTTP wire）→ [server · dto](./server.md)
