---
output: docs/work/agentgateway-openai-endpoint/implement-report.md
stage: implement
slug: agentgateway-openai-endpoint
status: in_progress
---

# Implement Report — OpenAI 相容 `/v1/chat/completions` 端點

**Execution Mode**: team-feature（PRD approved / Spec approved / qa-plan ready 皆讀取）
**cargo toolchain**: `~/.rustup/toolchains/stable-aarch64-apple-darwin`（PATH 未含 cargo,以完整 toolchain 路徑執行）

## Step 1 — 環境與邊界 [PASS]
- Mode team-feature;inputs 三者皆備。
- manifest:`test_cmd=cargo test` / `typecheck_cmd=cargo check` / `lint_cmd=cargo clippy -- -D warnings`;tests 慣例為模組內 `#[cfg(test)]`。
- 範圍限 spec Files:`openai.rs`[NEW]、`mod.rs`[MODIFY]、待做 `handler.rs`/`route.rs`。

## Step 2 — RED-GREEN-REFACTOR

### 迴圈 1 — `messages → AgentRequest` 映射（TC-U01..05）
- **RED**:`map_request` = `todo!()`,`cargo test --lib server::openai` → 6 FAIL（`not yet implemented`,observed failing,非編譯錯）。
  - 修正 1:`assert_eq!` 比較整個 `Result` 需 `AgentRequest: PartialEq` → 改用 `.unwrap_err()` 只比 `MapError`（不動現有型別）。
- **GREEN**:實作 `map_request`（drop system、最後 user→prompt、前面配對→history、交替驗證）→ 6 PASS。

### 迴圈 2 — usage 累加（TC-U06）
- **RED**:`accumulate_usage` = `todo!()` → FAIL。
  - 修正:`use serde::{Deserialize, Serialize}`（新增 response DTO 需 Serialize）。
- **GREEN**:sum prompt/completion/total,reasoning 有才 `Some` → PASS。

### 迴圈 3 — chunk 切塊（TC-U07 / TC-B05）
- **RED**:`build_chunks` = `todo!()` → FAIL。
- **GREEN**:leading role chunk → char-safe content chunks（CHUNK_CHARS=24）→ terminal `finish_reason:"stop"` → PASS。空答案亦正確終止。

**目前測試**:`cargo test --lib server::openai` → **10 passed / 0 failed**（166 既有 filter）。

## 已產出/修改檔案
- `src/server/openai.rs` [NEW] — DTO（ChatMessage/ChatCompletionRequest/Usage/ReasoningDetails/ChatCompletionChunk/ChunkChoice/Delta）+ `map_request`/`accumulate_usage`/`build_chunks` + 10 unit tests。
- `src/server/mod.rs` [MODIFY] — `pub mod openai;`。

## 剩餘（未完成）
- `ChatCompletionResponse`/`Choice` DTO（非串流 response）。
- `handler::chat_completions`（最大塊):`plan_stream_turn` prelude（dummy port + no-op emit）→ 分流 StreamPlan → `wants_report_pipeline` 選 pipeline;非串流 buffered `run()`→`ChatCompletionResponse`（D2）、串流 `run_emitting`→收 Finished→`build_chunks`→SSE（D1);usage 累加。
- `route.rs`:auth layer 之前註冊 `/v1/chat/completions`。
- L4 整合測試（受控 LLM/MCP 替身）:TC-I01..04、TC-ERR01..05。
- 完整驗證:`cargo test`（全）+ `cargo check` + `cargo clippy -- -D warnings`。

## Gate（迴圈 1–3 當時，進行中）
- type-check:PENDING（完整 `cargo check` 待 handler 完成後）
- lint:PENDING（`cargo clippy`）
- test:PARTIAL — openai 純函式層 10/10;整合測試 + 全 suite 待做
- UI mockup:N/A（has_ui=false）

---

# 續作 — handler 整合批（迴圈 4–6 + handler/route）

