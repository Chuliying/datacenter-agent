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
| [`injection.rs`](../../../src/runtime/guardrails/injection.rs) | prompt-injection 偵測 | 版本化 regex set（`InjectionDetector`），有單元測試 | 注意：**未接入 request path**（見下） |
| [`answer_policy.rs`](../../../src/runtime/guardrails/answer_policy.rs) | 回答前策略 | `trait AnswerPolicy` → `AnswerDecision`：離題拒絕／低信心加提示／其餘放行 | 已完成 已接線（離題/低信心生效） |

> **injection 偵測目前 dormant**：`InjectionDetector` 只在開機載入時建構並驗證 regex（`config.rs:293`）；真正的 request path `InputPipeline::run_with_config`（pipeline.rs）只跑 `normalize→intent→slots`，**從不呼叫 detector**，因此不會產生 `answer_policy` 依賴的 `prompt_injection_detected` warning。`refuses_prompt_injection_warning` / `versioned_detector_matches_*` 是**單元測試**（人工塞 warning / 直接測 detector），端到端的 injection 拒絕**尚未生效**。原始檔 `//!` 亦標 `skeleton`。

> 注意：上表「狀態」欄為現況標註，非設計目標；接線 detector→warning→policy 是後續工作。

## 設計意圖
讓模型**不對不支援或惡意輸入掰答案**，也**不為拒絕燒 token**：
- 語意拒絕（離題／低信心已生效；injection 待接線）→ 拒絕文字當 token 串出，HTTP **200**。
- 結構性拒絕（空／超長）→ HTTP **400**。
- 提示 → disclaimer 當開頭 token。

> 補充：`RuleAnswerPolicy` 的信心門檻目前**硬編**（`0.5`/`0.7`，answer_policy.rs），未讀 `cfg.thresholds`；intent classifier 則有讀 config thresholds。「門檻外部化」原則於 answer_policy 尚未完全落實。

`AnswerPolicy` 是 trait、由 config 選已註冊後端（目前 `rule`），並由 AppState 接線；threshold 數值本身尚未 config-driven。

## 相關
- 何時被呼叫 → [orchestrator](./runtime-orchestrator.md) 流程第 1、4 步
- regex 移植注意（`\b` 對 CJK 語意）→ [移植 PRD](../../agent-runtime-rust-port/prd.md)
- 後端組裝 → [registry](./runtime-registry.md)
- 決策後的稽核 → [audit](./runtime-audit.md)
