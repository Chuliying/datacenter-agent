# Implement Report — 退役被 `/agent/stream` 取代的四條端點

**日期**: 2026-08-17
**Branch**: `codex/retire-superseded-agent-endpoints`（base = `codex/reference-docs-0.3.0-sync`）
**PRD**: [`prd.md`](./prd.md) v1.0.0（approved）

## 交付摘要

淨刪除 **591 行**（170 insertions / 761 deletions，16 檔）。無新增功能程式碼。

| Gate | 結果 |
|---|---|
| `cargo fmt --check` | clean |
| `cargo clippy --all-targets -- -D warnings` | 通過 |
| `cargo test` | **215 passed / 0 failed / 6 ignored** |
| `cargo doc --no-deps` | **10 warnings**（parent 為 11）；本次造成的 2 個已修掉 |
| `bash -n scripts/staging-smoke.sh` | 語法 OK |

### rustdoc 警告的三個時點（`cargo clean --doc` 後實測）

| 時點 | 警告數 | 差異 |
|---|---|---|
| parent `2613ef2^` | 11 | 基線 |
| `2613ef2`（初次提交） | 13 | **+2**：刪掉 `insight_stream` / `report_stream` 後，`agent_stream` doc 裡指向它們的 intra-doc link 斷裂 |
| review 修正後 | **10** | −3：修掉上述 2 個，另順手拆掉一個既有的 `agent_stream` → private `insight_frames` 連結 |

剩下的 10 個全部 pre-existing：`agent/pipeline.rs` 7（`ConfiguredAgent` ×6、`SchemaTool` ×1）、
`agent/wiring.rs` 1（`ReportData`）、`server/openai.rs` 1（`map_request` → private
`is_ignored_role`）、`server/handler.rs` 1（`agent_stream` → private `wants_report_pipeline`）。

> **Gate 教訓（兩個）**
>
> 1. 初次提交只跑 fmt / clippy / test，漏了 `cargo doc`——而 fmt / clippy / test
>    **結構上抓不到 rustdoc lint**。刪除公開項目時必須跑 `cargo doc`。
> 2. `clippy` 對 `pub` 項目不會發 `dead_code`，所以它不能證明「沒有 pub 死碼」——
>    AC-003 的證據強度僅限於私有項目。`AppState::generation_config` 就是漏網的例子。

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

**已完成**：commit `8a20c46`（falcon-client，branch `chat-bot`）。淨 -238 行（90 insertions /
328 deletions，12 檔）。

分支選擇：`chat-bot`（非 manifest 的 base `dev`）——統一串流那個 commit `6fb42cc` 只在
`chat-bot` 上，`dev` 沒有，所以必須疊在 `chat-bot`。

| 項目 | 變更 |
|---|---|
| `agent-client.ts` | 移除 `callAgent`、`AgentResponse` 型別；`agentUrl` 型別收斂為 `AgentStreamPath \| '/greeting'`。`agentRequestBody` / `usesAgentServerMemory` / `agentErrorInfo` 保留給串流 |
| `nodes/route.ts` | 移除 `POST`，**保留 `GET`**（greeting，打 upstream `/greeting` 仍有效）；連帶移除只有 POST 用到的 8 個 import 與 helper |
| `useChiefOfStaffStream.ts` | 移除 `useStreaming` flag、`submitRestRequest`、`CreateNodeResponse`；`submitRequest` 直接等於 `submitStreamRequest` |
| `ChiefOfStaffPanel.tsx` | 停止按鈕條件 `isLoading && useStreaming` → `isLoading` |
| `nodes/route.test.ts` | **刪除**（4 個測試全部測已移除的 POST；`GET` 本來就無覆蓋，故非覆蓋倒退） |
| `agent-client.test.ts` | 測 `callAgent` 的那條**改寫**為 `openAgentStream`，保留原斷言意圖（base url 正規化 + session/option 轉發），並補驗 server-memory 關閉時 history 保留 |
| 文件 6 份 | PRD v2.2.0→v2.3.0（含 changelog 列、§5.5 與 FR-004 標 Superseded、§5.6 對照表移除「後備」欄、feature flag 段改寫、§7 依賴清單）、input-pipeline guide、runtime-plan、consistency-audit（FR-007 的 ✅ → 🔴）、work item `chief-of-staff-sse-align` 標 superseded |

Gate：type-check pass · `eslint --max-warnings 0` pass · chief-of-staff **15 檔 58 測試通過**
（基線 16/62，差額正是刪掉的 4 個 POST 測試）· 專案 pre-commit hooks（type-check /
lint-staged / console.log / secrets / 文件結構）全過。

