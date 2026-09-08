# 模組：`agent`（sub-agent 層）

> ← 回 [模組總覽](./index.md) ｜ [專案主體](../index.md)
>
> **Source**：[`src/agent/mod.rs`](../../../src/agent/mod.rs)。
> **結構與型別**（子模組職責、`AgentPayload` 狀態機、`SubAgent`/`Tool`/`EventSink`/`Clock` 抽象、
> 三條 pipeline 的 stage 表、falcon-chart 與 report-data 協定）由各檔 `//!` doc comment 擁有，
> 本頁不複述——讀 [`payload.rs`](../../../src/agent/payload.rs)、[`engine.rs`](../../../src/agent/engine.rs)、
> [`tools.rs`](../../../src/agent/tools.rs)、[`events.rs`](../../../src/agent/events.rs)、
> [`clock.rs`](../../../src/agent/clock.rs)、[`pipeline.rs`](../../../src/agent/pipeline.rs)、
> [`chart.rs`](../../../src/agent/chart.rs)、[`report.rs`](../../../src/agent/report.rs)、
> [`wiring.rs`](../../../src/agent/wiring.rs) 的檔頭，或 `cargo doc`。

## 職責

把單一 prompt 的 monolith turn 拆成**可組合的 sub-agent pipeline**；每個 stage 是「payload 的
純 async function」。本層是三份 sibling contract（`agent_payload` / `tool` / `sub_agent`）的移植。

> **本層一律在 runtime prelude 之後執行。**
> 兩個入口 [`/agent/stream`](../endpoints/agent-stream.md) 與
> [`/v1/chat/completions`](../endpoints/chat-completions.md) 都先跑 `plan_stream_turn`
> （guardrails / intent / answer policy / memory / audit），再驅動本層。曾繞過 prelude 的
> `/insight`、`/report` 系列端點已於
> [`retire-superseded-agent-endpoints`](../../work/retire-superseded-agent-endpoints/prd.md) 退役。

## 落差（程式不會自己承認的部分）

- **仍不在 trait seam 之後**：handler 直接呼叫 `agent::wiring`，不經 runtime `AgentPort`。
  把 pipeline 收到 `AgentPort` 之後是 plan §9 的待辦，目前尚未進行。
- **`ToolRegistry` dormant**：contract 定義的 backend-agnostic registry 已移植，但
  `ToolRegistry::new()` 全 crate 只出現在 `tools.rs` 的 `#[cfg(test)]` 內。production 的
  tool 解析走 `build_stage_tools` / `build_tool`（`wiring.rs`），不經過 registry。
  grant fail-fast（`validate_insight_grants` 於 AppState 組裝時，boot 即失敗）是真的，
  但**機制不是 registry**。
- **事件有損**：`ChannelSink` 用 `try_send`，buffer 滿了就丟；無損 channel 列為後續。
- **未外送事件**：`ToolStarted`、`ToolProduced`、`ReasoningDelta`、`StageProduced` 留在內部，
  不映射到外部 SSE。

## 決策與陷阱

- **async-openai 版本**：`llm.rs` 刻意鎖 **0.40**。contract 的參考 adapter 釘 0.41.1，
  但本 crate 與 production 迴圈都在 0.40，避免為此做 crate-wide bump。
- **tool grant 來自 config，不是 code**：`config/config.toml` 的 `[insight.grants]`；
  report pipeline 有自己的上界 `[report.grants].fetcher`（兩者目前都是同樣六個 tool，
  但分開宣告：report 閘是部分授權例外，其餘 intent 是嚴格判定，兩邊必須能各自調整）；
  改 grant 不需重新編譯。
- **兩張表必須合得起來**：`[insight.grants].fetcher` 是上界，`[authz.intent_tools]` 是每個
  intent 的必要 tool。嚴格 intent 少一個 tool 不是收窄而是**對所有使用者的永久拒答**，且外顯成
  「權限不足」。`AuthzConfig::validate_intent_reachability`（AppState 組裝時）因此讓這種漂移
  開機即失敗，`report` 除外——它是部分授權例外，缺 tool 只會降級。
- **報告模板固定**：chart / big-number 標題寫死在模板，動態化見上游
  [issue #8](https://github.com/h-alice/datacenter-agent/issues/8)。
- **`emit_report` 驗兩層，不只驗 schema**：serde 反序列化只證明「形狀」；模板的 client script
  另外依賴跨欄位不變量——`summary.latestCompletedPeriod` 必須等於**最近一個** `partial: false` 的
  `periods[].period`、兩個陣列非空、月份是 `YYYY-MM` 且由舊到新、只有最後一個月可 partial、
  `report.locale` 是 BCP-47 形狀。
  只過 schema 但違反其中一條的 payload，會讓瀏覽器在畫圖前就拋 `TypeError`：報表標頭已填、
  四張圖全白（headless Chrome 可重現）。`ReportData::validate`（`report.rs`）在 sink 端擋下這些，
  rejection reason 直接點名欄位與修法，回饋給 model 重試；rejection 以 `info` 等級寫 log，
  方便從 server log 分辨「model 一直把月份寫錯格式」和「fetch 沒有資料」。模板本身也加了
  最後一道防線：不變量失敗或 render 拋錯時顯示 `role="alert"` 的錯誤橫幅，圖表框改為說明文字，
  不再靜默留白。fetch 沒資料時 composer 被指示**不要**捏造項目；它因此送不出 `report.data`，
  stage 以 `MissingArtifact` 結束（`with_required_output` 會先催三次，這是既有的必要輸出守門，
  composer 的散文本身不會送到使用者）。handler 的 `classify_pipeline_failure` 把 report pipeline
  的這個結果映成穩定 code `report.data_unavailable` 與使用者可讀文案（stream `error` frame、
  OpenAI `502` envelope 皆同），Falcon 端再依 code 顯示對應文案；其他失敗維持 `upstream.error`。
  更早在 fetcher 邊界就偵測「沒資料」、省下兩個 LLM stage，列為後續。

## Contract 出處

- Payload contract — `.spec/contract/agent_payload`
- Tool contract — `.spec/contract/tool`
- Sub-agent contract — `.spec/contract/sub_agent`
