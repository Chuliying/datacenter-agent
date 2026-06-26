# Agent Runtime Rust 移植 — PRD

> 來源：本文件由 `to-prd` 依「falcon-client agent-runtime → datacenter-agent」的 brainstorming 對話與兩個 repo 的實際程式碼盤點 **彙整（synthesis）** 而成，未另行訪談。
> brainstorming 盤點來源：`falcon-client/docs/plans/chief-of-staff-agent-runtime/{capability-layer-map,implementation,notes,tasks}.md`。
> v1.1.0 依嚴格 review（subagent，2026-06-25）修訂：補上**模組可拔插 registry**、**完整 audit**、**錯誤策略**、**移植缺陷修正**。
> v1.2.0 新增**模型/skill 評估（Eval）子系統**：獨立於確定性測試的第二條驗證軸（fixtures-in-pack + response baseline + LLM-judge + CI gate）。
> v1.3.0（2026-06-26）falcon **thin-proxy 移植觸發的契約同步**：`intent.resolved` 升為 `/agent/stream` 正式 wire event（runtime 模式、附加、向後相容；拒絕路徑不送）；`AgentRequest` 加 `session_id`/`option_id`、`AgentResponse` 加 `intent`。

## 後設資料
- Slug：`agent-runtime-rust-port`
- 階段（Stage）：docs
- 版本：v1.3.0
- 建立日期：2026-06-25
- 狀態：draft
- 目標 repo：`datacenter-agent`（Rust，axum + rmcp + async-openai）
- 來源 repo：`falcon-client`（TypeScript / Next.js，`src/lib/chief-of-staff/agent-runtime/`）

---

## 問題陳述（Problem Statement）

`datacenter-agent`（Rust agent server）目前已具備真正的推理核心：`/agent`、`/agent/stream` 走 OpenAI-compatible 的 MCP tool-calling loop（`llm_connector/agent.rs`），有 bearer 認證、SSE、config 驅動的 prompt bank、greeting 預生成。但它對「使用者輸入」幾乎不做任何前處理：沒有中文 normalize、沒有 intent/slot 理解、沒有語意 guardrail（只有 64KiB body cap 與現有 handler 2000 字 prompt cap）、沒有 server 端 session memory、也沒有結構化 audit。

另一邊，`falcon-client` 已用 TypeScript 落地一整套可驗證的 agent-runtime portable core：input engineering（L5）、guardrails（L6，含 prompt-injection 偵測與回答前 answer-policy）、session memory（L12）、redacted audit（L14），並附完整單元測試。但這套能力卡在 Next.js 前端 repo、是 TypeScript，且 intent／詞庫硬綁 EV 充電營運 BI 領域，無法直接服務「機房分析」或其他垂直應用。

兩個 repo 的能力是**互補**的，但中間隔著語言（TS↔Rust）與領域（EV 充電↔通用）兩道牆。集團的目標不是只做一個 bot，而是要把 `datacenter-agent` 變成**可複用、可治理、領域無關的 LLM SaaS runtime 平台**——任何垂直應用（Chief of Staff、datacenter agent、HR/Legal/Finance…）都能靠 config **選擇並組裝**自己的模組與內容，接上同一個 runtime。

## 解決方案（Solution）

把 `falcon-client` 已落地、已驗證的 portable-core 能力（L5 input／L6 guardrails／L12 memory／L14 audit）**逐檔移植成 idiomatic Rust**，整合進 `datacenter-agent` 現有的 axum handler 與 MCP agent loop，形成單一 Rust agent server。

移植遵循兩條核心原則：

1. **機制在 Rust 程式碼，領域內容在 config**：intent allowlist、lexicon、信心門檻、注入規則等領域**資料**外部化成 config 檔。
2. **模組接線也由 config 決定（可拔插）**：input pipeline 的 stage 組成、answer-policy、各模組的**實作後端**（memory store、audit sink、guardrail 組合、slot extractor 集合）都由 config 選擇，runtime 開機時依 config 從 **registry** 查 trait object 組裝。換垂直應用＝換一份 config（資料 + 模組組裝），不動 Rust。

runtime 本身領域無關；falcon-client 的 EV 充電那套內容＋預設模組組裝，當作**第一個能力包（capability pack）**，證明「換 config 即換垂直應用、且可增減模組」。

