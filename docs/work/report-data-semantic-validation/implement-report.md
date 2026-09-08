# Implement Report — report-data-semantic-validation

> Execution Mode：`bug-fix`（flow 修錯：systematic-debugging → implement → verification-before-completion）。
> 條件式輸入：`plan` optional-missing（單一 sink + 模板的修正，不需 canonical plan）；prd / spec / qa-plan `absent`。

## 症狀與根因（systematic-debugging）

**症狀**：`/agent/stream` 走 report pipeline 時，產出的 `falcon-report` HTML 打開後標頭有填、
KPI 卡、月表與四張圖表全部空白。

**根因**：`emit_report` 是 `SchemaTool::sink`，只做 serde 反序列化——證明的是形狀，不是語意。
模板 `config/report_template/report.html` 的 client script 在畫圖之前先做
`data.periods.findIndex(item => item.period === data.summary.latestCompletedPeriod)`，
再直接讀 `completed.period`。以下 payload 全部過 schema、全部讓該行拋
`Uncaught TypeError: Cannot read properties of undefined`，script 中止，圖表程式碼從未執行：

| payload | 結果（headless Chrome，`--dump-dom` + console） |
|---|---|
| 正常資料 | console 乾淨；KPI 卡 5、圖 4 |
| `latestCompletedPeriod: "2026-6"`，periods 為 `"2026-06"` | TypeError line 753；KPI 0、圖 0 |
| `periods: []` | 同上 |
| `stationRanking: []` | TypeError line 778（`.name`）；KPI 0、圖 0 |
| anchor 指向 partial 月 | 不拋錯，但 KPI 錨在部分月份 |

repro 夾具與 Chrome 輸出：session scratchpad `repro/{good,lcp_mismatch,empty_periods,empty_ranking,lcp_is_partial}.html` 及對應 `.log` / `.dom.html`。

另外 `src/test_support.rs` 的 scripted composer 夾具本身就送 `stationRanking: []`——夾具就是這個 bug 的樣本。

## 變更

| 檔案 | 變更 |
|---|---|
| `src/agent/report.rs` | 新增 `ReportData::validate`（七條跨欄位不變量，reason 點名欄位與修法）、`is_year_month` 與 `is_language_tag`；模組 doc 新增「Two validation layers」；移除已無必要的 `#![allow(dead_code)]` |
| `src/agent/tools.rs` | `emit_report_tool` 由 `sink` 改為 `new`，`on_valid` 先 `validate()` 再序列化；工具 description 補上格式規則 |
| `src/agent/payload.rs` | `run_llm_loop` 的 tool rejection log 由 `debug`（`agent::probe`）升為 `info`，含 tool 名與 reason |
| `config/report_template/report.html` | script 拆成 `checkInvariants` + `renderReport`，兩者都在 try/catch 內；失敗時 `<main>` 前插入 `role="alert"` 橫幅（`data-report-error="true"`），四個圖表框改為說明文字；新增 `.report-error` 樣式 |
| `config/prompt_guide/report_composer_system.md` | 新增「What `emit_report` rejects」一節；資料缺月份或站點時不得送空陣列，改以一句話說明 |
| `src/test_support.rs` | scripted `sample_report_data` 補一筆站點，使 pipeline 能到 renderer |
| `src/server/codes.rs`、`src/server/handler.rs` | （第二輪）`report.data_unavailable` code；`classify_pipeline_failure` 把 report pipeline 的 `missing artifact: report.data` 映成該 code＋使用者文案，三條回傳路徑（`/agent/stream` error frame、OpenAI buffered 502、OpenAI stream in-band）共用 |
| `falcon-client`（分支 `fix/report-data-unavailable-copy`）`streaming-events.ts`、`useChiefOfStaffStream.ts`、`copy.ts` | （第二輪）error frame 型別補 `code?`；hook 以 `event.code ?? event.data` 分流；新增 `reportDataUnavailable` 文案 |
| `docs/reference/modules/agent.md`、`docs/reference/endpoints/{agent-stream,chat-completions}.md`、`CHANGELOG.md`、`docs/work/index.md` | 文件 |

## RED → GREEN → REFACTOR

### 迴圈 1：sink 端語意驗證

- **RED**：`src/agent/tools.rs` 新增
  `emit_report_tool_rejects_schema_valid_payloads_the_template_cannot_render`（八個 case）。
  `cargo test --lib emit_report_tool_rejects` →
  `panicked at src/agent/tools.rs:892:26: expected Rejected, got Produced(Json(...latestCompletedPeriod: "2026-5"...))`
  → 1 failed。原因正確：現況對錨點不匹配的 payload 直接 Produced。
- **GREEN**：實作 `ReportData::validate`、改 `emit_report_tool`。
  `cargo test --lib emit_report` → 2 passed。
- **REFACTOR**：`report.rs` 補五條單元測試（`validate_accepts_the_template_wire_sample`、
  `validate_names_the_field_for_each_unrenderable_invariant`、
  `validate_allows_a_window_with_no_partial_month`、`is_year_month_accepts_only_zero_padded_months`、
  `is_language_tag_accepts_hyphenated_subtags_only`）；
  移除 `allow(dead_code)`。全 lib 測試見下。

