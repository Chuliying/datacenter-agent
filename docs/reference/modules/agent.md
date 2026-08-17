# 模組：`agent`（sub-agent 層）

> ← 回 [模組總覽](./index.md) ｜ [專案主體](../index.md)
>
> **Source**：[`src/agent/mod.rs`](../../../src/agent/mod.rs)（“The sub-agent layer: config-driven and code-driven agents composed into pipelines”）

## 職責

把過去單一 prompt 的 monolith turn（一次做完 fetch + analyse + chart）拆成**可組合的
sub-agent pipeline**。每個 stage 是「payload 的純 async function」，可單獨單元測試，
組起來產生同樣的端到端行為。

這一層是三份 sibling contract（`agent_payload` / `tool` / `sub_agent`）的移植，一個 concern 一個
submodule。

> **重要：這一層與 runtime 是平行的兩套編排。**
> `/insight`、`/report` 系列端點**直接**驅動本層，繞過 [runtime turn](./runtime-turn.md) 的
> guardrails / intent / memory / audit。只有 [`/agent/stream`](../endpoints/agent-stream.md) 與
> [`/v1/chat/completions`](../endpoints/chat-completions.md) 會先過 runtime prelude 再進本層。
> 把 pipeline 收到 runtime `AgentPort` 之後是 plan §9 的工作，**目前尚未進行**。

## 子模組

| 檔案 | 職責 |
|---|---|
| [`payload.rs`](../../../src/agent/payload.rs) | `AgentPayload` sum type、抽象 `Tool` / `LlmCapability` capability、`ToolOutcome` 重試模型、tool-use 迴圈 `run_llm_loop`。移植 `agent_payload` contract |
| [`tools.rs`](../../../src/agent/tools.rs) | 封閉的邏輯 `ToolId`、泛型驗證用 `SchemaTool<T>`、MCP-backed `McpTool`。移植 `tool` contract。**注意**：contract 的 `ToolRegistry` 也在此，但目前 dormant（見下方 boot 驗證段） |
| [`config.rs`](../../../src/agent/config.rs) | 作者撰寫的 `SubAgentConfig` 表面、`Provider` / `LlmConfig` 模型、boot 解析規則（`resolve_llm`、`effective_output`）。移植 `sub_agent` contract PART A |
| [`engine.rs`](../../../src/agent/engine.rs) | `SubAgent` trait（統一 config-defined 與 code-defined）、`ConfiguredAgent`、`HelloWorld` 範本、多 pipeline `Orchestrator`。移植 `sub_agent` contract PART B |
| [`llm.rs`](../../../src/agent/llm.rs) | 具體 `OpenAiLlm`（buffered）與 `StreamingOpenAiLlm`（逐 token）adapter，把 `ResolvedLlm` 變成 `LlmCapability` |
| [`events.rs`](../../../src/agent/events.rs) | 串流事件模型：單一注入式 `EventSink` 承載單一 tagged `AgentEvent` |
| [`clock.rs`](../../../src/agent/clock.rs) | 時間作為注入式 capability（`Clock` / `SystemClock`）＋共用 `# Current Time` header |
| [`chart.rs`](../../../src/agent/chart.rs) | **falcon-chart** 協定（`ChartBatch` / `FalconChart`）：`charter` 的 `emit_chart` sink 驗證、`finalizer` 渲染 |
| [`report.rs`](../../../src/agent/report.rs) | **report data** 協定（`ReportData`）：`composer` 的 `emit_report` sink 驗證、`renderer` 注入模板 |
| [`pipeline.rs`](../../../src/agent/pipeline.rs) | `/insight` 與 `/report` 兩條 pipeline 的組裝，含純邏輯的 `render_report` / `render_report_html` |
| [`wiring.rs`](../../../src/agent/wiring.rs) | production 組裝：`build_insight_pipeline` / `build_report_pipeline` / `build_greeting_pipeline` |

## 核心抽象

### `AgentPayload` — stage 之間流動的值

```
Initial(InitialPrompt) ──▶ Intermediate(IntermediateData) ──▶ Final(FinalResult)
```

