# `POST /report/stream` — 現況契約

> ← [Endpoints](./index.md)  
> **Source**：[`src/server/handler.rs`](../../../src/server/handler.rs) `report_stream`；[`src/server/dto.rs`](../../../src/server/dto.rs) `StreamFrame`

[`POST /report`](./report.md) 的 SSE 版本，跑四階段 `fetcher → analyst → composer → renderer`
pipeline。

## 與 `/insight/stream` 的關係

**wire 契約完全相同**——同一組 `StreamFrame`、同樣的 `clear` → `token` → `done` 終局協定、
同樣的 keep-alive 與 buffer 行為。細節請直接看
[`/insight/stream`](./insight-stream.md) 的 SSE frame 一節，此處不重複。

差別只在**訊號的性質**：

| | `/insight/stream` | `/report/stream` |
|---|---|---|
| 中段 `token` | analyst 的散文即時串流，內容有意義 | `composer` 產的是 tool call 而非可串流散文，中段幾乎沒有有意義的 prose |
| 主要即時訊號 | token 串流 | **每個 stage 的 `stage` 進度 frame** |
| 終局 `token` | 報告 + charts | 完成的 HTML（`falcon-report` block） |

換句話說，`/report/stream` 的使用者體驗靠的是進度指示燈，不是逐字浮現的文字；
完成的 HTML 只會在終局那一筆 frame 一次到齊。

## Request / response

- Request body 與 [`/report`](./report.md) 相同。
- Bearer required（失敗 418）。
- 不經過 runtime turn。
- 空／超長 prompt 在建立 stream 前回 HTTP 400。

## Example

```bash
curl -N http://localhost:8080/report/stream \
  -H "Authorization: Bearer $GLOBAL_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"prompt":"做一份本月營運報告"}'
```

## Coverage gaps

同 [`/insight/stream`](./insight-stream.md#coverage-gaps)。
