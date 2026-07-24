---
output: docs/work/agentgateway-openai-endpoint/qa-report.md
stage: qa-report
slug: agentgateway-openai-endpoint
---

# OpenAI 相容 `/v1/chat/completions` 端點 驗收報告

**Spec**: `docs/work/agentgateway-openai-endpoint/spec.md` · **PRD**: `prd.md` · **QA plan**: `qa-plan.md`
**Capabilities**: `has_ui=false` · `has_api=true` · `typed_contracts=true` · `has_e2e=false`
**驗收日期**: 2026-07-22

## 1. Pre-flight Checklist
- [x] Type/compile check：`cargo check` → **0 errors**(獨立重跑)
- [x] Lint：`cargo clippy --all-targets -- -D warnings` → **clean**
- [x] Coding rules：新端點沿用既有 handler/route 慣例;route 掛 auth layer 前(繼承 `require_bearer`)

## 2. 測試執行

### Unit / Integration（L2/L4）
- 命令：`cargo test`(獨立重跑,非採信 report)
- 結果：**lib 181 passed / 0 failed**（176 baseline + 5 新 openai;openai 模組共 15 測）、eval 4/0、deployment_contract 1/0、eval_cli 1/0、runtime_contract 4/0。**全 suite 0 failed**。
- live pipeline 整合測試(`agent_pipeline`/`fetcher_datacenter`/`streaming_datacenter`/`llm_connector`/`repro_report_data`)皆 `#[ignore]`(需 live MCP+OpenRouter)。

### E2E（L5）
- **N/A**（`has_e2e=false`）。

### 端到端手動驗證（本機 mock stub + 真實 opus-4.7 LLM，補充 L4 真整合）
拓樸 `agent:18080 → mcp:8088 → stub:9099`;stub 回符合 `dto.rs` 契約的假資料。
- T1 無 token → **418** ✓；T2 bad-json → **400** ✓；T3 空 messages → **400** ✓
- T4 非串流 → **200** OpenAI `chat.completion`(含 falcon-chart、`finish_reason:stop`) ✓
- T5 串流 → **27 chunks**(role→content 累加=完整答案→stop)+ `data: [DONE]` ✓

## 3. AC 追溯

| AC ID | 覆蓋方式 | 狀態 |
|-------|---------|------|
| AC-1 非串流回 OpenAI response | openai unit(`build_response` 2 測)+ 端到端 T4 | ✅ PASS |
| AC-2 串流 chunks + `[DONE]` | openai unit(`build_chunks` 2 測)+ 端到端 T5 | ✅ PASS |
| AC-3 不破壞現有端點 | `cargo test` 181/0 全 suite 綠(回歸) | ✅ PASS |
| AC-4 規格書 C-3 curl | 端到端 T4/T5(串流+非串流)通過 | ✅ PASS |
| AC-5 governance 與 /agent/stream 一致 | 共用同一 `plan_stream_turn` prelude(code 層一致);injection refuse 端到端未直接測 | ⚠️ PASS(邏輯共用;直接測待補) |

### ERR 追溯
| ERR | 覆蓋 | 狀態 |
|-----|------|------|
| ERR2 bad-json/空 → 400 | 端到端 T2/T3 | ✅ |
| ERR4 auth → 418 | 端到端 T1 | ✅ |
| ERR1 runtime-off → 503 | handler `rt.enabled` 檢查(邏輯已實作);端到端待重啟 RUNTIME_ENABLED=false 驗 | ⚠️ 邏輯實作 |
| ERR3 prompt>4000 → 400 | 走 prelude runtime cap(邏輯共用);端到端待補 | ⚠️ 邏輯實作 |
| ERR5 capability → 502 | `agent_error_to_openai`(邏輯已實作);端到端待 stub 回錯誤驗 | ⚠️ 邏輯實作 |

