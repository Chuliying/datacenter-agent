# 模組：`runtime::guardrails`

> ← 回 [模組總覽](./index.md) ｜ [專案主體](../index.md)
>
> **Source**：[`src/runtime/guardrails/mod.rs`](../../../src/runtime/guardrails/mod.rs)

## 職責
請求防護與回答前策略：擋掉結構性無效輸入、偵測 prompt injection、決定該正常作答、加提示還是拒絕。

## 子檔案

| 檔案 | 職責 | 關鍵項 | 狀態 |
|---|---|---|---|
| [`input_guard.rs`](../../../src/runtime/guardrails/input_guard.rs) | 結構防護 | `validate_prompt`：空／超長 → 在 LLM/MCP 前拒絕並 audit | 部分完成 已接線；runtime SSE 外部 status 是 200 + error frame |
| [`injection.rs`](../../../src/runtime/guardrails/injection.rs) | prompt-injection 偵測 | 版本化 regex set（`InjectionDetector`） | 已接入 request path 與 memory sanitizer |
| [`answer_policy.rs`](../../../src/runtime/guardrails/answer_policy.rs) | 回答前策略 | `trait AnswerPolicy` → `AnswerDecision`：離題拒絕／低信心加提示／其餘放行 | 已完成 已接線（離題/低信心生效） |

`InputPipeline::run_with_config` 在 normalize 後執行 detector，產生 `prompt_injection_detected` warning；answer policy 會拒絕且不呼叫 upstream。該拒絕不寫入 session memory，既有 memory context 也使用同一 detector 過濾。

## 設計意圖
讓模型**不對不支援或惡意輸入掰答案**，也**不為拒絕燒 token**：
- 語意拒絕（離題／低信心／injection）→ 拒絕文字當 token 串出，HTTP **200**。
- 結構性拒絕（空／超長）→ HTTP **400**。
- 提示 → disclaimer 當開頭 token。

`AnswerPolicy` 是 trait、由 config 選已註冊後端（目前 `rule`），並由 AppState 接線；refusal/disclaimer thresholds 讀取 capability config。數值範圍與 ordering validation 仍待補。

## 相關
- 何時被呼叫 → [turn](./runtime-turn.md) 流程第 1、4 步
- regex 移植注意（`\b` 對 CJK 語意）→ [移植 PRD](../../agent-runtime-rust-port/prd.md)
- 後端組裝 → [registry](./runtime-registry.md)
- 決策後的稽核 → [audit](./runtime-audit.md)