`PayloadKind` 是便宜的 variant tag，讓 acceptance 檢查與 mismatch 錯誤不必攜帶整個 payload。
每個 `SubAgent` 用 `accepts()` 宣告它吃哪些 variant，不符就走 mismatch 檢查失敗。

`InitialPrompt` 帶 `prompt`、`history`（`Exchange` 列表）與 `now`。

### `SubAgent` — 統一 config-defined 與 code-defined

```rust
async fn run(&self, input: AgentPayload) -> Result<AgentPayload, AgentError>
```

一個 sub-agent 是最多三個 optional 元件——**LLM**、**Tools**、**Logic**——藏在同一個 trait 後面。
`ConfiguredAgent` 的 Logic 是內建的 LLM tool-loop；`HelloWorld` 的 Logic 是任意 Rust。
`Orchestrator` 把 payload 串過選定的 pipeline，**分辨不出**某個 stage 是哪種來源。

`Orchestrator` 內部就是 `HashMap<PipelineId, Vec<Arc<dyn SubAgent>>>`，兩個入口：
`run`（buffered）與 `run_emitting`（串流）。

### `Tool` — 一個抽象、三種 backend

一個 tool 是「LLM 可呼叫的具名 capability，其結果填進某個 artifact slot（`target`）」。
涵蓋三類 backend：MCP 資料抓取、驗證模型自身結構化輸出的 **sink**、純 **validator / compute**。
三者被 tool-use 迴圈以完全相同的方式 dispatch，也以完全相同的方式受 agent 的 grant 隔離。

`target` 由編排設計者接線，**永遠不由 LLM 選擇**。

`ToolOutcome` 只有兩種：

- `Produced(ArtifactValue)` — 成功，值填進 `target` artifact slot。
- `Rejected { reason }` — 被拒（例如 schema 驗證失敗）。以 tool message 回饋給模型，
  不記錄 artifact，可在迴圈的 step cap 內重試。

**malformed 輸出因此不會 crash**，而是被退回讓模型重寫，直到合法為止。

### `EventSink` / `AgentEvent` — 串流即 effect sink

把「串流 agent 的思考與用工具過程」建模成 **effect sink**，而不是 LLM 的功能。
三個邊界往同一條有序 sink 上發事件：

| 來源 | 事件 |
|---|---|
| streaming LLM adapter | `ContentDelta` / `ReasoningDelta`、模型的 tool-call 意圖 |
| tool wrapper（`StreamingTool`） | tool 開始 / 產出 / 被拒 |
| orchestrator（`run_emitting`） | `StageStarted` / `StageProduced` / `StageFinished` |

關鍵設計：stage 轉換是從 `SubAgent::run` **外部**發出的，所以 normative 的
`run(payload) -> Result<payload>` morphism 保持不變。

`StageFinished` 帶 `outcome`（success / failure），`Failure` 後面**必定**跟一個終止性的
`AgentEvent::Error`。

