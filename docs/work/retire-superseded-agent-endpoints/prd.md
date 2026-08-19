# PRD — 退役被 `/agent/stream` 取代的四條端點

**Slug**: `retire-superseded-agent-endpoints`
**版本**: v1.0.0
**狀態**: approved（v1.0.0 由使用者於 2026-08-17 核准並授權進入 implement）
**Execution Mode**: `refactor`（PRD required · Spec optional → 不產出 · qa-plan absent）

## 0. Context

### Goal

`POST /insight`、`POST /insight/stream`、`POST /report`、`POST /report/stream` 是 2026-07-13
統一端點整合（`5ab1b7f`）之後的殘留。`/agent/stream` 以同一組 SSE frame 契約、加上 intent
自動路由，在功能上嚴格涵蓋兩條串流版；兩條非串流版則自 `/agent` 被移除後失去唯一的
消費者。這四條同時是全服務唯一**不經過 runtime prelude** 的 prompt 入口——沒有 guardrail、
沒有 injection 偵測、沒有 audit 軌跡。

刪除它們，使 guardrail 與 audit 的覆蓋率由「9 條中 2 條」變成「所有吃 user prompt 的端點」，
且不新增任何程式碼。受益者是服務的可稽核性與 codebase 的可理解性。

### Evidence

- `src/server/route.rs:100-107` — 四條 route 註冊點
- `src/server/handler.rs:120-395` — 四個 handler，內無任何 guardrails / audit 引用
- `src/server/handler.rs:497,946` — `AuditWriter` 僅在 `agent_stream` 與 `chat_completions` 建立
- commit `5ab1b7f`（2026-07-13）— 統一端點整合的來源
- commit `ea2bcef`（2026-07-11）— 移除 `POST /agent`
- falcon-client `src/lib/chief-of-staff/agent-client.ts:73` — 仍打已刪的 `/agent`
- falcon-client `src/components/chief-of-staff/useChiefOfStaffStream.ts:18` — `NEXT_PUBLIC_COS_STREAMING` 預設 `true`
- `config/runtime/intents.toml`、`thresholds.toml`、`config/config.toml:126-128`、`injection.toml`
  — 否決「補 guardrail」方案的依據（見 brainstorm.md B1）
- 詳細探索與否決理由：[`brainstorm.md`](./brainstorm.md)

### Risk

| Risk | Trigger | Mitigation |
|---|---|---|
| 存在本次查證未涵蓋的外部呼叫端 | repo 外有客戶端直接打那四條 | 發布前公告；`/v1/chat/completions` 為非串流替代、`/agent/stream` 為串流替代，兩者皆已上線可直接遷移 |
| falcon 在 Rust 端刪除後才更新，期間切換 flag | 有人設 `NEXT_PUBLIC_COS_STREAMING=false` | 該 flag 早已失效（404，已五週），刪除不使情況更差；falcon 端一併移除 flag |
| 誤刪仍被 `/agent/stream` 或 `/v1` 使用的 helper | 刪除範圍判斷失誤 | FR-002 已逐符號列出保留/刪除清單並經 grep 查證；`cargo test` + `clippy -D warnings` 為 machine gate |

## 1. Scope

### Confirmed direction

不保留非串流路徑。四條端點全刪，falcon 端同步退役 REST 代理。已確定的 follow-up 見文末附件。

### In scope

- 移除 `route.rs` 的四條 route 註冊
- 移除 `handler.rs` 的 `insight` / `insight_stream` / `report` / `report_stream`
- 移除因此變成死碼的 helper 與其測試（清單見 FR-002）
- 更新 `docs/reference/`：刪除四份端點頁、修正 endpoints/modules 索引與相關敘述
- falcon-client：移除 REST 代理路徑與失效的 feature flag，並更新其文件（FR-004）

### Out of scope

- plan §9（把 sub-agent pipeline 收到 runtime `AgentPort` 後面）
- `run_agent_turn` 的復活；`llm_connector` 的 dormancy
- `RUNTIME_ENABLED` flag 的存廢（其 rollback 敘述會被修正，但 flag 本身保留）
- `/agent/stream`、`/v1/chat/completions`、`/greeting`、`/health`、`/ready` 的任何行為變更
- 擴充 `intents.toml` 關鍵字、調整 `answer_gray`、啟用 LLM normalizer
- `sub-agent` pipeline 本身（`build_insight_pipeline` / `build_report_pipeline` 全數保留）

### Dependencies & Constraints

- **Upstream**: falcon-client（`/Users/liying.chu/falcon-client`，分支 `chat-bot`）需同步變更；
  兩邊的變更順序不互相阻擋，因為被移除的路徑目前已是 404。