## 已知限制（非驗收失敗；spec 已記錄）
1. ~~**usage 全 0**（D3）~~ → **已解除（2026-07-23）**：usage 已實作,符合 OpenAI contract。非串流改走 streaming client + drain 收集 `AgentEvent::Usage`,`accumulate_usage` 填實值;串流於 `stream_options.include_usage=true` 時多送 usage-only chunk(`choices:[]` + `usage`)。未動 `llm.rs`/`payload.rs`,僅改新端點 helper。lib 測 181→185(全 suite 195/0)。usage 端到端 curl 待主 agent 事後驗。
2. **真實上游成功查詢**：`DATACENTER_API_BASE` 正確 host 未知,本次以本機 mock stub 驗證成功路徑;真實資料端到端待正確 host(規格書 C-3)。
3. ~~**post-stream audit**~~ → **已解除（2026-07-23）**：buffered/stream 兩路徑均寫 `AuditEvent::ResponseCompleted`/`ResponseFailed`,對齊 `/agent/stream`(prelude 另已寫 `RequestReceived`/`InputNormalized`)。**memory 因 OpenAI body 無 `session_id`(恆 `None`)仍 inert**,無 turn 可複製,故 `append_memory_turn_if_enabled` 省略(設計如此,非落差)。
4. ~~**多輪 follow-up 的 intent 分類不含 history**~~ → **已解除（2026-07-23，第三輪）**：改為在 `chat_completions` **呼叫 prelude 前**就把 history 折入 `AgentTurnInput.prompt`（`fold_history_into_prompt` 呼叫點從「Proceed 後建 `InitialPrompt`」上移到「建 `AgentTurnInput` 前」，Proceed 後直接用已 folded 的 `agent_input.prompt`，不再 double fold）。**handler-only：shared `plan_stream_turn`、`/agent/stream`、falcon 全不動**，intent 分類 / answer-policy 因此也看得到對話上下文。e2e 證：前輪問營收、本輪「那 AC 佔比呢?」現在 intent=**revenue**（conf 0.72，原 unknown 0.25）、**不再 refuse**、正常回答（403 chars、usage 非 0）。cargo lib **207/0** 回歸不破壞。（副作用：折入後的 prompt 一併過 prelude 4000-char cap；適度多輪無虞，極長對話可能觸 cap，屬既有輸入保護。）

## Code review findings 處理（2026-07-23）

review 對 `/v1/chat/completions` 提 9 項 findings,全數處理,三 gate 全綠（lib 185→**201/0**,全 suite **211/0**,5 live `#[ignore]`;`cargo check` + `cargo clippy --all-targets -- -D warnings` 皆 exit 0）。詳見 `implement-report.md`「續作 — code review findings 修正」。

| # | 嚴重度 | 處理 | 測試證據 |
|---|---|---|---|
| #1 非串流 timeout | Important | ✅ 修 | `/v1` 專屬 600s（其他 7 端點維持 120s，sub-router `.merge()`）;`per_group_timeout_layers_survive_a_merge` |
| #2 map_request 放寬 | Important | ✅ 修 | content 陣列 / developer / 相鄰同 role 合併 / 開場 assistant;6 新 unit（先 RED）+ spec 同步 |
| #3 lossy sink 正確性 | Important | ✅ 修 | answer/failure 改 `run.await` 權威,drain 只收 Usage;`resolve_outcome_...` unit（含 unknown-pipeline） |
| #4 refusal include_usage | Minor | ✅ 修 | 串流 refusal 補零值 usage-only chunk;3 refusal body 測 |
| #5 chunk/response 欄位 | Minor | ✅ 修 | `logprobs`(null) + `system_fingerprint`(null);2 序列化 pin 測 |
| #6 error status | Minor | ✅ 修 | 413/415/400 分流（`real_json_rejections_...` 經真 axum 抽取器）+ **auth /v1→401 OpenAI envelope**（不動共用 418）;判定分離乾淨故採 401 |
| #7 disclaimer 可見 | Minor | ✅ 修 | `prefix` prepend 進答案;`with_prefix_...` unit |
| #8 斷線 abort | Minor | ⏸ 保留現狀+理由 | 共用多工 MCP peer,mid-request abort 取消安全無法離線確證,與 `agent_stream` 一致,不引入 corruption 風險（無 code 變更） |
| #9 handler 整合測試 | Minor | ⚠️ 部分（結構限制） | 補所有不需 pipeline 的 handler 真實測試（refusal/json-rejection/auth/timeout）;pipeline 兩路徑端到端待 mock 基礎設施/live,未造假 mock |

## 第二輪 code review findings 處理（2026-07-23）

