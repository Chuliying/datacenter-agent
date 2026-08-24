# 模組：`runtime::guardrails`

> ← 回 [模組總覽](./index.md) ｜ [專案主體](../index.md)
>
> **Source**：[`src/runtime/guardrails/mod.rs`](../../../src/runtime/guardrails/mod.rs)

## 職責
請求防護與回答前策略：擋掉結構性無效輸入、偵測 prompt injection、決定該正常作答、加提示還是拒絕。

## 結構與現況

子檔案分工見各檔 `//!`（[`mod.rs`](../../../src/runtime/guardrails/mod.rs)、`input_guard.rs`、`injection.rs`、`answer_policy.rs`）。接線現況：

- `input_guard.rs` `validate_prompt`（空／超長）已接線；0.4.0 起錯誤在**建立 Response 前**映射
  HTTP status（400），不再是 200 + error frame。
- `injection.rs` 的版本化 regex set（`InjectionDetector`）已接入 request path 與 memory sanitizer。
- `answer_policy.rs` 的離題拒絕／低信心提示已接線生效。

`InputPipeline::run_with_config` 在 normalize 後執行 detector，產生 `prompt_injection_detected` warning；answer policy 會拒絕且不呼叫 upstream。該拒絕不寫入 session memory，既有 memory context 也使用同一 detector 過濾。

## 設計意圖
讓模型**不對不支援或惡意輸入掰答案**，也**不為拒絕燒 token**：
- 語意拒絕（離題／低信心／injection）→ 拒絕文字當 token 串出，HTTP **200**。
- 結構性拒絕（空／超長）→ HTTP **400**。
- 提示 → disclaimer 當開頭 token。

`AnswerPolicy` 是 trait、由 config 選已註冊後端（目前 `rule`），並由 AppState 接線；refusal/disclaimer thresholds 讀取 capability config。數值範圍與 ordering validation 仍待補。

## 相關
- 何時被呼叫 → [turn](./runtime-turn.md) 流程第 1、4 步
- regex 移植注意（源自 TS 移植）：`/i`→`(?i)`、`\b` 的 word boundary 對 CJK 語意不同、anchor 行為有差——新增 pattern 逐條檢視，不可 verbatim 搬
- 後端組裝 → [registry](./runtime-registry.md)
- 決策後的稽核 → [audit](./runtime-audit.md)
