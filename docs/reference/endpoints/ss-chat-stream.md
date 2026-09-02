# `POST /ss-chat/stream` — 現況契約

> ← [Endpoints](./index.md)  
> **Source**：[`src/server/handler.rs`](../../../src/server/handler.rs) `ss_chat_stream` / `select_ss_chat_pipeline` / `run_chat_stream`；[`src/agent/wiring.rs`](../../../src/agent/wiring.rs) `build_ss_chat_pipeline`；[`src/runtime/guardrails/answer_policy.rs`](../../../src/runtime/guardrails/answer_policy.rs) `AlwaysAnswerPolicy`；[`config/config.toml`](../../../config/config.toml) `[ss_chat.grants]`

**星星電力投資平台的串流前門**：與 [`/agent/stream`](./agent-stream.md) 完全相同的四階段 chat
pipeline（`fetcher → analyst → charter → finalizer`），但跑在六支 `ss_*` 投資平台工具上，
使用自己的 stage prompt。

兩支端點的 SSE body 是**同一段程式碼**（`run_chat_stream`），route 只提供三件事：audit 的
`route` 標籤、answer policy、以及 pipeline 選擇函式。因此 frame 契約、keep-alive、
bearer gate、burst limiter 行為不可能與 `/agent/stream` 漂移。

## Pipeline

| Stage | Kind | 工具 | Prompt |
|---|---|---|---|
| `fetcher` | `ConfiguredAgent` + MCP tools | `[ss_chat.grants].fetcher`（六支 `ss_*`） | `ss_fetcher_system` |
| `analyst` | `ConfiguredAgent`，無工具 | — | `ss_analyst_system` |
| `charter` | `ConfiguredAgent` + `emit_chart` sink | `[ss_chat.grants].charter` | `ss_charter_system` |
| `finalizer` | 純邏輯，無 LLM | — | — |

Pipeline id 為 `ss-chat`（`ss_chat_pipeline_id()`），與 insight pipeline 的 `agent` 區分，
stage trace 因此指得出實際跑的是哪一條。

### 工具 grant 是顯式清單，不是 `"*"`

**同一台 MCP server 同時提供 EV 充電工具與 `ss_*` 工具**（boot 時共 discover 12 支）。
`[insight.grants].fetcher` 用不用 `"*"` 都無妨，但 `[ss_chat.grants].fetcher` **必須是顯式的
六支 `ss_*`**：一旦寫成 `"*"`，SS fetcher 就拿得到 `bill_revenue` 這類充電工具，而 SS analyst
會在星星電力的品牌下對充電資料下結論。

這條不變量由 `config::tests::ss_chat_fetcher_grant_never_uses_the_wildcard`（同時檢查 shipped
config 與 in-code default）與 live test 的 tool 斷言共同釘住。

grant 於 boot 以 `validate_ss_chat_grants` 對 discovered set 驗證，typo 或 server 未提供的
`ss_*` 工具會**中止啟動**，而非在第一個請求才失敗。

## 與 `/agent/stream` 的兩點差異

### 1. 不做 intent filtering

runtime 的 intent pack（[`config/runtime/intents.toml`](../../../config/runtime/intents.toml)）
描述的是 EV 充電領域。SS 問題在它底下多半落到 `unknown`，而配置的 `RuleAnswerPolicy` 會把
`unknown` / 低信心一律以 `off_scope` 拒答——**pipeline 根本不會被執行到**。

本端點因此改用 `AlwaysAnswerPolicy`：

- **保留** prompt injection 拒答（那是安全 guardrail，不是 intent filtering）；
- **放寬** scope gate，`unknown` 與低信心都照常回答；
- intent 仍然解析、仍然送出 `intent.resolved` frame、仍然寫入 audit——只是不再決定任何事。

prelude 的其餘部分（prompt 長度上限、injection 偵測、audit）**完全不變**。session memory
的讀寫也走同一條路，但 SS turn 以 `ss_pipeline` 標記寫入：SS 的 intent 一律是 `unknown`
（EV intent pack 沒有 SS 詞彙），沒有這個標記，重放過濾器會把每一筆 SS turn 當低信心
turn 丟棄——端點會靜默變成單輪。重放是**路由族隔離**的：SS turn 只在本路由重放（授權依
SS gate 判定），EV turn 不會滲入本路由，反之亦然。

實測（2026-08-27，同一個問題）：

| 端點 | resolved intent | 結果 |
|---|---|---|
| `/agent/stream` | `unknown` | `token: 這個問題超出我目前能回答的範圍。` → `done` |
| `/ss-chat/stream` | `unknown` | 正常呼叫 `ss_power_pipeline` / `ss_btm_projects` 後作答 |

