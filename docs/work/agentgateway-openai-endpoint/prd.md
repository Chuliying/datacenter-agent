---
output: docs/work/agentgateway-openai-endpoint/prd.md
stage: prd
slug: agentgateway-openai-endpoint
kind: to-prd-snapshot
---

# PRD — OpenAI 相容端點接入 agentgateway

> 快照式 PRD（to-prd）：由 [brainstorm.md](brainstorm.md) + 使用者提供的 agentgateway 接入規格書
> + 實際 code 理解萃取，未重新訪談。方案決策見 brainstorm.md。

## 目標與對象

- **對象**：平台端 agentgateway（RD = 本服務 datacenter-agent）。
- **目標**：讓本服務以 agentgateway **Path C（OpenAI 相容 LLM backend）** 掛上 gateway，
  對外提供標準 `POST /v1/chat/completions`（含 SSE streaming），由 gateway 統一路由/認證/governance/可觀測性。
- **決策**：採**方案 A**——新增端點並存，`/agent/stream` 與其豐富事件原樣保留（falcon/前端依賴）。

## 功能需求（FR）

- **FR1 新端點**：新增 `POST /v1/chat/completions`，接受 OpenAI Chat Completions request（`messages[]`、`model`、`stream`）。
- **FR2 request 映射**：`messages` → 內部 `AgentRequest`——最後一則 `user` → `prompt`；其餘 user/assistant 配對 → `history[{user_prompt, model_response}]`；`system` 訊息處理由 spec 定。
- **FR3 串流輸出**：`stream:true` 回標準 OpenAI SSE——`data: {object:"chat.completion.chunk", choices[].delta.content}`，結尾 `data: [DONE]`，`Content-Type: text/event-stream`。
- **FR4 非串流輸出**：`stream:false` 回標準 OpenAI response——`{choices[].message.content, usage}`；接現成但未接線的聚合 `run_agent_turn`。
- **FR5 內部能力對齊**：走與 `/agent/stream` 同一 `plan_stream_turn` prelude（audit/guardrails/intent/answer-policy/memory）+ sub-agent pipeline。
- **FR6 相容性**：`/agent/stream` 及其他 7 條路由行為完全不變。
- **FR7 認證**：沿用現有 bearer（gateway 存 key、統一對外認證）；回應碼是否由 418 對齊 gateway 慣例 401，見 FU4。

## 驗收標準（AC，Given-When-Then）

- **AC-1 非串流**：Given gateway 以 OpenAI 格式打 `/v1/chat/completions`（`stream:false`），When 正常查詢，Then 回傳符合 OpenAI schema 的 `choices[0].message.content`（非空）與 `usage`。
- **AC-2 串流**：Given `stream:true`，When 查詢，Then client 逐塊收到 `data: {chunk, delta.content}`，累加即完整答案，最後收到 `data: [DONE]`。
- **AC-3 不破壞現有**：Given 現有 `/agent/stream` client（falcon/前端），When 新端點上線後，Then `/agent/stream` 的自訂事件（intent.resolved/stage/tool_call/clear/done）行為不變。
- **AC-4 規格書驗證**：Given 規格書 C-3 的 curl（串流與非串流），When 執行，Then 皆通過。
- **AC-5 governance 一致**：Given guardrails 應拒絕的輸入（injection），When 打新端點，Then 走同一 prelude 並適當拒絕，與 `/agent/stream` 一致。

## 錯誤場景（ERR）

- **ERR1 runtime 未啟用**：`RUNTIME_ENABLED=false` 時新端點行為（現有 `/agent/stream` 回 503）——回 OpenAI 風格 error 或 503，spec 定（見 FU5）。
- **ERR2 無效 request**：`messages` 空 / 缺 role/content / 非法 JSON → OpenAI 風格 error（400）。
- **ERR3 prompt 超長**：超過現有長度上限（legacy 2000 / runtime EV-pack 4000）→ error。
- **ERR4 auth 失敗**：現有回 418；gateway 慣例 401（見 FU4）。
- **ERR5 上游能力失敗**：LLM/MCP capability 失敗（現有映射 502）→ 對應 OpenAI 風格 error。

## 非功能需求（NFR）

- **串流延遲**：首 token 延遲取決於映射策略（真串流 vs 偽串流，見 FU3）。
- **SSE 完整性**：若 gateway 前有反向代理，須 `proxy_buffering off`、拉長 timeout、不壓縮 `text/event-stream`（見 FU2；屬部署但影響 AC-2/AC-4）。
- **無 rate-limit**：現況無 rate-limit middleware，不在本次範圍新增。

## 範圍邊界

**做**：新增 `/v1/chat/completions`（串流 + 非串流）、request/response 映射、共用 runtime prelude + pipeline。
**不做**：不改現有端點；不支援 OpenAI function-calling / tools 欄位；不把豐富事件塞進 OpenAI 端點做非標準擴充；不自架 gateway（平台端負責）。

## Follow-ups（皆非阻擋型；FU3/4/5 交 spec 技術決策，FU1/2 為部署協調）

- **FU3（spec-resolved）**：串流語意映射——「只串最終回答 token（真串流）」vs「Finished 後分塊送（偽串流）」，spec 查 pipeline 定案。
- **FU4（spec-resolved）**：auth 回應碼 418 → 是否對齊 401。
- **FU5（spec-resolved）**：runtime 未啟用時新端點行為（ERR1）。
- **FU1（deployment）**：gateway 是否需額外 authn/authz。
- **FU2（deployment）**：gateway 前是否有反向代理（影響 SSE）。

## 版本歷史

| 時間 | 內容 | 對應版本 | 作者 |
|------|------|---------|------|
| 2026-07-22 | 初版（to-prd 快照，from brainstorm + 規格書） | brainstorm v1 | Chuliying + Claude |