- **Breaking change**: **Yes** — 移除 4 個公開 HTTP 端點。已知消費者不受影響
  （falcon 走 `/agent/stream`）；未知消費者的遷移路徑為 `/agent/stream`（串流）或
  `/v1/chat/completions`（非串流）。
- **Assumptions**: 除 falcon 之外無其他呼叫端。此前提未經 repo 外驗證，列為 Risk 第一項。

## 2. Product Summary

### Business Goal

吃 user prompt 的 HTTP 端點，100% 經過 guardrail 與 audit（目前 2/4）。淨刪除行數 > 0。

### Personas + Pain Points

| Persona | Context | Pain Point |
|---|---|---|
| 維運/稽核 | 需回答「某次查詢問了什麼、被如何處理」 | 四條端點無 audit 事件，該問題無法回答 |
| 後端開發者 | 讀 codebase 判斷請求路徑 | 兩套平行編排、9 條端點中 4 條是殘留，理解成本高且易改錯 |
| falcon 前端開發者 | 需知道 upstream 契約 | 文件描述的 REST 退路實際 404，誤導性強 |

## 3. Functional Requirements (FR)

### FR-001: 移除四條 route 註冊

**使用者價值**: 端點清單反映實際受支援的契約，不再有無防護入口。
**Behavior**: `build_router` 的 standard sub-router 由 8 條降為 4 條
（`/health`、`/ready`、`/greeting`、`/agent/stream`）；OpenAI sub-router 不變。
**Data source**: Existing（`src/server/route.rs`）
**Permissions / Visibility**: 不變（standard 群維持 `require_bearer` → 418）
**Boundary conditions**:

- 刪除後對四條舊路徑的請求回 `404`（axum 預設 fallback），且**不經過** auth layer——
  因此不會洩漏「token 是否正確」的資訊。
- `route.rs:97` 的註解「The seven original endpoints」本已過期（實際 8 條），須一併修正為實際數字。

### FR-002: 移除連帶死碼

**使用者價值**: 不留下指向已刪端點的誤導性符號。
**Behavior**: 移除下列符號及其測試。清單經逐符號 grep 查證。
**Data source**: Existing（`src/server/handler.rs`）

**刪除**（僅被那四個 handler 使用）:

| 符號 | 位置 | 唯一使用點 |
|---|---|---|
| `insight` / `report` / `report_stream` / `insight_stream` | `:128` / `:188` / `:242` / `:322` | route 註冊 |
| `insight_initial` | `:1396` | `:162, 220, 277, 359` |
| `final_answer` | `:1416` | `:168, 226` |
| `insight_error_to_app_error` | `:1430` | `:164, 222` |
| `validate_prompt`（handler 版） | `:1367` | `:142, 200, 255, 337` |
| `USER_PROMPT_LENGTH_CAP` | `:54` | `:1373, 1375` |
| 上述兩者的測試 | `:1513-1536` | — |

**保留**（`/agent/stream` 或 `/v1/chat/completions` 仍在用）:

`insight_frames`（`:633` agent_stream）、`INSIGHT_STREAM_BUFFER`（`:543, 1103, 1231`）、
`build_insight_pipeline` / `build_report_pipeline`（`:563/548` agent_stream、`:820/806`
chat_completions）、`agent_pipeline_id` / `report_pipeline_id`、`wants_report_pipeline`、
`status_to_app_error`、`UnusedAgentPort`。

**Boundary conditions**:

- `runtime::guardrails::input_guard::validate_prompt` 是**不同函式**（帶 `max_prompt_chars`
  參數，由 prelude 使用），必須保留。勿與 handler 版混淆。
- `tests/agent_pipeline.rs:264` 的 `final_answer` 是同名區域變數，非本 helper。
- `insight_frames` 與 `INSIGHT_STREAM_BUFFER` 保留後名稱仍含 `insight`，雖指向已刪端點，
  但改名屬純美化，列為 FU-002。

### FR-003: 更新 `docs/reference/`

**使用者價值**: reference 是宣告的 canonical source of truth，不得描述不存在的端點。
**Behavior**:

- 刪除 `endpoints/insight.md`、`insight-stream.md`、`report.md`、`report-stream.md`
- `endpoints/index.md`：路由表 9→5、移除「三種 agent 執行路徑」中的直接 pipeline 一列、
  移除 2000/4000 雙 cap 敘述（只剩 4000）
- `endpoints/agent-stream.md`：移除「rollback 時改用 `/insight/stream` 或 `/report/stream`」
- `modules/index.md`、`modules/server.md`、`modules/agent.md`：handler 由 9 個改為 5 個、
  依賴圖移除「直接驅動、繞過 runtime」那一路
- `reference/index.md`：更新端點數與同步狀態註記