### 迴圈 2：模板防線

- **RED**（observed failing one-off test）：舊模板 + 四個壞 payload 於 headless Chrome：
  `Uncaught TypeError` ×1、KPI 卡 0、圖 0、無任何可見錯誤訊息（上表）。
- **GREEN**：加入 `checkInvariants` / `showReportError` / try-catch。同四個夾具重跑：

  | 夾具 | Uncaught | KPI | 圖 | 橫幅 |
  |---|---|---|---|---|
  | good | 0 | 5 | 4 | 無 |
  | lcp_mismatch | 0 | 0 | 0 | `summary.latestCompletedPeriod「2026-6」不在 periods 之中` |
  | empty_periods | 0 | 0 | 0 | `periods 為空` |
  | empty_ranking | 0 | 0 | 0 | `stationRanking 為空` |

### 附帶：夾具修正

首次全 lib 測試 `authorized_report_crosses_runtime_pipeline_with_terminal_degradation_notice` 失敗
（HTTP 502 ≠ 200）：scripted composer 的 `stationRanking: []` 被新驗證擋下。補一筆站點後通過；
該測試檢驑的是 authz 降級文案，不是報表內容。

## 第二輪：fresh fable reviewer（PASS-WITH-FIXES）後的修正

新開 context 的 fable reviewer 自行重現了舊模板全白與新模板橫幅，判 PASS-WITH-FIXES，三條 IMPORTANT：

| # | 發現 | 處置 |
|---|---|---|
| 1 | prompt 叫 composer「一句話說明後停止」，但 `with_required_output` 會催三次再 `MissingArtifact`，且 composer `capture_message: false`，使用者只看到泛用 upstream error；文件卻寫成「一句話說明缺哪些資料」 | 使用者拍板 **Option A**：保留硬失敗、給專屬訊息。新增 code `report.data_unavailable` 與 `classify_pipeline_failure`；prompt 改成誠實描述（不捏造、重複聲明、server 會以資料不足結束）；文件同步改寫。Falcon 端依 code 顯示文案 |
| 2 | `report.locale` 沒驗，`zh_TW` 過 sink 後瀏覽器 `RangeError` | `validate` 第 1 條加 `is_language_tag`（BCP-47 形狀） |
| 3 | rejection log 移出 `agent::probe` target，repro harness 的 `RUST_LOG='agent::probe=debug'` 會濾掉 | 補回 `target: "agent::probe"`，等級維持 `info` |
| minor | anchor 訊息說「most recent」但程式只要求「某個完整月」 | 改為強制 anchor＝最近一個完整月，多一種 rejection |
| minor | 兩處註解誇大兩層驗證對等 | 改寫為「模板只擋會拋錯的四條」 |
| minor | 月份格式測試只變異 index 0 | `report.rs`、`tools.rs` 各加 index 1 case |
| minor | anchor 為第一個月時 kWh 顯示「較上月 0.0%」（既有） | `previousCompleted` 為 null 時顯示「無前期資料可比較」 |
| minor | 「Call exactly once」與「call again」矛盾 | tool description 與 prompt 改為「一次成功呼叫即完成；被 REJECTED 就修正欄位再呼叫」 |

### 迴圈 3：資料不足的回傳流程（Option A）

- **RED**：`src/server/handler.rs` 新增 `report_without_emitted_data_fails_as_report_data_unavailable`
  （scripted LLM 以 `refuse_report()` 讓 composer 永不呼叫 `emit_report`），與
  `classify_pipeline_failure_gives_missing_report_data_its_own_code_only_for_the_report_pipeline`。
  未實作前前者會拿到 502、`code` 為 `upstream.error`、`message` 為原始 `missing artifact: report.data`。
- **GREEN**：`codes.rs` 新增 `REPORT_DATA_UNAVAILABLE`；handler 三處回傳路徑經 `classify_pipeline_failure`。
  `cargo test --lib` → 315 passed。
- Falcon：`npx tsc --noEmit` exit 0；`vitest run src/components/chief-of-staff src/lib/chief-of-staff` → 17 files / 95 tests passed。

### Chrome 複驗（第二輪模板）

| 夾具 | Uncaught | KPI | 圖 | 備註 |
|---|---|---|---|---|
| good | 0 | 5 | 4 | kWh「較上月 +11.7%」 |
| anchor 為第一個月 | 0 | 5 | 4 | kWh「無前期資料可比較」 |
| lcp_mismatch | 0 | 0 | 0 | 橫幅：`summary.latestCompletedPeriod「2026-6」不在 periods 之中` |
| bad_locale `zh_TW` | 0 | 0 | 0 | 橫幅：`Invalid language tag: zh_TW`（sink 端現在已擋下，此為最後防線） |

## 第三輪：第二位 fresh fable reviewer（PASS-WITH-FIXES）後的修正