事件如何映射成外部 SSE frame 見 [`/insight/stream`](../endpoints/insight-stream.md#sse-frame)。

### `Clock` — 時間作為注入式 capability

LLM 沒有時鐘：拿到「結束於當前這個進行中月份」的營收，它會把不完整的數字讀成真的下跌。
解法是在對話裡明說「現在幾點」。

在 payload contract 下，`now` 是**turn 資料**：`Clock` 在邊界戳一次進 `InitialPrompt.now`，
之後原樣穿過每個 stage，每個 stage 各自渲染它。因此時鐘只在邊界讀取，
**固定 clock 下整條 pipeline 是決定性的**——這是可測試性的關鍵。

## 三條 pipeline

### `/insight`（四階段）

| Stage | Kind | 讀 | 產出 | Payload shape |
|---|---|---|---|---|
| `fetcher` | `ConfiguredAgent` + MCP tools | user prompt | `fetcher.{tool_name}`——每個授權 tool 一個獨立 slot（`ArtifactKey::new(stage, name)`） | Intermediate |
| `analyst` | `ConfiguredAgent`，無 tools | 上一階段的 `fetcher.*` artifacts | `analyst.message`（散文） | Intermediate |
| `charter` | `ConfiguredAgent` + `emit_chart` sink | analyst 的分析 | 驗證過的 `ChartBatch` | Intermediate |
| `finalizer` | 純邏輯（`render_report`） | 以上全部 | 報告 + 內嵌 `falcon-chart` block | Final |

### `/report`（四階段）

前兩段相同（analyst 換用 `report_analyst_system` prompt），後兩段改為：

| Stage | Kind | 產出 |
|---|---|---|
| `composer` | `ConfiguredAgent` + `emit_report` sink | 驗證過的 `ReportData` |
| `renderer` | 純邏輯（`render_report_html`） | 自足 HTML，包在 `falcon-report` block |

**設計核心經濟性**：rendered report 約 99% 是靜態的（design-system CSS、版面骨架、
建 KPI/表格/圖表的 client-side JS），唯一會變的是一小塊 JSON。因此 composer 只吐
`ReportData`，由 renderer escape 後注入 boot 載入模板的單一 `__REPORT_DATA_JSON__` placeholder。
**沒有任何 LLM 產生 HTML** → 更快、更省 token、每輪設計穩定。

### greeting（兩階段）

`build_greeting_pipeline`：`fetcher → analyst`，用 `greeting_fetcher_system` /
`greeting_analyst_system` 兩份 prompt。由 [`server::greeting`](./server.md) 在啟動時的背景
task 呼叫，預先產生問候語。

## Config 驅動的 tool grant

tool grant **來自 config，不是 code**：`config/config.toml` 的 `[insight.grants]`
對應 `crate::config::InsightGrants`：

| 欄位 | 意義 |
|---|---|
| `fetcher` | fetcher 可用的資料 tool（MCP wire 名稱，或 `["*"]` 代表所有 boot 探索到的） |
| `charter` | charter 可用的 tool（正常就是內建的 `emit_chart` sink） |

`/report` 的 fetcher 重用同一份 `[insight.grants].fetcher`。改 grant **不需重新編譯**。

## Boot-time 驗證

這一層刻意把失敗往前推到啟動時：

- **grant 完整性檢查**由 `validate_insight_grants`（`wiring.rs`）負責，於
  [`AppState`](../index.md#4-啟動與組裝top-level-接線) 組裝時呼叫。某個 stage 被授予
  server 從未 advertise 的 tool，會在**啟動時**就失敗，不會等到某次請求。
- `resolve_llm` / `effective_output` 的每個解析失敗都在 boot 浮現，**早於任何一次 LLM 呼叫**。
- secret 用 k8s 風格綁定。

> **`ToolRegistry` 目前 dormant。** contract 定義的 backend-agnostic registry 已移植，
> 但 `ToolRegistry::new()` 全 crate 只出現在 `tools.rs` 的 `#[cfg(test)]` 內。
> production 的 tool 解析走 `build_stage_tools` / `build_tool`（`wiring.rs`），
> 不經過 registry。上面描述的 fail-fast 行為是真的，但**機制不是 registry**。

## 已知邊界與後續

- **繞過 runtime**：直接 pipeline 端點沒有 guardrails / intent / memory / audit，
  `AgentResponse.intent` 硬寫 `"unknown"`。收到 `AgentPort` 後面是 plan §9。
- **async-openai 版本**：`llm.rs` 刻意鎖 **0.40**。contract 的參考 adapter 釘 0.41.1，
  但本 crate 與 production 迴圈都在 0.40，避免為此做 crate-wide bump。
- **事件有損**：`ChannelSink` 用 `try_send`，buffer 滿了就丟；無損 channel 列為後續。
- **未外送事件**：`ToolStarted`、`ToolProduced`、`ReasoningDelta`、`StageProduced` 目前留在內部。
- **報告模板固定**：chart / big-number 標題寫死在模板，動態化見上游
  [issue #8](https://github.com/h-alice/datacenter-agent/issues/8)。

## Contract 出處

- Payload contract — `.spec/contract/agent_payload`
- Tool contract — `.spec/contract/tool`
- Sub-agent contract — `.spec/contract/sub_agent`
