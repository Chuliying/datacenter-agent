# Implement Report — 退役被 `/agent/stream` 取代的四條端點

**日期**: 2026-08-17
**Branch**: `codex/retire-superseded-agent-endpoints`（base = `codex/reference-docs-0.3.0-sync`）
**PRD**: [`prd.md`](./prd.md) v1.0.0（approved）

## 交付摘要

淨刪除 **591 行**（170 insertions / 761 deletions，16 檔）。無新增功能程式碼。

| Gate | 結果 |
|---|---|
| `cargo fmt --check` | clean |
| `cargo clippy --all-targets -- -D warnings` | 通過（無 dead_code 警告） |
| `cargo test` | **215 passed / 0 failed / 6 ignored** |

6 個 ignored 全部是既有的 live-network 測試（`--ignored` 才跑，需真 MCP + OpenRouter），
非本次造成。

## FR-001 — 移除四條 route 註冊

`src/server/route.rs`：standard sub-router 由 8 條降為 4 條。最終 route 表：

```
standard (120s, require_bearer → 418): /health  /ready  /greeting  /agent/stream
openai   (600s, require_bearer_openai → 401): /v1/chat/completions
```

同時修正三處已過期的註解：

- `route.rs` 的「The seven original endpoints」→ 實際數字（該註解在本次之前就已錯，當時是 8 條）
- `route.rs:68` 的「per-field 2 000-char prompt cap」→ 改指 `thresholds.input.max_prompt_chars`
- `auth.rs:68` 與 `route.rs:123` 的「the other seven endpoints」→「the other standard endpoints」

## FR-002 — 移除連帶死碼

依 PRD 清單刪除，並以編譯器逐步確認：

| 符號 | 結果 |
|---|---|
| `insight` / `report` / `report_stream` / `insight_stream` | 刪（連續區塊，含各自的區段註解） |
| `insight_initial` | 刪 |
| `final_answer` | 刪 |
| `insight_error_to_app_error` | 刪 |
| `validate_prompt`（handler 版） | 刪 |
| `USER_PROMPT_LENGTH_CAP` | 刪 |
| `prompt_validation_rejects_empty_prompt`、`prompt_validation_preserves_existing_2000_char_cap` | 刪（測試數 207 → 205） |

保留且確認仍被使用：`insight_frames`、`INSIGHT_STREAM_BUFFER`、`build_insight_pipeline`、
`build_report_pipeline`、`agent_pipeline_id`、`report_pipeline_id`、`wants_report_pipeline`、
`status_to_app_error`、`UnusedAgentPort`。

`runtime::guardrails::input_guard::validate_prompt`（帶 `max_prompt_chars` 參數、由 prelude 使用）
未受影響——與被刪的 handler 同名函式是不同的東西。

### PRD 清單之外的發現：`AgentResponse` 也死了

刪除後 `cargo check` 報 `unused import: AgentResponse`。查證後確認 `AgentResponse`
（`dto.rs`）在全 crate 只剩定義本身，**沒有任何使用點**——它的唯一用途就是本次退役的非串流
回應形狀。因此一併刪除 struct 與 import。

PRD FR-002 的清單沒有列到這個 DTO，屬實作階段發現的連帶死碼。`AgentRequest` 保留
（`agent_stream` 與 `openai::map_request` 都在用）。

順帶修正 `dto.rs` 的兩處註解：`StreamFrame` 的「`POST /insight/stream`（and legacy
`/report/stream`）SSE wire」→ `/agent/stream`。

## FR-003 — 更新 `docs/reference/`

刪除 4 份端點頁：`endpoints/{insight,insight-stream,report,report-stream}.md`。

更新 7 份：