整合方式：在 handler 與既有 agent loop 之間，加入一個移植自 `run-agent-turn` 的 Rust orchestrator，依 config 組裝出的 pipeline 串起 input → audit → answer-policy 決策 → session memory →（現有）LLM/MCP loop → 回寫 memory + audit。對外 wire 契約以 `token/done/error/clear` 為基底維持相容；runtime 串流另**附加** `intent.resolved` 首幀（thin-proxy 後前端串流 intent 的來源；附加、向後相容）。

## 使用者故事（User Stories）

### 平台 / 可拔插
1. 作為平台擁有者，我要讓 agent runtime 的機制以 Rust 落在 `datacenter-agent` 內，這樣才有單一伺服器擁有推理權威，而不是能力散落在 TypeScript 前端。
2. 作為平台擁有者，我要讓所有領域內容（intents、lexicon、門檻、注入規則）從 config 檔載入，這樣接入新垂直應用就是換一組能力包，而不是改 Rust。
3. 作為平台擁有者，我要把 falcon-client 的 EV 充電 BI 內容原樣移植成第一個能力包，這樣 config 驅動設計能對著真實已驗證的資料被證實。
4. 作為平台擁有者，我要能用 config **啟用／停用**輸入 pipeline 的各個同步 stage（normalize / input-guard / injection / intent / slots），並以獨立區段選擇 answer-policy 與 memory，這樣不同垂直應用可組出不同的處理鏈，而不改 Rust。
5. 作為平台擁有者，我要能用 config **選擇模組後端**（answer policy = rule|…、memory store = in-memory|redis|…、audit sink = stdout|file|…），這樣換實作不動 orchestrator。
6. 作為平台擁有者，我要能用 config **選擇啟用哪些 slot extractor 與 guardrail**，這樣能力包可宣告自己的抽取/防護組合。
7. 作為開發者，我要 runtime 在開機時把 config 字串對照到 Rust **registry** 內註冊的 trait object 來組裝 input pipeline、answer policy、memory、audit、eval，這樣「可插拔模組」是真的由 config 決定模組路徑，而非只換資料。

### Input engineering（L5）
8. 作為操作者，我要原始中文／中英混合輸入在分類前被 normalize（NFKC **＋全形標點對照表**、空白、大小寫），這樣詞彙比對才穩定。
9. 作為操作者，我要 intent 由 `option_id` option-path + rule-lexicon 計分加上 text-override 路徑分類，輸出帶信心分數的 canonical intent。
10. 作為操作者，我要 slots（時間範圍、metric、asset、rank limit）透過 extractor registry 抽取；**asset 的未知判定走 config allowlist**（不得硬編特定資產名），未知 asset 標記 warning。

### Guardrails（L6）
11. 作為安全負責人，我要 prompt-injection 啟發式（版本化 regex set）對每個請求執行，把這類輸入當 data 處理。
12. 作為安全負責人，我要回答前策略決策（injection 拒絕、離題拒絕、低信心加提示、其餘正常作答），讓模型不對不支援/惡意輸入掰答案，也不為拒絕燒 token。
13. 作為操作者，我要超長／空輸入在任何 LLM/MCP 呼叫前被拒絕，並**為此產出 audit 事件**，這樣成本/濫用面有界且留痕。

### Memory（L12）
14. 作為終端使用者，我要 server 端以 `session_id` 為鍵的 session memory，模糊追問能沿用先前 focus。
15. 作為操作者，我要 memory context 被 sanitize/truncate/budget 限制，並在像 system prompt 或 session id 不符時丟棄，避免 memory 變注入向量或撐爆 budget。
16. 作為開發者，我要 session memory store 藏在 trait 之後並先有 in-memory 實作，之後可換 Redis/DB 而不動 orchestrator。
17. 作為開發者，我要在 server-memory 模式下對 upstream 送 `history: []`、把記憶折進 prompt（與 TS 一致），client fallback 模式才送 `history`，避免雙重注入。

### Audit（L14）— 完整、可插拔
18. 作為合規負責人，我要 audit **涵蓋每個決策點**：request received、input normalized、input rejected（含空/超長/injection 被擋的 400 路徑）、refused、memory injected/dropped、tool called/result、answer cleared、response completed、response failed。
19. 作為合規負責人，我要每筆 audit 有 requestId/sessionId 關聯、單請求單調遞增 `seq`、PII 雜湊、secrets 遮罩（secret 名單對齊 **Rust 端**：`GLOBAL_TOKEN`/`OPENROUTER_API_KEY`，非 falcon 的 token 名），preview 預設關閉（env gate）。
20. 作為合規負責人，我要 `AuditSink` 是 trait、可由 config 選 sink，且定義 append-only/排序語意與寫入失敗策略（fail-open/fail-closed；tamper 立場至少明述），這樣 audit 本身也可插拔且可信。

