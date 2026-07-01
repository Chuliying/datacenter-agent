# P3 — L6 Guardrails + 回答前決策

**分期**: P3 ・ 依賴: P1, P2 ・ 估時: ~3h ・ 上層: `spec-overview.md`

> 注入偵測、結構性 guard、回答前 4 級決策。決定「擋下/拒絕/加提示/作答」，且在呼叫 LLM 前完成。

## 變更檔案
| 路徑 | 操作 | 說明 |
|------|------|------|
| `src/runtime/guardrails/mod.rs` | NEW | guardrails 匯出 |
| `src/runtime/guardrails/injection.rs` | NEW | 版本化注入 regex set（`(?i)` flag）|
| `src/runtime/guardrails/input_guard.rs` | NEW | 長度/必填（`apply_input_guardrails`）|
| `src/runtime/guardrails/answer_policy.rs` | NEW | `decide_answer_action`（4 級）|

## 型別 / 邏輯

### `answer_policy.rs`（對照 `answer-policy.ts`）
```rust
pub enum RefuseReason { PromptInjection, OffScope }
pub enum AnswerAction { Answer, AnswerWithDisclaimer, Refuse(RefuseReason) }
pub fn decide_answer_action(n: &NormalizedInput, t: &Thresholds) -> AnswerAction;
```
4 級：① warnings 含 `prompt_injection_detected` → Refuse(PromptInjection)；② intent=unknown 或 confidence < answer_gray → Refuse(OffScope)；③ confidence < answer_normal → AnswerWithDisclaimer；④ 其餘 Answer。

### `injection.rs`（對照 `injection-patterns.ts`）
- 版本化 regex set（帶 `INJECTION_HEURISTIC_VERSION`）；JS `/i` → Rust `(?i)`；檢視 `\b`/anchor 對 CJK 的語意差異（非 verbatim）。

### `input_guard.rs`
- 空/超長檢查（cap 來自 capability pack config：`[input].max_prompt_chars`；EV parity pack 設為 4000。host 仍保留 64KiB body cap）；失敗回 `RuntimeError::InputRequired`/`InputTooLong`，**並由 orchestrator 發 `InputRejected` audit**（見 P4/P5）。

## 拒絕 / 提示 wire 契約（不新增事件型別）

### 語意拒絕（off-scope / injection）→ 既有 frame + HTTP 200
```text
data: {"event":"token","data":"這個問題超出我目前能回答的範圍…"}
data: {"event":"done"}
```
非串流：`200 { "user_prompt": "...", "model_response": "這個問題超出…" }`

### 灰色地帶 → disclaimer 當開頭 token
```text
data: {"event":"token","data":"（以下為初步判讀，可能需要進一步確認）\n\n"}
data: {"event":"token","data":"…"}
data: {"event":"done"}
```

### 結構性拒絕（空/超長）→ HTTP 400（且發 `InputRejected` audit）
```json
{ "error": "prompt must not be empty" }
```
> 規則：**語意拒絕回 200 + 拒絕節點**；**結構性拒絕回 400**。兩者都發 audit。

## 測試（移植自 `answer-policy.test.ts` / `injection-patterns.test.ts`）
```rust
#[test] fn refuse_on_injection() { /* warnings 含 prompt_injection_detected → Refuse(PromptInjection) */ }
#[test] fn refuse_off_scope_when_unknown() { /* intent=unknown, conf 0.3 → Refuse(OffScope) */ }
#[test] fn disclaimer_on_gray_confidence() { /* revenue, conf 0.6 (<0.7) → AnswerWithDisclaimer */ }
#[test] fn injection_matches_zh_and_en() { /* 中英「忽略先前指令／system prompt」命中；版本號存在 */ }
```

## 錯誤處理
| PRD 對應 | 觸發 | 行為 |
|---------|------|------|
| US-13 | prompt 空 | 400 + `InputRejected`，不呼叫 LLM |
| US-13 | prompt 超長 | 400 + `InputRejected` |
| US-11/12 | injection 命中 | 拒絕(200) + `Refused`／或 pre-LLM `InputRejected` |
| US-12 | off-scope / intent=unknown | 拒絕(200) + `Refused` |