| 檔案 | 變更 |
|---|---|
| `endpoints/index.md` | 路由表 9→5、加「已退役端點 + 遷移路徑」區塊、「三種執行路徑」→「兩條」、prompt cap 收斂為單一 4000 |
| `endpoints/agent-stream.md` | 移除 rollback 建議；**SSE frame 完整表格搬入本頁**（原本連到已刪的 insight-stream.md）；補終局協定與未外送事件 |
| `modules/index.md` | 依賴圖改為單一 request 路徑；重點 1 改為「不再有繞過 runtime 的 prompt 入口」 |
| `modules/server.md` | handler 9→5、「三種執行路徑」→「兩條」、DTO 表移除 `AgentResponse`、錯誤映射改述、加 `insight_frames` 命名註記 |
| `modules/agent.md` | 開頭方框改為「本層一律在 prelude 之後」、pipeline 名稱與端點名稱脫鉤（`/insight` → insight pipeline）、已知邊界改述 |
| `modules/llm-connector.md` | 端點清單移除已退役者 |
| `index.md` | 端點數 9→5、新增「prompt 入口」列、同步狀態註記補第二次校正 |

**重要區分**：insight / report **pipeline 仍然存在**（`agent_pipeline_id`、`report_pipeline_id`
與四階段組成都保留），只有端點消失。文件已把「pipeline 名稱」與「端點路徑」的敘述分開，
避免讀者誤以為 pipeline 也被刪了。

連結檢查：`docs/reference/` 內所有 `.md` 與 `src/*.rs` 相對連結全數可解析。

## FR-004 — falcon-client

**本次未執行**，見下方待辦。

## AC 驗收

| AC | 狀態 | 證據 |
|---|---|---|
| AC-001 四條端點不再存在 | **部分** | route 註冊點已移除（`route.rs` 僅 5 條）、handler 已刪、編譯通過。**但無 route-level 404 斷言** |
| AC-002 保留端點行為不變 | **部分** | 未觸碰 `agent_stream` / `chat_completions` / `greeting` / `health` / `ready` 的任何程式碼；215 tests 全過（含 `wants_report_pipeline`、`insight_frames`、openai mapping 等既有斷言）。**無端到端驗證** |
| AC-003 沒有殘留死碼 | **PASS** | `clippy --all-targets -- -D warnings` 通過、`cargo test` 全過 |
| AC-004 guardrail 覆蓋率完整 | **PASS** | route 表僅 `/agent/stream` 與 `/v1/chat/completions` 吃 prompt，兩者都呼叫 `plan_stream_turn`（`handler.rs` 內僅此兩處建立 `AuditWriter`） |
| AC-005 reference 無指向已刪端點的連結 | **PASS** | 連結檢查通過；殘留提及全在「已退役」敘述脈絡內 |

### AC-001 / AC-002 為何只有部分證據

repo 目前**沒有** Router-level oneshot 測試套件——`build_router` 需要完整 `AppState`
（含 `McpHandle`），這是既有的 coverage gap，已記錄在
[`endpoints/index.md`](../../reference/endpoints/index.md) 的 Coverage gaps 一節，
非本次造成。因此「打舊路徑得到 404」與「保留端點行為完全不變」目前靠**移除註冊點 + 型別檢查
+ 未觸碰保留路徑**佐證，而不是可執行的斷言。

要補齊需啟動本機服務實測（`source ./env` + MCP on :8088，見 memory `local-integration-env`）。
本次未做，列為待辦。

## 待辦

1. **FR-004 falcon-client** — `callAgent` / `submitRestRequest` / `nodes/route.ts` 的 POST /
   `NEXT_PUBLIC_COS_STREAMING` flag，以及 7 處文件（清單見 PRD FR-004）。跨 repo，需另外確認。
2. **AC-001 / AC-002 端到端實測** — 起本機服務確認舊路徑 404、保留端點正常。
3. FU-002 `insight_frames` / `INSIGHT_STREAM_BUFFER` 更名（本次刻意不做）。
4. FU-003 `RUNTIME_ENABLED` flag 存廢（本次只修正文件敘述）。
5. `docs/reference/` 的 `prd.md` / `spec/spec.md` / `tests/qa-plan.md` 仍為 v1.3.0 視角。