### 契約 / 品質
21. 作為 API 使用方，我要既有 SSE wire 契約（token/done/error/clear）與 `/agent` JSON 契約保持相容、現有 client 持續可用；runtime 串流可**附加** `intent.resolved` 首幀（向後相容，舊 client 忽略未知 event）。
22. 作為 API 使用方，我要拒絕與提示用既有 frame 表達（拒絕文字當 token 串出、提示當開頭 token），不需新增事件型別；**語意拒絕回 200、結構性拒絕回 400** 的規則明定。
23. 作為操作者，我要 runtime 正確處理 Rust 獨有的 `LlmEvent::Clear`（清空答案 buffer），工具迴圈 preamble 不被當答案保存。
24. 作為開發者，我要 intent 表示為對 config allowlist 驗證過的 `String`（非編譯期 enum），且 config 載入時驗證：每個 `option_prefixes` 值與 `[[intent]].id` 都在 allowlist、`unknown` 必須存在、keywords 非空、id 不重複。
25. 作為開發者，我要明確的錯誤策略：runtime/config 用 `thiserror` 錯誤列舉、request path 一律 `?` 傳遞、`runtime/` request path **禁用 `unwrap`/`expect`**；config 載入失敗中止開機，per-request 失敗映射成 `AppError`。
26. 作為開發者，我要 orchestrator 只依賴 trait（`AgentPort`/`SessionMemoryStore`/`AuditSink`/`PipelineStage`/`AnswerPolicy`/`LlmInputNormalizer`），並明定 `impl Stream<LlmEvent>` → `BoxStream<AgentTurnFrame>` 的映射（含 `Clear`→清 buffer；tool call/result 必須以內部 `AgentTurnFrame` 交給 orchestrator）。
27. 作為平台擁有者，我要把（已建置的）LLM input-normalizer fallback 留成 `LlmInputNormalizer` trait + `[runtime.llm_normalizer]` config gate、預設關閉；它由 orchestrator 在 rule input pipeline 後、answer policy 前呼叫，只能補強低信心/灰區，不取代 deterministic pipeline。
28. 作為開發者，我要每個移植模組都帶單元測試，且**對 Rust 獨有路徑（config 驗證、Clear→清 buffer、refusal/disclaimer wire 序列、abort、被擋請求的 audit）另寫整合測試**，這樣全程可驗證。

### 模型 / skill 評估（Eval）— 第二條驗證軸
29. 作為平台擁有者，我要一套**獨立於確定性測試的 eval 子系統**，評估 LLM/skill 的**輸出品質**（是否 grounded、有 evidence-backed insight、避免幻覺/離題、prompt/skill 改版有無回歸），因為確定性測試只驗機制、驗不到模型行為。
30. 作為平台擁有者，我要 eval **fixtures 隨能力包走**（每個垂直應用自帶 golden set：每 intent 數筆中英測資，涵蓋 root option / 自由文字 / 模糊 / injection / no-data），這樣換應用＝換 eval 集，runtime 本身領域無關。
31. 作為平台擁有者，我要 eval 是**可拔插**的：`Evaluator` trait 至少兩類實作——**pipeline evaluator**（intent/slots/template/refuse 期望，離線 CI 必跑）與 **response evaluator**（response baseline / LLM-judge / rubric 評分 grounding/insight/離題，需 live 或 replay），由 config 選用；並輸出 latency/token/refuse/fallback 指標。
32. 作為開發者，我要 eval 可重跑且能當 **CI gate**（`cargo run --bin eval`），並區分 **pipeline eval（離線可跑）** 與 **provider-dependent response eval（需 live LLM 或 recorded replay）**，這樣回歸可被攔下、又不讓 CI 強依賴外部模型。

## 實作決策（Implementation Decisions）