**toolchain**：`~/.rustup/toolchains/stable-aarch64-apple-darwin/bin`（PATH prefix 執行）。
基線：進場時 `cargo test --lib server::openai` → 10 passed；全 crate 可編譯。

## Step 2（續）— RED-GREEN

### 迴圈 4 — 非串流 response DTO（AC-1 / spec D2）
- 新增 `ChatCompletionResponse{id,object:"chat.completion",created,model,choices,usage}` + `Choice{index,message:ChatMessage,finish_reason}` + `build_response(answer,id,model,created,usage)`。`ChatMessage` derive 補 `Serialize/PartialEq/Eq`（回應的 assistant message 重用同型別，spec 契約 `message: ChatMessage`）。
- **RED**：`build_response` = `todo!()`,測 `build_response_wraps_answer_as_a_single_stop_choice` + `response_serializes_to_the_openai_wire_shape` → 2 panic（`not yet implemented: build_response`,observed）。
- **GREEN**：single choice、index 0、`finish_reason:"stop"`、`role:"assistant"`、usage 透傳 → 2 PASS。

### 迴圈 5 — OpenAI 錯誤碼對映（spec Errors）
- 新增 `MapError::to_openai()->(u16,&str,String)` + `error_type_for_status(u16)->&str` + 常數 `ERR_INVALID_REQUEST/ERR_UPSTREAM/ERR_SERVER`。
- **RED**：兩函式 `todo!()`,測 `map_error_maps_to_400_invalid_request` + `error_type_tracks_the_http_status` → 2 panic（observed）。
- **GREEN**：`MapError`→(400, invalid_request_error);status 4xx→invalid_request_error、502→upstream_error、其餘 5xx→server_error → 2 PASS。

### 迴圈 6 — OpenAI 錯誤 envelope（純資料，序列化 pin）
- 新增 `OpenAiErrorBody{error:OpenAiError{message,type}}` + `OpenAiErrorBody::new`。測 `openai_error_body_serializes_to_the_nested_envelope` 固定 `{"error":{"message","type"}}` wire shape（純資料,首跑即綠,無需 RED todo）。

**openai 模組**：`cargo test --lib server::openai` → **15 passed / 0 failed**（166 filtered）。

## handler / route 整合（無新單元測；理由見下）
- `src/server/handler.rs`[MODIFY]：新增 `chat_completions`（`#[instrument(skip_all)]`)+ 私有 helper `openai_error` / `agent_error_to_openai` / `openai_chunk_event` / `build_openai_pipeline` / `openai_refusal` / `openai_buffered_response` / `openai_stream_response`。
  - 前置：body 解析失敗→400；`runtime` None/disabled→503（D7）；`map_request` Err→其 `to_openai()`（400,D5 忽略 system）；`created`（`SystemTime`→unix secs）+ `id`（`chatcmpl-{uuid}`）於 handler 生成。
  - prelude：完全比照 `agent_stream`——`UnusedAgentPort` + no-op emit + `AgentTurnDeps` → `plan_stream_turn`（route 標 `/v1/chat/completions`）。
  - 分流：`StreamPlan::Error{code,status}`→`error_type_for_status` 對映 HTTP + OpenAI error；`Refused{copy}`→200,copy 當單一 assistant（串流走 `build_chunks`,非串流走 `build_response`);`Proceed`→選 pipeline 跑。**`prefix`（disclaimer）刻意丟棄**——`/agent/stream` 終端 `clear` 本就蓋掉它,交付內容不含,對齊之。
  - 非串流（D2）：`build_openai_pipeline(None)` → `Orchestrator::run` → `Final.assistant` → `build_response`;capability 失敗→`agent_error_to_openai`（Capability→502、其餘→503）。
  - 串流（D1 偽串流）：`build_openai_pipeline(Some(sink))` → `tokio::spawn(run_emitting)` → drain：忽略 `ContentDelta`、累加 `AgentEvent::Usage`→`UsageData`、`Finished` 取完整答案、`Error` 記 failure;收完 → `build_chunks` 逐塊 `Event::json_data`（純 `data:` 行）+ `data: [DONE]`;無答案時 in-band OpenAI error object + `[DONE]`（headers 已 200,無法改 status）。
