# `POST /report` — 現況契約

> ← [Endpoints](./index.md)  
> **Source**：[`src/server/handler.rs`](../../../src/server/handler.rs) `report`；[`src/agent/wiring.rs`](../../../src/agent/wiring.rs) `build_report_pipeline`；[`src/agent/report.rs`](../../../src/agent/report.rs) `ReportData`；[`src/agent/pipeline.rs`](../../../src/agent/pipeline.rs) `render_report_html`

非串流 HTML 報告端點：跑完四階段 `/report` pipeline，回 renderer 產出的自足 HTML 報告，
包在 `falcon-report` fenced block 內。

## 編排：直接驅動 pipeline

與 [`/insight`](./insight.md) 相同，**不經過 runtime turn**。

```
fetcher ──▶ analyst ──▶ composer ──▶ renderer
（MCP tools）  （純 LLM）  （emit_report）  （純邏輯注入模板）
```

`/insight` 與 `/report` 差在後兩段：`charter` + `finalizer` 換成 `composer` + `renderer`，
且 analyst 用的是另一份 system prompt（`report_analyst_system`）。

### 設計要點：LLM 不寫 HTML

一份 rendered report 約 99% 是靜態內容（design-system CSS、版面骨架、建 KPI/表格/圖表的
client-side JS）。唯一會變的是一小塊 JSON。因此 `composer` 只透過 schema 驗證的 `emit_report`
sink 吐出 [`ReportData`](../modules/agent.md)，由純邏輯的 renderer escape 後塞進 boot 時載入的
模板（`config/report_template/report.html`）的單一 `__REPORT_DATA_JSON__` placeholder。

結果：**沒有任何 LLM 產生 HTML**，所以報告更快、更省 token，每輪的設計也穩定。
shape 不合法會被 `Rejected` 並回饋給模型重試，不會 crash。

## Request

與 [`/insight`](./insight.md) 完全相同的 `AgentRequest`。差異：

- `session_id`、`option_id` 連 tracing span 都不記（handler 的 span 只帶 `prompt_len`、`history_len`）。

## Response

`AgentResponse`，`model_response` 是包在 `falcon-report` block 的 HTML，`intent` 恆為 `"unknown"`。

## Error mapping

與 [`/insight`](./insight.md) 相同（共用 `validate_prompt`、`final_answer`、
`insight_error_to_app_error`），另加：

| 情況 | HTTP | 來源 |
|---|---|---|
| report 模板未載入 / 組裝失敗 | 502 | `build_report_pipeline` 錯誤包成 `BadGateway` |

## Example

```bash
curl -s http://localhost:8080/report \
  -H "Authorization: Bearer $GLOBAL_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"prompt":"做一份本月營運報告"}'
```

## 已知後續

上游 issue [#8 Dynamic HTML report rendering](https://github.com/h-alice/datacenter-agent/issues/8)
要把固定的 chart / big-number 標題改成動態模板，並讓 `emit-report` tool 接受標題。
目前的固定模板是 PR #7 用 system prompt 補的暫解，長期穩定性不足。