### 新建／修改的模組
- **新建 `src/runtime/` 模組樹**，鏡像 `falcon-client` 的 `agent-runtime/`：
  - `config` — 載入能力包 TOML（intents/lexicon/thresholds/injection **＋ 模組組裝段**）成 typed struct，並 `validate()`。
  - `registry` — 把 config 字串對照到註冊的 trait object（input pipeline stage、answer policy、LLM input normalizer、memory store、audit sink、guardrail set、extractor set、evaluator）。
  - `schema` — `NormalizedInput`、`NormalizedSlots`、`TimeRangeSlot`。`intent` 為 validated `String`，非 enum。同時 derive `Serialize` + `Deserialize`（測試/fixtures）。
  - `error` — `RuntimeError`（`thiserror`），config/per-request 分流。
  - `input/{normalizer,intent,slots,pipeline}` — L5。normalizer 含 **全形標點對照表**（NFKC 不足以涵蓋）。
  - `guardrails/{injection,input_guard,answer_policy}` — L6。
  - `memory/{store,context}` — L12。`SessionMemoryStore` trait + in-memory 實作。
  - `audit` — L14。`AuditSink` **trait** + 預設 stdout 實作；六＋種事件、`seq`、sha256、secret 遮罩、failure policy。
  - `llm_normalizer` — 可選 fallback。`LlmInputNormalizer` trait + disabled/default gate；只在 rule pipeline 後、answer policy 前補強低信心/灰區。
  - `orchestrator` — `run-agent-turn` 移植；只依賴 trait；包現有 `llm_connector` agent loop。
  - `eval` — 模型/skill 評估：`Evaluator` trait（pipeline evaluator + response baseline + LLM-judge seam）、fixtures loader、response baseline、report；由 `src/bin/eval.rs` 重跑、可當 CI gate。
- **修改 `src/config.rs`** — 擴 `Manifest` 接 `[runtime]`（能力包檔 ref）＋ `[runtime.pipeline]`/`[runtime.answer_policy]`/`[runtime.llm_normalizer]`/`[runtime.memory]`/`[runtime.audit]`/`[runtime.guardrails]`/`[runtime.eval]` 模組組裝段。
- **修改 `src/appstate.rs`** — 加 `runtime: Arc<RuntimeConfig>`、`sessions: Option<Arc<dyn SessionMemoryStore>>`、`audit: Arc<dyn AuditSink>`、可選 `llm_normalizer`、組裝好的 `input_pipeline` 與 `answer_policy`。
- **修改 `src/server/dto.rs`** — `AgentRequest` 加 `session_id: Option<String>` 與 `option_id: Option<String>`；保留 `history` 向後相容。
- **修改 `src/server/handler.rs`** — 兩條 route 改走 orchestrator。

### 模組可拔插（registry + config）
- config 新增模組組裝段：
  - `[runtime.pipeline] input_stages = ["normalize","input_guard","injection","intent","slots"]`
  - `[runtime.answer_policy] backend = "rule"`
  - `[runtime.llm_normalizer] enabled = false, backend = "disabled"`
  - `[runtime.memory] enabled = true, backend = "in-memory"`
  - `[runtime.audit] sink = "stdout", failure_policy = "fail-open"`
  - `[runtime.guardrails] enabled = ["injection","input_guard","answer_policy"]`
  - `[runtime.slots] extractors = ["time_range","metric","asset","rank_limit"]`
- Rust 端各類有一個 registry（id → 建構函式），開機依 config 查表組裝；未知 id → config 驗證失敗中止開機。

### Config／能力包
- `config/runtime/` 放第一個能力包：`intents.toml`/`lexicon.toml`/`thresholds.toml`（**含完整 `COS_CLASSIFIER`：textOverride 0.9、marginTiers、unknown 0.25、keyword 權重**）/`injection.toml`，加上模組組裝段。
- 換垂直應用 = 換 `config/runtime/*`；不動 Rust。

### 行為契約
- **Wire 相容 + runtime 附加 `intent.resolved`。** 既有 `token/done/error/clear` 不變；runtime 串流於 intent 解析後、任何 token 前送一次 `intent.resolved`（拒絕路徑不送 → 前端落回 root topic）。拒絕 → 拒絕文字當 `Token` + `Done`（非串流 → `model_response` = 拒絕文字，HTTP 200）。提示 → disclaimer 當開頭 `Token`。**結構性拒絕（空/超長）回 400**。
- **`LlmEvent::Clear`**：orchestrator 清空 buffer；audit 記 `answer.cleared`，只保最終答案。
- **記憶權威** server 端：有 `session_id` → 讀寫 store、折進 prompt、upstream `history: []`；無 → 退回 client `history`。`option_id` 只作為本 turn 的分類/audit 訊號，不進 server memory key。