- `src/server/route.rs`[MODIFY]：auth layer **之前** `.route("/v1/chat/completions", post(handler::chat_completions))`,繼承 `require_bearer`（D6,418）。
- `src/server/mod.rs`：`pub mod openai;`（迴圈 1 已加）。

## 驗收 gate（實跑 evidence）
| gate | 指令 | 結果 |
|---|---|---|
| type-check | `cargo check` | **PASS**（Finished dev profile,無 error/warning） |
| lint | `cargo clippy -- -D warnings` | **PASS**（無 warning）;額外 `cargo clippy --all-targets -- -D warnings` 亦 **PASS**（含新測試碼） |
| test | `cargo test` | **PASS** — lib **181/0**（前基線 176 + 新 5)、eval bin 4/0、`deployment_contract` 1/0、`eval_cli` 1/0、`runtime_contract` 4/0;live 整合測試（agent_pipeline/fetcher/streaming/llm_connector/repro）皆 `#[ignore]`（需 live MCP+OpenRouter）。**全 suite 0 failed**。 |

AC-3（回歸）：既有 lib 181 + 整合測試全綠,證 `/agent/stream`、`/insight*`、`/report*`、runtime 行為未被破壞。

## TC 覆蓋
| TC | 層級 | 狀態 | 覆蓋方式 |
|---|---|---|---|
| TC-U01..05（map_request 映射/錯誤） | L2 | ✅ | openai 單元測（迴圈 1，10 測內） |
| TC-U06（usage 累加） | L2 | ✅ | openai 單元測 |
| TC-U07 / TC-B05（chunk 切塊/空答案） | L2 | ✅ | openai 單元測 |
| ERR2 對映（MapError→400 invalid_request） | L2 | ✅ | 迴圈 5 `map_error_maps_to_400_invalid_request` |
| ERR1/ERR3/ERR5 狀態→type 對映 | L2 | ✅ | 迴圈 5 `error_type_tracks_the_http_status` + envelope 序列化 |
| AC-1 非串流組裝（`build_response`） | L2 | ✅ | 迴圈 4 兩測 |
| TC-I01..04（端到端非串流/串流/回歸/injection） | L4 | ⚠️ 部分 | 回歸(TC-I03)✅;非串流/串流/injection **端到端待手動 curl（規格書 C-3）**——見下方限制 |
| TC-ERR01（runtime off→503） | L4 | ⚠️ | handler 邏輯已實作;端到端待 curl |
| TC-ERR02（bad JSON→400） | L4 | ⚠️ | 已實作;端到端待 curl |
| TC-ERR03/TC-B01/B02（4000 邊界） | L4 | ⚠️ | 走 prelude 4000 cap（D4);端到端待 curl |
| TC-ERR04（auth→418） | L4 | ⚠️ | route 繼承 `require_bearer`;端到端待 curl |
| TC-ERR05（capability→502） | L4 | ⚠️ | `agent_error_to_openai` 已實作;端到端待 curl |

