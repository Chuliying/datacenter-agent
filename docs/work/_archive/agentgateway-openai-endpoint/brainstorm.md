---
output: docs/work/agentgateway-openai-endpoint/brainstorm.md
stage: brainstorm
slug: agentgateway-openai-endpoint
---

# Brainstorm — OpenAI 相容端點接入 agentgateway

## 問題定義

讓本服務（datacenter-agent）能以 **agentgateway 的 Path C = OpenAI 相容 LLM backend** 掛上平台 gateway，
對外提供標準 `POST /v1/chat/completions`（含 SSE streaming），**同時不影響**現有 `/agent/stream`
（falcon / 前端依賴其自訂豐富事件）。

## 背景與現況（實際 code）

現有 8 條路由中，`/agent/stream` 是**唯一走 runtime 的生產路徑**（guardrails/intent/memory/audit
prelude）。與 OpenAI contract 有三層硬落差：

1. **輸入**：自訂 `AgentRequest{prompt, history[{user_prompt,model_response}], session_id, option_id}`，
   非 `messages`/`model`/`stream`。（`src/server/dto.rs:27`）
2. **串流**：自訂 `StreamFrame` 信封（`{"event":"token","data":...}`），結尾 `{"event":"done"}`，
   **無** `choices/delta`、**無** `[DONE]`；且串流哲學是「中間各 sub-agent 逐 token 送 preview →
   終局 `clear` 清掉 preview → 重送完整答案（report+charts 或 rendered HTML）」，
   與 OpenAI「累加 delta」不同。（`src/server/dto.rs:66`、`src/server/handler.rs:769-831`）
3. **非串流 body**：`{user_prompt, model_response, intent}`，非 `choices[].message`。（`src/server/dto.rs:39`）

其他事實：
- auth 為 bearer（GLOBAL_TOKEN），相容 OpenAI 形式，但失敗回 `418` 而非 gateway 慣例 `401`。
  （`src/server/auth.rs:54`）
- 聚合式非串流 runtime turn `run_agent_turn` **已存在、未接任何路由**（僅測試用），
  新端點的非串流分支可直接接它。（`src/runtime/turn.rs:194`）

## 決策：方案 A（新端點並存）

| 維度 | A：新增 `/v1/chat/completions`，`/agent/stream` 保留 | B：直接改 `/agent/stream` 成 OpenAI |
|---|---|---|
| 現有 client | 零 breaking | Breaking，全部要改 |
| 豐富事件 | 保留在 `/agent/stream` | 對外消失 |
| path/慣例 | 標準 `/v1/chat/completions` | 不符慣例 |
| 「原樣搬」判準 | ✅ | ❌ |
| 維護 | 兩套對外序列化（內部單一） | 一套 |

**選 A**。理由：契合既有判準（原樣搬、stream 為重點、REST/stream 同組件）；gateway 只需標準 OpenAI
content 串流，而 intent/stage/tool_call 是系統價值，B 會使其對外蒸發。使用者已確認前端/falcon
依賴那些豐富事件 → 不可 breaking。

## 範圍邊界

**做**：
- 新增 `POST /v1/chat/completions`（`stream` 欄位決定串流與否，符合 OpenAI）。
- 共用同一 `plan_stream_turn` prelude + sub-agent pipeline；非串流分支接 `run_agent_turn`。
- request：`messages` → 內部 `prompt`（最後一則 user）+ `history`（前面配對）。
- response：token → `choices[].delta.content` + 結尾 `[DONE]`；非串流 → `choices[].message`。

**不做（YAGNI）**：
- 不改 `/agent/stream` 及其他現有端點。
- 不支援 OpenAI function-calling / tools 欄位（gateway Path C 不需要）。
- 不把豐富事件（intent/stage/tool_call/clear）塞進 OpenAI 端點做非標準擴充。

## 交給 spec 的已知技術點

1. **串流語意映射（核心）**：現有「preview + clear 重送」如何對到 OpenAI 累加 delta。
   兩條路待 spec 查 pipeline 定案：
   - 真串流：只轉發「最終回答」的 token（忽略中間 stage preview，Finished 不再重送）。
   - 偽串流：Finished 拿到完整答案後於映射層分塊送 delta（首 token 延遲 = 完整計算時間）。
2. auth 回應碼 `418` → 是否對齊 gateway 慣例 `401`。
3. gateway 對接參數（baseUrl=`http://host:port/v1` / model / key）與 SSE 外層代理設定
   （proxy_buffering off、拉長 timeout、不壓縮 text/event-stream）。

## 開放問題（規格書待確認事項）

- RD agent 框架（此處即本服務，Rust axum）— 已知，非阻礙。
- gateway 是否需額外 authn/authz。
- gateway 前是否有反向代理（影響 SSE）。

## Gate B1

問題定義 / 範圍邊界 / 技術可行 / 使用者確認 — 皆 PASS（使用者：「確認 A，進 spec」）。