### 2. 不做 pipeline routing

目前只有一條 SS pipeline，report 版本是後續步驟，因此 `wants_report_pipeline` **刻意不被呼叫**。
`select_ss_chat_pipeline` 保留共用的 `PipelineSelector` 形狀，之後要加 report 分支時改這裡即可，
不必動共用的串流本體。

## Request / response

與 `/agent/stream` 相同：

- Request body 為 `AgentRequest`（`{prompt, history?, session_id?, option_id?}`）。授權模式下
  `history` 被忽略：server-side session memory 是唯一脈絡來源，與 `/agent/stream` 相同（FR-009）。
- Service bearer required（失敗 418）。
- **使用者身份 required**：`X-Falcon-Authorization: Bearer <FALCON_ACCESS_TOKEN>`。缺失回
  `401` + `identity.header_missing`；token 失效依 `error_code` 白名單回 `identity.token_refreshable`
  （可續期一次）或 `identity.token_terminal`（不得續期）——與 `/agent/stream` 完全同一個身份層。
- **SS 權限 gate**：本路由不做 intent 判定（SS 問題在 EV intent pack 下一律 `unknown`），授權改依
  `[authz].ss_chat_permissions`——持有任一列出的 Falcon 權限碼即解鎖完整 `[ss_chat.grants]`
  fetcher 集合；一個都沒有（含只持 `startrade-power` 父層頁）則在呼叫 LLM／MCP 之前以
  `200` SSE 拒答收場：`token`（拒答文案）→ `refusal`（`code: authz.insufficient`）→ `done`。
- 成功後為 `text/event-stream`，keep-alive interval 15 秒。
- prompt cap 為 runtime config `thresholds.input.max_prompt_chars`（目前 4 000），由 prelude 執行。
- SSE frame 型別與 `/agent/stream` 完全一致（`intent.resolved` / `stage` / `token` / `tool_call` /
  `tool_args` / `usage` / `clear` / `error` / `done`）。
- Standard group 成員，共用 120 s timeout、外層全域 burst limiter（與 `/agent/stream` 同一個 bucket）與**內層 per-actor limiter**（以 `actor_key` 為鍵；超限回 `429` + `rate_limit.actor`）。

Prelude 的三種結果（`Error` / `Refused` / `Proceed`）對外行為與
[agent-stream](./agent-stream.md#prelude-的三種結果) 相同。

## 驗證

Live 端到端測試：[`tests/ss_chat_pipeline.rs`](../../../tests/ss_chat_pipeline.rs)，題組取自
`eomc-mcp/docs/ss_chatbot_agent_test.md` 的七題管理層問題。它驅動的是**production 的
`build_ss_chat_pipeline`**，並讀 `config.toml` 的真實 prompt 與 grant，所以通過即代表 production
wiring 本身可用。

```bash
SS_CHAT_QUESTION=all cargo test --test ss_chat_pipeline -- --ignored --nocapture
```

機械斷言三項：pipeline 產出非空 `Final`；**呼叫過的資料工具全部是 `ss_*`**（`emit_chart` 為
內建 sink，是唯一合法的非 `ss_` 名稱）；analyst 有串出 `ContentDelta`。答覆內容是散文，
由人對照參考文件判讀，測試不評分。

### 已知限制：算術正確性取決於 model

2026-08-27 以 `google/gemini-3.1-flash-lite`（`.env` 的 `OPENROUTER_MODEL`）實跑七題，
五題正確——包含兩項對抗性檢查（拒絕 kWh + kW 相加、主動更正錯誤的計畫名稱）。**兩題出現
加總錯誤**（實際數字略去——這份文件對所有 repo 讀者可見，而端點資料受 `[authz]`
權限閘門保護，具體金額以權限內呼叫工具取得為準）：

- 多列累計題：73 列的兩季累計加總錯誤，偏差約三成（以直接呼叫工具核對）。
- 少列合計題：三筆款項逐列正確，合計卻與逐列相加不符。

同一份 prompt 換成 `anthropic/claude-opus-5` 後兩題皆精確命中，與參考文件逐項相符。
**這是 model 能力上限，不是 pipeline 或 prompt 的邏輯錯誤**；若本端點要對管理層提供
可引用的數字，建議為它指定較強的 model。

`ss_analyst_system` 已針對這兩類錯誤加上硬性規則（單一加總基準、合計必須等於列出的列相加、
佔比分母唯一且不得超過 100%、逐列判斷逾期且未逐列檢查前不得寫「無逾期項目」）。這修正了
`gemini-3.1-flash-lite` 原本誤報「無逾期項目」的問題（實際有一筆已逾期案場），但無法補足
大量列的加總能力。