## 限制與待辦（未造假,未連真實服務）
1. **L4 真整合無法離線自動化**：`McpHandle` 包住 live `rmcp::Peer<RoleClient>`,無 fake/mock 建構子;`build_*_pipeline` 內部直接建 `OpenAiLlm`/`StreamingOpenAiLlm` + `McpTool`,handler 層無注入 fake LLM/MCP 的接縫（scripted fake 僅在 `ConfiguredAgent` 單元層可用,不經 `build_*_pipeline`）。repo 既有 pipeline 整合測試（`tests/*_datacenter.rs`、`agent_pipeline.rs`、`repro_report_data.rs`）**全部 `#[ignore]` 且需 `DATACENTER_MCP_URL` + `OPENROUTER_API_KEY`**。故 TC-I01..04 / TC-ERR01..05 端到端維持**手動 curl 驗證（規格書 C-3）**,未加造假替身,未連真實 OpenRouter/MCP。
2. **非串流 usage 從缺（D3）**：buffered `OpenAiLlm::chat` 只 log 不發 `Usage`（`llm.rs:401`),且 `Orchestrator::run`（非 emitting）不收集;故非串流 `usage` 填 `Usage::default()`（全 0)。改善需動 `llm.rs`/`payload.rs`（本批範圍外,spec 已標註）。
3. **串流 usage 僅記 log**：串流累加 `AgentEvent::Usage` 後以 `tracing::info!` 輸出,**不放上 wire**——`ChatCompletionChunk` DTO 無 usage 欄位,spec SSE 範例亦無 usage 行,故不擅自加欄位。
4. **未複製 `agent_stream` 的 post-stream 副作用**：OpenAI 端點不寫 `ResponseCompleted`/`ResponseFailed` audit、不做 memory append（OpenAI body 無 `session_id`,memory 本就 inert;prelude 已寫 RequestReceived/InputNormalized/Refused）。

## 本批修改檔案
- `src/server/openai.rs`[MODIFY,untracked 新檔續補] — `ChatCompletionResponse`/`Choice`/`build_response`、`OpenAiErrorBody`/`OpenAiError`、`MapError::to_openai`、`error_type_for_status`、`ERR_*` 常數 + 5 新單元測（openai 模組 15/15）。
- `src/server/handler.rs`[MODIFY] — `chat_completions` + 7 helper。
- `src/server/route.rs`[MODIFY] — 註冊 `/v1/chat/completions`（auth 前）。

## 最終 Gate
- type-check：**PASS**（`cargo check`）
- lint：**PASS**（`cargo clippy -- -D warnings`;`--all-targets` 亦 PASS）
- test：**PASS**（`cargo test` 全 suite 0 failed;lib 181/0）
- UI mockup：N/A（has_ui=false）
- L4 端到端：**已驗證（本機 mock stub，2026-07-22）** — 見下方端到端段落

## 端到端驗證（全本機 mock stub 繞過錯 host）

拓樸：`agent(127.0.0.1:18080) → datacenter-mcp(127.0.0.1:8088) → mock stub(127.0.0.1:9099)`，
LLM 為真實 `anthropic/claude-opus-4.7`。stub 對 6 個 `starcharger/api/v2/*` endpoint 回符合
`dto.rs` 契約的假 JSON array（繞過 `DATACENTER_API_BASE=falcon.andywu.uk` 錯 host）。

| 測試 | 期待 | 實測 |
|---|---|---|
| T1 無 token | 418 | **418** ✓（D6 繼承 require_bearer） |
| T2 bad-json + token | 400 | **400** + `{"error":{"message":..,"type":"invalid_request_error"}}` ✓ |
| T3 空 messages + token | 400 | **400** `messages must contain at least one user message` ✓ |
| T4 非串流成功查詢 | 200 OpenAI `chat.completion` | **200** ✓ `choices[0].message.content`（含 falcon-chart）、`finish_reason:"stop"`、`usage` 全 0（D3） |
| T5 串流成功查詢 | SSE chunks + `[DONE]` | **27 chunks** ✓ 首塊 `delta.role="assistant"` → content 塊（累加還原=完整答案）→ 末塊 `delta:{},finish_reason:"stop"` → `data: [DONE]`；`object:"chat.completion.chunk"` |

結論：wiring / auth(D6) / 映射(D5) / prelude / pipeline 驅動 / 非串流(D2) / 串流偽串流(D1) / 錯誤 envelope / SSE `[DONE]` 全部端到端 work。D3 usage 全 0 經實測確認（已知限制）。真實上游成功查詢仍待正確 `DATACENTER_API_BASE` host（規格書 C-3）。
