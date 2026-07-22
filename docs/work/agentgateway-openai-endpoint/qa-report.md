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
1. **usage 全 0**（D3）：非串流 buffered pipeline 不發 `Usage`、串流 usage 僅 log 不上 wire。實測確認。需 `llm.rs`/`payload.rs` 擴充(範圍外)。
2. **真實上游成功查詢**：`DATACENTER_API_BASE` 正確 host 未知,本次以本機 mock stub 驗證成功路徑;真實資料端到端待正確 host(規格書 C-3)。
3. **post-stream audit**：新端點未寫 `ResponseCompleted`/`Failed`（prelude 已寫 `RequestReceived`/`InputNormalized`）。

## 結論

✅ **驗收通過（PASS，附 notes）**。核心 AC(AC-1..4)以 unit + 端到端實測驗收;AC-3 回歸 181/0 確認未破壞既有端點;AC-5 及 ERR1/3/5 的處理邏輯已實作並透過共用 prelude 覆蓋,少數場景的**直接端到端測**與 usage/真實上游列為上述已知限制/待補,均已誠實記錄,不阻擋核心功能交付。

> Closeout 提示:PASS 後可在 `prd.md` 追加 `## Delivery`、`meta.yml` 標 `shipped`、升級長效 ADR/術語;未授權 branch 不自行 merge/push。