falcon 工作樹另有 155 個**無關**的未提交變更（skills submodule 同步、zip、flow-map 等），
只 stage 了本次的 12 個檔案，未混入。

**交付狀態（2026-08-19）**：`8a20c46` 已 push 為 `origin/refactor/retire-nonstreaming-rest`
（`8a20c460071a3c3e054d2420c12695d2a58e3839`）；`dev` 與 `chat-bot` 未動。falcon 端 PR 尚未
建立——`gh` 無法解析 `HDRenewables/falcon-client`（研判 SAML SSO 未授權）。

合併模擬：`git merge-tree --write-tree origin/dev 8a20c46` exit 0、零衝突；合併後的樹中
`submitRestRequest` 與 `AgentResponse` 殘留 0 處，`callAgent` 僅剩 `agent-client.test.ts`
的一處說明註解。該 commit 疊在落後 `origin/dev` 177 個 commit 的分支上，但未造成衝突面。

**這不是上線順序風險。** 先前記錄曾稱「upstream 端點消失會讓 falcon 壞掉」，該判斷有誤：
falcon 的非串流路徑打的是 `POST /agent`（`agent-client.ts:73`），而該端點早於本次變更即由
`ea2bcef` 移除，所以它今天就已經是死路；且 `useChiefOfStaffStream.ts:18` 的
`useStreaming = process.env.NEXT_PUBLIC_COS_STREAMING !== 'false'` 預設走串流，該路徑預設
不可達。falcon 實際使用的 `/agent/stream` 與 `/greeting` 都在本次退役後保留
（`src/server/route.rs:107-125`，退役後共 5 條）。FR-004 因此是清理指向已消失端點的
client 端死碼，沒有跨 repo 的發布順序依賴。

### 全 repo 測試的 4 個既有失敗

`src/__tests__/features/menu-access-control.test.ts` 有 4 個失敗，**與本變更無關**：
那些測試只碰 `menuDevStatus` / `menuBetaStatus` / `menuPermissions` / `getMenuPermissions`，
皆未被本次改動；且在同一 commit 的乾淨 worktree checkout 上 45/45 全過，
故屬工作樹既有未提交 WIP 造成。未進一步追查（超出範圍）。

## AC 驗收

| AC | 狀態 | 證據 |
|---|---|---|
| AC-001 四條端點不再存在 | **部分** | route 註冊點已移除（`route.rs` 僅 5 條）、handler 已刪、編譯通過。2026-08-19 補 [`tests/route_contract.rs`](../../../tests/route_contract.rs)：四條退役路徑不得重新註冊、存活路徑恰為五條。**仍無 HTTP 層 404 斷言** |
| AC-002 保留端點行為不變 | **部分** | 未觸碰 `agent_stream` / `chat_completions` / `greeting` / `health` / `ready` 的任何程式碼；215 tests 全過（含 `wants_report_pipeline`、`insight_frames`、openai mapping 等既有斷言）。**無端到端驗證** |
| AC-003 沒有殘留死碼 | **PASS** | `clippy --all-targets -- -D warnings` 通過、`cargo test` 全過 |
| AC-004 guardrail 覆蓋率完整 | **PASS** | route 表僅 `/agent/stream` 與 `/v1/chat/completions` 吃 prompt，兩者都呼叫 `plan_stream_turn`（`handler.rs` 內僅此兩處建立 `AuditWriter`） |
| AC-005 reference 無指向已刪端點的連結 | **PASS** | 連結檢查通過；殘留提及全在「已退役」敘述脈絡內 |

### AC-001 / AC-002 為何只有部分證據

**更正**：初版報告寫「repo 沒有 Router-level oneshot 測試套件」，這是錯的。
`src/server/route.rs` 已有兩個用 `tower::ServiceExt::oneshot` 的 Router 測試
（`per_group_timeout_layers_survive_a_merge`、`openai_timeout_returns_openai_error_envelope`），
只是它們建的是**合成 router**，不是 `build_router(state)`。

真正的阻礙只有一個：`AppState.mcp: McpHandle` 包住私有的 `Peer<RoleClient>`
（`src/mcp_client.rs`），全 repo 只能透過 live `McpClient::connect_http` 取得——四個使用者
都是 `--ignored` 的 live 測試。

可行路徑（review 指出，約 50 行、中等成本）：rmcp 0.17 支援 in-memory transport，
在 process 內起一個 trivial `ServerHandler` 跑在 `tokio::io::duplex` 上、handshake 後取
`client.handle()`，`AppState` 其餘欄位填 plain data（`runtime: None` 即可——404 fallback
在任何 handler 與 auth 之前就命中）。本次未做。

