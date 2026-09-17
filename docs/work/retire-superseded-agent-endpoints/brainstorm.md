# Brainstorm — 退役被 `/agent/stream` 取代的四條端點

**日期**: 2026-08-17
**Slug**: `retire-superseded-agent-endpoints`

## 起點：一個問錯的問題

這條線起於 reference 文件同步（commit `2d0a64b`）時的一個發現：9 條 HTTP 端點裡，只有
`/agent/stream` 與 `/v1/chat/completions` 會經過 runtime prelude（guardrails / intent /
answer policy / memory / audit）。`/insight`、`/insight/stream`、`/report`、`/report/stream`
四條直接驅動 sub-agent pipeline，對 guardrails 與 audit 零引用
（`handler.rs:120–395` 內無任何一處；`AuditWriter` 只在 `:497` 與 `:946` 建立）。

當時的結論是「這是暴露在外的洞，要把 guardrail 補上」。**這個結論是錯的**，原因見下。

## 探索過的三個方案（全部否決）

| | 做法 | 否決原因 |
|---|---|---|
| A | 四條 handler 原樣呼叫 `plan_stream_turn`，拿全套 prelude | 見 B1：造成嚴重功能倒退 |
| B | 抽窄 prelude，只跑 input_guard + injection + audit | 為沒有流量的端點新增平行元件 |
| C | 照 plan §9 把 pipeline 收到 runtime `AgentPort` 後面 | 範圍失焦；且不解決「端點本身冗餘」 |

方案 A 曾一度定案（含兩個附帶決策：REST refusal 回 200 + copy 放 `model_response`；
四條改為 require runtime 回 503），後由 review 推翻。

## 推翻方案 A 的三項查證

### B1：answer policy 會讓這四條開始拒絕它們今天答得出來的問題

`RuleAnswerPolicy::decide`（`guardrails/answer_policy.rs:55`）在
`intent == "unknown" || confidence < answer_gray` 時回 `Refuse("off_scope")`。
而 intent 來自 `config/runtime/intents.toml` 的小型關鍵字包（5 個意圖，每個 4–8 個關鍵字），
未命中者拿 `unknown_confidence = 0.25`（`thresholds.toml`），低於 `answer_gray = 0.5`。
唯一的救援機制 LLM normalizer 在 production 是 `enabled = false`（`config/config.toml:126-128`）。

具體例子：`月報` 不包含 `報告` 這個子字串、`營運` 不包含 `營收`。所以「本月月報」、
「上週營運狀況」、「台北站使用率」今天 `/insight` 都會正常回答，接上 prelude 後全部變成
「這個問題超出我目前能回答的範圍。」

同時，換來的防護是 `config/runtime/injection.toml` 的 **3 條 regex**，該檔自己標 `placeholder`。
**代價遠大於收益。**

### 這四條端點查不到呼叫端

已知消費者 falcon-client 只打 `/agent/stream`（串流）、`/greeting`、與 `/agent`（見下）。
repo 內外都找不到打那四條的程式。

### 整合早就發生過，這四條是殘留

commit `5ab1b7f`（2026-07-13）訊息即為
「Runtime intent routing via new POST `/agent/stream`, 2 agent pipelines are now reachable
through unified pipeline」。falcon 端對應的整合是 `6fb42cc`（2026-07-21，統一串流至
`/agent/stream`）與 PRD v2.2.0（`a0ca63a`）。

換言之 `/insight/stream` 與 `/report/stream` 是 `/agent/stream` 的**強制 pipeline 版本**——
`/agent/stream` 用同一組 `insight_frames`、同一組 frame，只多了 intent 路由，功能是嚴格超集。

## 連帶發現：falcon 的 REST 退路已斷五週

falcon `agent-client.ts:73` 的 `callAgent` 打 `POST <base>/agent`，而該路由在 `ea2bcef`
（**2026-07-11**）被 `/insight` 取代。falcon 在 **2026-07-21**（`6fb42cc`）還動過同一個檔案，
統一了串流卻把 `/agent` 呼叫原封不動留下。

觸發條件是 `NEXT_PUBLIC_COS_STREAMING=false`（`useChiefOfStaffStream.ts:18`，預設 `true`），
所以正常使用不會踩到——這也解釋了為什麼斷了五週沒人發現。

**兩個 repo 各自都有一條寫在文件裡、實際上走不通的退路**：
Rust 端的 `RUNTIME_ENABLED=false` →「改用 `/insight/stream`」（但那會繞過 guardrail），
falcon 端的 `NEXT_PUBLIC_COS_STREAMING=false` →「切回 legacy `/agent`」（404）。

## 定案：刪除，不是補強

**guardrail 的覆蓋缺口用刪除關閉，不寫新程式。**

刪掉四條之後剩 5 條（`/health`、`/ready`、`/greeting`、`/agent/stream`、
`/v1/chat/completions`），其中兩條會吃 user prompt 的都已有完整 prelude → 覆蓋率 100%，
且完全不觸發 B1。

非串流需求若日後出現，`/v1/chat/completions` 已提供（且有 guardrail）。

falcon 端同步退役 REST 路徑：`callAgent`、`submitRestRequest`、`nodes/route.ts` 的 POST、
`NEXT_PUBLIC_COS_STREAMING` flag。

### 使用者決策紀錄

- 2026-08-17：方案 A（完整 prelude）→ 使用者選定
- 2026-08-17：REST refusal 回 200 + copy 放 `model_response` → 使用者選定
- 2026-08-17：四條 require runtime 回 503 → 使用者選定
- 2026-08-17：以上三項因 B1 與「無呼叫端 / 已被取代」而作廢；使用者選定
  **「1 不留」= 不保留非串流路徑，四條全刪**

## Gate B1

| 檢核項目 | 狀態 |
|---|---|
| 問題定義 | 四條端點是 `/agent/stream` 整合後的殘留，無呼叫端、無 guardrail、無 audit ✓ |
| 範圍邊界 | 見 prd.md §1 ✓ |
| 技術可行 | 刪除範圍已逐符號查證（見 prd.md FR-002），無測試綁定那四條 route ✓ |
| 使用者確認 | 「1 不留」✓ |