**Data source**: Existing（commit `2d0a64b` 剛建立這些頁）
**Boundary conditions**: `prd.md` / `spec/spec.md` / `tests/qa-plan.md` 仍為 v1.3.0 視角，
本次不同步（既有已知缺口，已於 `reference/index.md` 標註）。

### FR-004: falcon-client 同步退役 REST 路徑

**使用者價值**: 消除一條 404 的程式路徑與描述它的文件。
**Behavior**:

- 移除 `agent-client.ts` 的 `callAgent` 與 `agentUrl` 的 `'/agent'` 型別分支
- 移除 `useChiefOfStaffStream.ts` 的 `submitRestRequest` 與 `useStreaming` flag
  （`submitRequest` 直接等於 `submitStreamRequest`）
- 移除 `app/api/chief-of-staff/nodes/route.ts` 的 `POST`（**保留 `GET`**——它是 greeting，
  打 `/greeting`，仍然有效）
- 文件更新清單：
  - `docs/prd/features/chief-of-staff.md` — FR-004（§4）、§5.5、§5.6 對照表、`:724`、
    v2.1.0/v2.2.0 changelog 中「非串流 `/agent` REST fallback 不受影響」的敘述
  - `docs/guides/chief-of-staff-input-pipeline.md:169`
  - `docs/prd/features/chief-of-staff-agent-runtime-plan.md:67-68, 132`
  - `docs/plans/chief-of-staff-agent-runtime/consistency-audit.md:63`（FR-007 的 ✅ 已失效）
  - `docs/work/chief-of-staff-sse-align/` — 整份標 `superseded`（目標端點為 `/insight/stream`）

**Data source**: Existing（falcon-client repo）
**Boundary conditions**:

- falcon 工作樹在 `chat-bot` 分支上已有大量**無關**的未提交變更（`.claude/skills/*`、
  `.agent/*` submodule 同步）。只 stage 本次明確檔案，不得混入。
- falcon 的 PRD 版本需 bump 並記錄 changelog（該 repo 慣例）。

## 4. Non-functional Requirements (NFR)

| Category | Requirement |
|---|---|
| Performance | N/A — 純刪除，不改變保留路徑的執行成本 |
| Security / Compliance | 提升：移除唯一的無 guardrail / 無 audit prompt 入口。刪除後舊路徑回 404 且在 auth layer 之前，不洩漏 token 有效性 |
| Accessibility | N/A（`has_ui=false`） |
| Compatibility | **破壞性**：移除 4 個公開端點。遷移路徑：串流 → `/agent/stream`；非串流 → `/v1/chat/completions` |

## 5. Error Scenarios (ERR)

### ERR-001: 舊路徑仍被呼叫

**Trigger**: 未知客戶端在刪除後打 `/insight`、`/report` 及其 stream 版。
**Expected behavior**: `404 Not Found`（axum fallback），不進 auth layer、不進 pipeline。
**Recovery**: 客戶端改打 `/agent/stream`（串流、含 intent 路由）或 `/v1/chat/completions`
（非串流、OpenAI 形狀）。

### ERR-002: falcon 切換已失效的 flag

**Trigger**: 設 `NEXT_PUBLIC_COS_STREAMING=false`。
**Expected behavior**: FR-004 完成後該 flag 不存在，前端一律走串流；無退路可切。
**Recovery**: 串流本身故障時的處置屬維運議題，不再有端點層 fallback。此為明示接受的取捨
（該 fallback 自 2026-07-11 起已實際失效）。

## 6. Acceptance Criteria (AC)

### AC-001: 四條端點不再存在

```gherkin
Given 服務以預設設定啟動
When 對 /insight、/insight/stream、/report、/report/stream 發出 POST
Then 全部回 404
And 回應不因 Authorization header 正確與否而不同
```

### AC-002: 保留端點行為不變

```gherkin
Given 服務以預設設定啟動
When 對 /health、/ready、/greeting、/agent/stream、/v1/chat/completions 發出既有的合法請求
Then 行為與刪除前完全一致（status、body、SSE frame 序列）
And /agent/stream 仍能依 intent 路由到 insight 或 report pipeline
```

### AC-003: 沒有殘留死碼

```gherkin
Given 已依 FR-002 完成刪除
When 執行 cargo clippy -- -D warnings
Then 通過，且無 dead_code 警告
And cargo test 全數通過
```

### AC-004: guardrail 覆蓋率完整

```gherkin
Given 刪除後的 route 表
When 列出所有接受 user prompt 的端點
Then 只有 /agent/stream 與 /v1/chat/completions
And 兩者都呼叫 plan_stream_turn
```

### AC-005: reference 文件無指向已刪端點的連結