### 架構決策
- 採「模組化移植 + config 驅動領域 **+ registry 驅動模組組裝**」。orchestrator 包覆而非取代 agent loop。
- 錯誤策略：`thiserror` 列舉、`?` 傳遞、request path 禁 `unwrap/expect`。

### 移植缺陷（非 verbatim，需重設計/補表）
- **slot-extractor 的 asset 判定**：TS 內含硬編資產名/skiplist；移植時改為 **config allowlist 驅動**。
- **normalizer**：NFKC + **手工全形標點對照表**一起移植（NFKC 不轉 `、「」` 等）。
- **regex**：`/i`→`(?i)`、`\b` 對 CJK 語意不同、anchor；逐條檢視非 verbatim。
- **runtime 替代**：`Date.now`→`SystemTime`、`randomUUID`→`uuid`、`crypto.sha256`→`sha2`（加入 Cargo 依賴）。

## 測試決策（Testing Decisions）

- **測試 seam = runtime 模組邊界。** 優先測純函式（normalizer/intent/slots/pipeline/injection/answer_policy/memory::context）與 trait（`SessionMemoryStore`/`AuditSink`）；orchestrator 以注入 fake 測。
- **Rust 獨有路徑必測（不可從 TS 移植）：** config 載入/驗證（allowlist、unknown 存在、重複 id、未知模組 id）、`Clear`→清 buffer、refusal/disclaimer 的 emitted frame 序列、abort（有 buffer vs 空）、**被擋請求（空/超長/injection）產出 `input.rejected` audit**。
- **行為一致性：** 移植 TS 代表性案例；parity 需 `thresholds.toml` 含完整 classifier 區塊才寫得出（如 text-override、disclaimer 灰色）。
- **可測量驗收：** 列出 N 條移植斷言全綠 + parity diff = 0 作為 green-bar；既有 `llm_connector/agent.rs` 的 `#[cfg(test)]` 為 in-repo 範例。
- **第二條驗證軸＝模型/skill eval（獨立於上述確定性測試）：** `Evaluator` trait 分 pipeline evaluator（離線 assertion：intent/slots/action）與 response evaluator（response baseline / LLM-judge）；fixtures 隨能力包；產出 latency/token/refuse/fallback budget、insight/anti-claim 檢查。`cargo run --bin eval -- --pipeline-only` 可重跑並當 CI gate；provider-dependent response eval 需 live LLM 或 recorded replay，CI 選跑。

## 不在本輪範圍（Out of Scope）

- LLM input-normalizer 即時 fallback（I05）：只做介面 + config gate、預設關閉。
- L7/L8 正式 MCP/Tool Hub registry、RAG/Evidence Hub、L11 strict-JSON 輸出驗證、L9 skill registry。
- Eval：**框架本身（trait + runner + pipeline evaluator + response evaluator seam + 第一個能力包的種子 fixtures）在本輪範圍內**；但完整 golden-set 規模化、LLM-judge 的線上常態跑、eval 結果 dashboard 屬後續（本輪只立可重跑骨架 + pipeline CI gate）。
- 持久化 memory/audit 後端真正實作（Redis/Postgres/file）：本輪只做 trait + registry seam + in-memory/stdout 預設（**保留 config 可選**）。
- Rate limiting 真正實作延後，但**必須保留 guardrail/config seam**（之後加是 config toggle，不是重接線）。
- `falcon-client` 前端變更不屬於 Rust runtime core implementation；但 migration/release rollout 必須另以 Phase 8 完成 endpoint flag、`session_id`/`option_id` forwarding 與 TS runtime deprecation。
- `datacenter-agent` 完整 shared-skills onboarding（manifest/guardrails）：另案；缺失在 spec 標 risk。

## 補充說明（Further Notes）

- `datacenter-agent` 的 endpoints 正好對應 falcon-client `agent-client.ts` 呼叫對象——即 capability-map 的 L16「Agent Server」終態以 Rust 實現。
- 最大型別差異：TS 用編譯期 `INTENT_ALLOWLIST` enum；config 驅動平台要求 intent 為 validated `String`。
- 來源 plan 的 D12（推理權威歸屬）由本次移植回答：推理權威收斂進 Rust runtime。
- 分期建議：P1 config + registry + schema + error → P2 L5 input → P3 L6 guardrails → P4 L14 audit（完整事件 + trait）+ L12 memory → P5 orchestrator + 接線 → **P6 eval 子系統（trait + fixtures-in-pack + baseline + `bin/eval` + CI gate）**。