| # | 發現 | 處置 |
|---|---|---|
| 1 IMPORTANT | `run_llm_loop` 的必要輸出催促文「You MUST call the required tool now with fully-populated arguments」會把「資料真的沒有」的 composer 推向捏造 | 催促文加逃生口：資料存在才呼叫；真的缺就**不得捏造**，重述缺哪些資料 |
| 2 IMPORTANT | 只有 OpenAI buffered 路徑有 e2e；Falcon 實際吃的 `/agent/stream` error frame 分支零測試，且沒驗證催三次 | 新增 `report_stream_without_emitted_data_emits_report_data_unavailable_error_frame`：斷言 frame `code`/`data`、無 `done`、scripted LLM 收到 `MAX_OUTPUT_RETRIES` 次催促 |
| 3 MINOR | `/agent/stream` 的 audit `error_code` 改記 code，與 OpenAI 兩路（記原始訊息）不一致，失去 stage 細節 | 還原：三路 audit 一律記原始 stage 訊息；只有 wire frame 用分類後的 code＋文案 |
| 4 MINOR | 文案斷言「資料不足」，但 `MissingArtifact` 也可能來自 model 連續格式錯到步數上限 | 文案改為「未能取得可渲染的報表資料，可能是…」；兩份 endpoint 文件同步 |
| 5 MINOR | prompt 第 1、5 行仍有「exactly once」「single call」 | 移除 |
| 6 MINOR | falcon 兩半映射無 vitest | 補 hook 測試（`code: report.data_unavailable` → `reportDataUnavailable` 文案）與 BFF route 測試（`code` 欄位原樣穿透） |
| 7/9 | 本文件與 `agent.md` 計數／措辭過時 | 已更正（七條不變量、五條測試、「最近一個」完整月、修正「未實作前」描述） |
| 8 NIT | `is_language_tag` 註解誇稱等同 `Intl.NumberFormat` 接受集 | 改為「superset」 |
| 10 NIT | `classify_pipeline_failure` 每個 Error event 算兩次 | 隨 #3 還原後只剩一次 |

## Step 3：完整驗證（第三輪後）

```
cargo fmt                                   → ok
cargo clippy --all-targets -- -D warnings   → Finished，0 warning
cargo test                                  → lib 316 passed / 0 failed；integration 全部 ok（含 7 個 #[ignore]，需 API key）
bash scripts/check-doc-links.sh             → doc link gate: PASS（86 檔）
bash .agent/skills/_shared/scripts/work-items.sh check → PASS（7 valid）
falcon: npx tsc --noEmit → 0；vitest chief-of-staff + BFF route → 18 files / 106 tests passed
```

`has_ui: false` → design-token gate N/A。`typed_contracts` 未宣告 → typecheck 以 clippy（含 check）代表。

## 未涵蓋

- `tests/repro_report_data.rs`（真實 OpenRouter model，`#[ignore]`）未重跑；要確認小模型面對新 rejection reason 能在 8 步內修正，需帶 `OPENROUTER_API_KEY` 跑一次。
- 資料不足時 composer 仍會被 `with_required_output` 催三次才失敗（多花三次 minimal-reasoning 呼叫）；在 fetcher 邊界直接偵測空資料、不進 analyst/composer，列為後續。
- 模板防線的驗證是 headless Chrome 一次性腳本，未進 repo（專案 guardrails：核心不引入 JS/Python 測試）。

## Execution Checklist

```
Skill: implement
Executed At: 2026-09-08 15:00
Files: src/agent/report.rs, src/agent/tools.rs, src/agent/payload.rs, src/test_support.rs,
       src/server/codes.rs, src/server/handler.rs,
       config/report_template/report.html, config/prompt_guide/report_composer_system.md,
       docs/reference/modules/agent.md, docs/reference/endpoints/agent-stream.md,
       docs/reference/endpoints/chat-completions.md, CHANGELOG.md, docs/work/index.md,
       falcon-client: src/components/chief-of-staff/useChiefOfStaffStream.ts,
                      src/lib/chief-of-staff/agent-runtime/streaming-events.ts, src/lib/chief-of-staff/copy.ts
       docs/work/report-data-semantic-validation/{meta.yml,implement-report.md}
Report: docs/work/report-data-semantic-validation/implement-report.md

Steps:
Step 1: 環境與邊界 - PASS
  Evidence: mode bug-fix；plan optional-missing；manifest 已讀（test_cmd cargo test / lint clippy -D warnings）
Step 2: RED-GREEN-REFACTOR - PASS
  Evidence: RED `expected Rejected, got Produced` 1 failed → GREEN 2 passed；模板 RED TypeError/0 圖 → GREEN 橫幅/0 Uncaught；
            迴圈 3 RED（code 缺、raw missing artifact）→ GREEN 315 passed
Step 3: 專案模式與完整驗證 - PASS
  Evidence: fmt ok；clippy 0 warning；cargo test 316 lib + integration pass；doc links PASS；falcon tsc 0 / vitest 106 pass

Gate:
  type-check: PASS（clippy --all-targets）
  lint: PASS
  test: PASS (316/316 lib + integration suites; falcon 106/106)
  UI mockup: N/A
```