因此「打舊路徑得到 404」與「保留端點行為完全不變」目前靠**移除註冊點 + 型別檢查 + 未觸碰保留
路徑的程式碼**佐證，而不是可執行的斷言。另一條補齊路徑是啟動本機服務實測
（`source ./env` + MCP on :8088，見 memory `local-integration-env`）。

## Review 修正（Fable subagent，2026-08-17）

review 對 `2613ef2` 提出 7 項，全部驗證屬實並修正：

| # | 問題 | 處置 |
|---|---|---|
| 1 | 破壞性變更沒進 release artifact（`CHANGELOG.md` 的 `[Unreleased]` 是空的），而 repo 有 Keep-a-Changelog 慣例（見 `ba3fa5c`、`03136c9`） | 補 `[Unreleased]` 的 `### Removed` / `### Changed`。**版本未 bump**——本 repo 的慣例是獨立的 bump commit，留給發布時處理 |
| 2 | **我新增了兩個壞掉的 rustdoc 連結**：`handler.rs` 的 `agent_stream` doc 仍以現在式寫 `Unlike [insight_stream] / [report_stream]`，刪掉函式後連結斷裂 | 改寫成過去式、去掉連結括號。實測 13 → 10 warnings（見上方三時點對照） |
| 3 | src 註解仍斷言存在繞過 prelude 的入口——正是本 commit 聲稱消除的風險：`agent/mod.rs:61,63`、`wiring.rs:42`（我只改了 `:45` 的串流那條，漏了 buffered 那條）、`appstate.rs:190`（`RUNTIME_ENABLED=false` 回退到 legacy path） | 四處全部改寫 |
| 4 | `scripts/staging-smoke.sh` 已死且無人標記：curl `POST /agent`（404，`-fsS` 會直接中止）、斷言已刪的 `AgentResponse` 形狀（**這就是那個 serde 消費端**）、SSE 白名單只允許 5 種 event 而實際有 9 種 | 改寫：非串流改打 `/v1/chat/completions` 並驗 `chat.completion` envelope、event 白名單補齊 9 種、加 `done` 終局檢查 |
| 5 | canonical `docs/reference/tests/qa-plan.md` 仍引用本次刪掉的兩個測試（TC-U01 / TC-U02-L）與 2000 cap 當作 evidence | 相關列改為刪除線並註明原因 |
| 6 | `README.md` 端點清單過期（列 `/agent`、缺 `/v1/chat/completions`），且 Runtime 段仍說 `RUNTIME_ENABLED=false` 回退 legacy path | 更新端點清單 + 加退役說明 + 改寫 Runtime 段 |
| 7 | `AppState::generation_config`（`appstate.rs:281`）零呼叫端，doc 卻說「retained for the monolith loop's remaining callers」 | **未刪**（pre-existing 死碼，超出本次範圍）；列為待辦 |

review 同時確認為正確的部分：刪除的每個符號都真的不可達（含 `tests/`、`src/bin/eval.rs`）、
保留的每個符號都真的還在用（含 `InsightGrants.charter`、`report_template`、`PromptBank.agent_system`
由 eval runner 使用）、`/agent/stream` 除 import 與 503 字串外無任何非註解變更、report pipeline
從兩個保留端點都仍可達、`agent-stream.md` 的 9 種 frame 表與 `dto.rs` 逐項相符、
404-before-auth 成立、`plan_stream_turn` 恰好兩個呼叫點。

## 待辦

1. **AC-001 部分完成** — `tests/route_contract.rs` 已釘住 route 表（四條退役路徑不得重新註冊、
   存活路徑恰為五條）。仍缺 HTTP 層的 404 斷言與 AC-002 的 SSE 序列比對，兩者都需要 live 服務
   或為 rmcp `server` feature 加 dev-dependency，見 prd.md `## Delivery` 的遺留 gate 欄。
2. **`AppState::generation_config` 是零呼叫端的 pub 死碼**（`appstate.rs:281`，pre-existing）。
   `clippy` 不會對 pub 項目發 `dead_code`，所以沒被 gate 抓到。
3. ~~版本 bump 到 `0.4.0`~~ **已完成**：Cargo.toml / Cargo.lock、CHANGELOG 切出 `[0.4.0] - 2026-08-19`、`docs/reference/index.md` 的 crate 版本列同步。
4. FU-002 `insight_frames` / `INSIGHT_STREAM_BUFFER` 更名（本次刻意不做）。
5. FU-003 `RUNTIME_ENABLED` flag 存廢（本次只修正文件敘述）。
6. `docs/reference/` 的 `prd.md` 與 `spec/spec.md` 仍為 v1.3.0 視角（`spec.md` 還有 `POST /agent` 路由表與 2000 cap 列）。`qa-plan.md` 本次已修正被刪測試的引用。
