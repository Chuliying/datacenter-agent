# `POST /ss-report` — 現況契約

> ← [Endpoints](./index.md)  
> **Source**：[`src/server/handler.rs`](../../../src/server/handler.rs) `ss_report_stream` / `authorize_ss_report_route` / `select_ss_report_pipeline` / `wants_full_ss_sweep` / `run_chat_stream`；[`src/agent/wiring.rs`](../../../src/agent/wiring.rs) `build_ss_report_pipeline`；[`src/agent/ss_report.rs`](../../../src/agent/ss_report.rs) 資料協定；[`config/report_template/ss_report.html`](../../../config/report_template/ss_report.html) 模板；[`config/config.toml`](../../../config/config.toml) `[ss_report]`

**星星電力投資平台的 HTML 報表前門**。它與 [`/ss-chat/stream`](./ss-chat-stream.md) 走同一個
identity/權限閘、同一批六支 `ss_*` 工具、同一段 SSE body（`run_chat_stream`），差別只有一件事：
它**固定**跑四階段報表 pipeline（`fetcher → analyst → composer → renderer`），terminal frame 是
一個 ```` ```falcon-report ```` HTML 區塊，而不是 markdown 散文。

這條端點是 [`/agent/stream`](./agent-stream.md) 報表分支的 SS 對應版本。差別在於 `/agent/stream`
會依 intent 在 chat 與 report pipeline 之間選擇；**`/ss-report` 不做這個選擇——端點本身就是報表**。

## Pipeline

| Stage | Kind | 讀取 | 產出 | 工具 | Prompt |
|---|---|---|---|---|---|
| `fetcher` | `ConfiguredAgent` + MCP tools | user prompt | `fetcher.{tool}`（每支工具一格） | `[ss_report.grants].fetcher`（六支 `ss_*`） | `ss_fetcher_system` |
| `analyst` | `ConfiguredAgent`，無工具 | `fetcher.*` | `analyst.message` | — | `ss_report_analyst_system` |
| `composer` | `ConfiguredAgent` + `emit_ss_report` sink | `fetcher.*` + `analyst.message` | `ss_report.data` | `emit_ss_report`（code-backed） | `ss_report_composer_system` |
| `renderer` | 純邏輯，無 LLM | `ss_report.data` + 模板 | `falcon-report` HTML | — | — |

Pipeline id 為 `ss-report`（`ss_report_pipeline_id()`）。`composer` 以
`with_required_output(ArtifactKey::ss_report_data())` 宣告必要產出，因此沒呼叫 sink 就想收尾的
模型會被 nudge 重試，最終失敗也發生在 **composer**，而不是在 renderer 拋一個難解的 missing
artifact。

`fetcher` 與 `composer` 跑在**降低 reasoning** 的 LLM 上（`MECHANICAL_REASONING`），`analyst`
保留預設額度——與 `/report` 完全相同的理由：composer 只是把取回的數字轉寫成固定形狀，全額
reasoning 是浪費，也是 payload 這麼寬時 token 截斷的直接來源。

## 資料協定是 section-shaped，不是 period-shaped

這是 SS 報表與 starcharger 報表**唯一真正的結構差異**，也是為什麼
[`ss_report.rs`](../../../src/agent/ss_report.rs) 是獨立協定而不是 `ReportData` 的泛化：

| | starcharger（`/report`） | 星星電力（`/ss-report`） |
|---|---|---|
| 資料模型 | 一條月度時間序列，每個指標逐月 | 六份**異質快照**，一份對應一支工具 |
| 時間軸 | 所有指標共用 | 三支帶期間、三支是「呼叫當下」 |
| 缺資料時 | period 列裡的一個 `0` | 整個 section 沒有列 |

把 SS 塞進 `periods[]` 骨架，等於替其中四支工具**發明一條它們沒有的月份軸**
（`ss_power_pipeline`、`ss_btm_projects`、`ss_ftm_projects` 是當前階段快照；`ss_sunshine_hours`
的工具說明明確寫著它不是時間序列）。因此 payload 每支工具一個欄位，各自帶自己的期間標籤與列。

`report.periodLabel` 與 `report.snapshotLabel` 是**兩個分開的欄位**：六支工具不共用一個時間窗，
在 header 放單一日期範圍會是資料不支持的斷言。

### 空 section 是正確結果，不是缺陷

每個 section 的內容都是 `Vec`，**空 `Vec` 讓模板隱藏該 section**。這條規則直接來自 starcharger
報表圖表消失問題的教訓（見 CHANGELOG 0.5.2）：固定的 `periods[]` 欄位逼 composer 對沒取到的
主題補零，於是渲染成一張讀者無法與真實零值區分的平坦圖表。

這裡不會。SS 的工具 grant 依呼叫者的 Falcon 權限逐請求收窄，所以**部分報表是常態，不是失敗**：

- 沒取到的工具 ⇒ 該 section 為空 ⇒ 模板隱藏它；
- composer prompt 明文禁止用零或佔位列填充空 section；
- 空掉的 section 必須寫進 `report.dataNote`——section 被隱藏了，那行註記是讀者唯一得知它缺席的地方。

唯一「零是真資料」的地方是漏斗階段：上游把漏斗補齊到固定階段骨架，所以 `quantity: 0` 的階段
要保留。

### FTM 是三個扁平向量，不是嵌套樹

`ss_ftm_projects` 上游回傳 `projects[].stages[]`。協定把它攤平成 `projects` / `stages` /
`receivables` 三個向量，以專案**名稱**字串 join，模板再重新分組。理由是 composer 跑在最低
reasoning：它填三張扁平表遠比填一棵 array-of-object-of-array 可靠——後者正是這類模型會截斷或把
嵌套值誤編成字串的地方。

模板對 join 不上的孤兒列會**單獨開一個標題列出**，而不是靜默丟掉：那是 composer 真的送來的資料，
默默丟棄只會把不一致藏起來。

## 報表 sweep 由關鍵字決定

`/ss-report` 也會被用來問窄問題（「只要日照資料」），對這種請求硬跑六支工具是浪費六次上游呼叫、
還會把答案埋掉。所以**請求自己的用字**決定 fetcher 的 grant 綁多緊，與 `wants_report_pipeline`
決定 `/agent/stream` 跑哪條 pipeline 是同一個作法：

- prompt 命中 `SS_REPORT_SWEEP_KEYWORDS`（`report`、`報告`、`報表`、`完整`、`整體`、
  `各條業務線`，大小寫不敏感）⇒ `FetchTask::Report`：fetcher 被明文要求呼叫**每一支**已授權工具；
- 否則 ⇒ `FetchTask::Chat`：只取問題需要的。

`FetchTask::Report` 附加的 sweep 指令是**針對報表的資料需求**寫的，不是針對工具名稱——grant 會被
權限收窄，活下來的必須全部掃完，沒有任何工具涵蓋的 section 則降級（缺席）而不是被猜出來。

## 權限閘與 `/ss-chat/stream` 相同

`authorize_ss_report_route` 用的是同一組 `[authz].ss_chat_permissions` 權限碼：兩條端點觸及**同一份**
投資平台資料，能聊它的人就能對它出報表，反之亦然。只有**ceiling**不同
（`[ss_report.grants]` 而非 `[ss_chat.grants]`），這讓報表可以用滿整組工具而不必放寬 chat 能觸及的範圍。

沒有 charter stage，所以 charter grant 傳空——報表的圖表是模板從 `ss_report.data` 畫的，沒有
`emit_chart` 需要授權。

它與 `/agent/stream`、`/ss-chat/stream` 同在 streaming nested router 上，因此**identity layer、
per-actor limiter、global burst limiter、bearer gate 一律適用**。這點由
`route_contract::ss_report_route_requires_identity_and_a_startrade_permission` 釘住。

## 記憶體 replay 與 `/ss-chat/stream` 同一家族

memory replay 是 route-family-scoped 的：replay filter 會丟掉 `ss_pipeline` 標籤與當前 route 的
`ss_route` 旗標不一致的 turn。`is_ss_route` 因此同時涵蓋 `/ss-chat/stream` 與 `/ss-report`——
少了這一條，SS 報表端點會靜默變成單輪，而它的 fetcher prompt 卻在指示模型重用先前的 turn。

## 與 `/ss-chat/stream` 共用的部分

answer policy 覆寫（`AlwaysAnswerPolicy`）、prompt injection 拒答、audit、prompt caps、
frame 契約、keep-alive 全部相同，理由見
[ss-chat-stream](./ss-chat-stream.md#與-agentstream-的兩點差異)。SSE body 是同一段
`run_chat_stream`，route 只提供三件事：audit 的 `route` 標籤、answer policy、pipeline 選擇函式。

## 模板

`[ss_report].template`（預設 `report_template/ss_report.html`）。設計 token、panel、table 與 KPI
卡片的 CSS 與 starcharger 模板**逐字相同**，因此兩份報表讀起來是同一套設計系統；SS 額外加的是
結構性的東西：section 分隔線、成對 panel 的兩欄切分、工項狀態 pill、行內進度條。

模板必須含 `__REPORT_DATA_JSON__` placeholder，boot 時檢查（兩份模板都檢查），缺了就**中止啟動**
——否則每個請求都會渲染出一份空報表。

八張圖表（Chart.js，CDN）：各站日照對照、投標容量與得標率、案場服務收入堆疊、發電量與轉供量、
售電漏斗、購電漏斗、BTM 階段容量、FTM 完成率。KPI 卡片由模板從 section 列**計算**而來，不是由
composer 撰寫——這樣 KPI 不可能與它下面的表格不一致，而且沒取到的 section 其 KPI 自然不出現。