第二輪 review 提 6 項 findings（本表 #1–#6 為**第二輪**，與上一節第一輪 #1–#9 不同源），全數處理,三 gate 全綠（lib **202→207/0**,全 suite **0 failed**,5 live `#[ignore]`;`cargo check` + `cargo clippy --all-targets -- -D warnings` 皆 exit 0;`manifest-stack.sh` 實跑驗證通過）。詳見 `implement-report.md`「續作 — 第二輪 code review findings 修正」。

| # | 嚴重度 | 處理 | 測試證據 |
|---|---|---|---|
| #1 history 被丟棄（多輪只用最後一則） | 🔴 | ✅ 修 | 純函式 `fold_history_into_prompt` prepend transcript,`chat_completions` 織入(僅此一處,未動 engine/`/agent/stream`);3 新 unit(先 stub RED 重現 drop→GREEN):空/單輪/多輪順序 |
| #2 斷線 abort | tradeoff | ⏸ 保留現狀+理由 | 共用多工 rmcp peer,mid-request abort 取消安全離線無法確證,corruption 風險 > 成本,與 `agent_stream` 一致;supervised cancellation 列後續（無 code 變更） |
| #3 manifest value backtick | 🔴 | ✅ 修 | `## Stack`/`## Paths` value 去 markdown backtick;實跑 `manifest_stack_capability has_api→true`、`has_ui→false`、`manifest_stack_value test_cmd→cargo test`(無 backtick),BEFORE 為 exit 2 / `` `cargo test` `` |
| #4 timeout 504 空 body | 🟡 | ✅ 修 | openai sub-router 改 `HandleErrorLayer`+`tower::timeout::TimeoutLayer`,逾時回 **504 + OpenAI envelope**(standard 120s 未動);`openai_timeout_returns_openai_error_envelope`(先 stub 空 body RED→GREEN) |
| #5 include_usage content chunk 缺 `usage:null` | 🟡 | ✅ 修 | `usage: Option<Option<Usage>>`(三態)+ `build_chunks(include_usage)` 一般 chunk `Some(None)`;`content_chunks_carry_null_usage_when_include_usage`(先忽略 flag RED→GREEN)+ 更新 omit pin 測 |
| #6 pipeline-build 失敗 502 無 audit | 🟡 | ✅ 修 | buffered/stream 兩路徑回 502 前寫 `ResponseFailed{error_code,duration_ms}`(warn-only,`started`);stream fn 改 `async` await audit。無專屬 unit(失敗需 live wiring,離線無接縫;finding 未要求測),以 compile/clippy + 對齊既有 audit 模式為據 |

> **rustfmt（2026-07-24 更正）**：先前此處記「repo 基線有 14 處 fmt diff（含未觸及碼）、全庫 fmt 會 churn 無關碼，故不執行」——**此判斷已證偽**。上游 CI（`.github/workflows/runtime.yml`）以 `cargo fmt -- --check` 為 gate；PR #9 首跑即卡在此步。全庫實跑 `cargo fmt -- --check`（本機 rustfmt 1.9.0-stable，與 CI `dtolnay@stable` 產出一致）證實 diff **只有 16 處、全在本次新增的三檔**（`handler.rs`/`openai.rs`/`route.rs`），main 本身 fmt-clean、**跑 fmt 對無關碼零 churn**。已於 commit `3e5e52a` 補 `cargo fmt`（僅動此三檔、fmt 前後 test 數不變 lib 207/0，純格式化）。CI 全綠：Check formatting / Clippy(`--all-targets --all-features`) / Test / Pipeline eval / Response replay smoke 皆 ✓。教訓:push 前先跑 `cargo fmt`,勿以「churn」為由略過 fmt gate。

## 結論

✅ **驗收通過（PASS，附 notes）**。核心 AC(AC-1..4)以 unit + 端到端實測驗收;AC-3 回歸(第二輪:lib **202→207/0**、全 suite 0 failed)確認未破壞既有端點;第一輪 9 項 + 第二輪 6 項 review findings 全數處理(第二輪 #2 保留現狀含理由、#6 無專屬 unit 誠實標註),usage/真實上游/pipeline 端到端列為已知限制/待補,均已誠實記錄,不阻擋核心功能交付。

> Closeout 提示:PASS 後可在 `prd.md` 追加 `## Delivery`、`meta.yml` 標 `shipped`、升級長效 ADR/術語;未授權 branch 不自行 merge/push。