```gherkin
Given 已依 FR-003 更新
When 檢查 docs/reference/ 內所有相對連結
Then 全部可解析
And 沒有任何檔案描述 /insight 或 /report 為可用端點
```

## 7. Flow

Flow: N/A，因為本變更為移除，不引入新的使用者流程。刪除後的請求路徑見
[`docs/reference/endpoints/index.md`](../../reference/endpoints/index.md)。

## 8. UI/UX

UI: N/A（`has_ui=false`）。falcon 端 FR-004 移除的是非預設的 code path，
使用者可見行為不變（預設本來就走串流）。

## 9. Related Documents

| Document | Link |
|---|---|
| Brainstorm | [`brainstorm.md`](./brainstorm.md) |
| Spec | N/A（refactor mode，Spec optional；刪除範圍已於 FR-002 逐符號界定，無額外設計） |
| QA Plan | N/A（refactor mode，qa-plan absent；驗收由 AC + `cargo test`/`clippy` machine gate 承擔） |
| Reference（現況契約） | [`docs/reference/endpoints/index.md`](../../reference/endpoints/index.md) |

---

## Appendix: Resolved Follow-ups

### FU-001: 非串流 `AgentResponse` 形狀的端點

| 狀態 | 內容 |
|---|---|
| Non-blocking | 刪除後不再有回 `AgentResponse`（`{user_prompt, model_response, intent}`）的端點。 |
| 選項 | A: 不提供，需要非串流者用 `/v1/chat/completions`（OpenAI 形狀）／ B: 保留 `/insight` 並補 prelude |
| 決定 | **A**。使用者 2026-08-17 選定「1 不留」。理由：該形狀的唯一消費者（falcon REST 代理）一併退役，且 `/v1` 已提供有 guardrail 的非串流路徑。 |

### FU-002: `insight_frames` / `INSIGHT_STREAM_BUFFER` 更名

| 狀態 | 內容 |
|---|---|
| Non-blocking | 兩個保留符號的名稱仍含 `insight`，指向已刪端點，語意上誤導。 |
| 選項 | A: 本次不改，列為後續美化／ B: 一併更名為 `pipeline_frames` / `PIPELINE_STREAM_BUFFER` |
| 決定 | **A**。更名會擴大 diff 且與刪除的驗收混在一起；`/agent/stream` 仍走 insight pipeline，名稱並非全錯。Owner: Runtime。 |

### FU-003: `RUNTIME_ENABLED` rollback flag 的存廢

| 狀態 | 內容 |
|---|---|
| Non-blocking | 刪除後 `RUNTIME_ENABLED=false` 只剩 `/health`、`/ready`、`/greeting` 可用，該 flag 實質失去 rollback 意義。 |
| 選項 | A: 本次只修正文件敘述，保留 flag／ B: 一併移除 flag 與 legacy 組裝路徑 |
| 決定 | **A**。移除 flag 會動到 `AppState` 組裝與啟動路徑，超出本次範圍（已列 Out of scope）。Owner: Runtime；與 plan §9 同批處理。 |

## Delivery

| 項目 | 內容 |
|---|---|
| Candidate | PR [#11](https://github.com/h-alice/datacenter-agent/pull/11)，branch `codex/retire-superseded-agent-endpoints` |
| Release | 2026-08-19 merge 進 `main`，merge commit `7aa2af1d364fd0cf45f1c04cc5d67f1087551cd5`（by h-alice） |
| PRD 偏離 | **無**。FR-001～FR-004 全數依 PRD 執行；FU-002 / FU-003 依附件的決定刻意不做，屬 PRD 已載明的 Out of scope。 |
| 提升為長期資產 | `docs/reference/endpoints/`、`docs/reference/spec/spec.md`、`docs/reference/prd.md` 已同步退役後的現況（`spec.md` 與 `prd.md` 的 v1.3.0 殘留以刪除線標記）。 |
| 最終證據 | [`implement-report.md`](./implement-report.md) — `cargo fmt` clean · `clippy -D warnings` 通過 · `cargo test` 215 passed / 0 failed · CI `rust` job pass |
| 遺留 gate | **AC-001 / AC-002 沒有 route-level 404 斷言**。`build_router` 需要完整 `AppState`（含只能經 live connect 取得的 `McpHandle`），可行解法是 rmcp in-memory transport + `tokio::io::duplex`（約 50 行），本次未做。這是缺一個測試，不是交付未完成。 |
| 下游 | falcon-client `8a20c46` 已 push 為 `origin/refactor/retire-nonstreaming-rest`，PR 待開。與本次無上線順序依賴——falcon 打的 `POST /agent` 自 `ea2bcef` 起已 404 五週。 |
