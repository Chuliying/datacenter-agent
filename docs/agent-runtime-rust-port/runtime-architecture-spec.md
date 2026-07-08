# 通用 Agent Runtime — 平台架構規格（System Architecture / Design / Testing）

**Spec 版本**: v1.0.0
**性質**: 領域無關的平台架構規格。`spec/`（falcon-client 移植，已依分期拆檔）是本架構的**第一個實例**；本文件定義「任何垂直應用都能接上」的通用骨架。
**目標 repo**: `datacenter-agent`（Rust）

> 設計北極星：把 `datacenter-agent` 做成**可複用、可治理、領域無關**的 LLM SaaS runtime。四條硬性目標貫穿全文：
> ① config 驅動、**模組路徑可拔插** ② **完整 audit** ③ **SOLID Rust** ④ **全程可驗證**。

---

## 第一部分：系統架構（System Architecture）

### 1.1 分層總覽

```
┌──────────────────────────────────────────────────────────────┐
│ Edge（host-specific，axum）                                     │
│  route.rs  ·  auth  ·  dto  ·  error  ·  handler                │
└───────────────┬──────────────────────────────────────────────┘
                │ 注入 trait object（依 config 組裝）
┌───────────────▼──────────────────────────────────────────────┐
│ Runtime Core（領域無關，純機制，零 axum 依賴）                  │
│                                                                │
│  orchestrator ── 一輪 turn 的編排權威（只依賴 trait）          │
│      │                                                          │
│      ├─ InputPipeline（Vec<Box<dyn PipelineStage>>，由 config 組裝）│
│      │     normalize · input_guard · injection · intent · slots │
│      │     ← 同步、可增刪/排序                                  │
│      ├─ AnswerPolicy (trait)       ← backend 由 config 選       │
│      │                                                          │
│      ├─ SessionMemoryStore (trait)   ← backend 由 config 選     │
│      ├─ AuditSink (trait)            ← sink 由 config 選        │
│      └─ AgentPort (trait)            ← 包現有 LLM/MCP loop      │
│                                                                │
│  Registry ── id(字串) → trait object 建構函式（拔插核心）      │
│  RuntimeConfig ── 領域資料 + 模組組裝（Assembly）+ validate()  │
└───────────────┬──────────────────────────────────────────────┘
                │
┌───────────────▼──────────────────────────────────────────────┐
│ Capability Pack（config/runtime/*，每個垂直應用一份）          │
│  intents · lexicon · thresholds · injection · assembly         │
└──────────────────────────────────────────────────────────────┘
```

### 1.2 三層職責邊界


| 層                   | 職責                                               | 依賴方向                      | 可搬性                      |
| ------------------- | ------------------------------------------------ | ------------------------- | ------------------------ |
| **Edge**            | HTTP、認證、SSE、DTO、錯誤映射                             | 依賴 Core 的 trait           | host-specific，換 host 需重寫 |
| **Runtime Core**    | 編排、pipeline、memory、audit、guardrails、intent/slots | 只依賴**自身 trait**，零 host 依賴 | 純機制，可整包複用                |
| **Capability Pack** | 領域資料 + 模組組裝宣告                                    | 被 Core 載入驗證               | 純 config，換應用＝換檔          |


**鐵則**：依賴只能由外往內（Edge → Core → 自身 trait）。Core 不得 import axum/host 型別；領域知識不得寫進 Core 程式碼，只能進 Capability Pack。

### 1.3 模組組裝（拔插核心）

runtime 開機流程：

```
load config.toml
  → RuntimeConfig::load(capability pack)        # 領域資料 + Assembly
  → Registry::with_builtins()                    # 註冊內建模組 id
  → RuntimeConfig::validate(&registry)           # 驗證 allowlist + Assembly 的每個 id 都存在
  → registry.build_input_pipeline(cfg) -> Vec<Box<dyn PipelineStage>>
  → registry.build_answer_policy(cfg)  -> Arc<dyn AnswerPolicy>
  → registry.build_llm_normalizer(cfg) -> Option<Arc<dyn LlmInputNormalizer>>
  → registry.build_memory(cfg)   -> Option<Arc<dyn SessionMemoryStore>>
  → registry.build_audit(cfg)    -> Arc<dyn AuditSink>
  → AppState { runtime, pipeline, answer_policy, llm_normalizer, sessions, audit, agent_port, ... }
```

- **「拔插」的具體意義**：config 的 `assembly.`* 是一串字串 id；Registry 把 id 對照到 Rust 建構函式。新增實作＝註冊一個 id；啟用/停用/重排＝改 config 字串陣列。**orchestrator 不認得任何具體實作，只迭代 trait object。**
- 未知 id 在 `validate()` 即中止開機（fail-fast），不留到 request 時才爆。

### 1.4 一輪 turn 的資料流（領域無關）

```
Request
 → audit: RequestReceived
 → InputPipeline 依序跑 enabled stage：
     normalize → input_guard(可 400) → injection → intent → slots → NormalizedInput
 → audit: InputNormalized
 → optional LLM normalizer：只在 enabled 且低信心/灰區時補強 NormalizedInput
 → AnswerPolicy 決策：Refuse(200) | Disclaimer | Answer
 → memory stage（若啟用）：store.get → build_context → 折入 prompt（audit: MemoryContext）
 → AgentPort.stream（包現有 LLM/MCP loop）
     Token→buffer/emit · Clear→buffer.clear()(audit) · ToolCalled/ToolResult→audit · Done · Error→audit
 → store.append + audit: ResponseCompleted
```

每個箭頭都有對應 audit 事件——這是「完整 audit」與「全程可驗證」的結構保證。

---

## 第二部分：設計邏輯（Design Logic）

### 2.1 SOLID 對應


| 原則         | 落實                                                                                                    |
| ---------- | ----------------------------------------------------------------------------------------------------- |
| **S** 單一職責 | 每模組一事：normalize 只清字、intent 只分類、store 只存取、audit 只記錄。檔案過大＝職責過多訊號。                                       |
| **O** 開放封閉 | 新增能力＝在 Registry 註冊新 id / 新 Capability Pack，**不改** orchestrator 與既有 stage。                             |
| **L** 里氏替換 | 同類 trait 的任一實作可互換（in-memory↔redis store、stdout↔file audit）而行為契約不變；trait 文件定義不變量。                      |
| **I** 介面隔離 | trait 切小：`PipelineStage`/`AnswerPolicy`/`LlmInputNormalizer`/`SessionMemoryStore`/`AuditSink`/`AgentPort` 各自獨立，消費者只依賴用得到的。 |
| **D** 依賴反轉 | orchestrator 與 Edge 只依賴 trait，不依賴具體型別；具體實作在 boot 時注入。                                                 |


### 2.2 核心 trait 契約

```rust
// 拔插單位：pipeline 的一個階段
pub trait PipelineStage: Send + Sync {
    fn id(&self) -> &str;
    fn run(&self, cx: &mut PipelineCx) -> Result<(), RuntimeError>;
}

// 回答前策略：同步決策，不做 I/O；backend 可由 config 更換
pub trait AnswerPolicy: Send + Sync {
    fn id(&self) -> &str;
    fn decide(&self, input: &NormalizedInput, thresholds: &Thresholds) -> AnswerAction;
}

// 可選 LLM normalizer：不屬於同步 pipeline，預設關閉
#[async_trait] pub trait LlmInputNormalizer: Send + Sync {
    fn id(&self) -> &str;
    async fn enhance(&self, input: NormalizedInput, ctx: &LlmNormalizerContext)
        -> Result<NormalizedInput, RuntimeError>;
}

// 記憶後端（in-memory 預設，可換 redis/db）
#[async_trait] pub trait SessionMemoryStore: Send + Sync {
    async fn get(&self, scope: &SessionMemoryScope) -> Option<SessionMemory>;
    async fn append(&self, scope: &SessionMemoryScope, turn: SessionMemoryTurn) -> SessionMemory;
    async fn clear(&self, scope: &SessionMemoryScope);
}

// 稽核 sink（stdout 預設，可換 file/SaaS）
#[async_trait] pub trait AuditSink: Send + Sync {
    async fn write(&self, ctx: &AuditCtx, seq: u64, event: AuditEvent)
        -> Result<(), RuntimeError>;
}

// 推理傳輸（包現有 LLM/MCP loop，未來可換自家 model gateway）
pub enum AgentTurnFrame {
    Token(String),
    Clear,
    ToolCalled { tool: String, args_hash: String },
    ToolResult { tool: String, bytes: usize, ok: bool },
    Done,
    Error(String),
}

#[async_trait] pub trait AgentPort: Send + Sync {
    async fn stream(&self, prompt: String, history: Vec<History>)
        -> BoxStream<'static, AgentTurnFrame>;
}
```

### 2.3 Config schema（通用骨架）

```toml
# 領域資料（每個 pack 自填內容，schema 固定）
[runtime] intents=... lexicon=... thresholds=... injection=...

# 模組組裝（拔插宣告）
[runtime.pipeline]
input_stages = ["normalize","input_guard","injection","intent","slots"]
[runtime.answer_policy]
backend = "rule"
[runtime.llm_normalizer]
enabled = false
backend = "disabled"
[runtime.memory]
enabled = true
backend = "in-memory"
[runtime.audit]
sink = "stdout"
failure_policy = "fail-open"
[runtime.guardrails]
enabled = ["injection","input_guard","answer_policy"]
[runtime.slots]
extractors = ["time_range","metric","asset","rank_limit"]
```

設計原則：**schema 固定、值由 pack 決定**。Core 認得 schema 的 key，不認得 value 的領域含義。

### 2.4 錯誤策略

- `RuntimeError`（`thiserror`）分兩類：**config/boot 類**（中止開機，fail-fast）與 **per-request 類**（映射 `AppError`）。
- request path 一律 `?` 傳遞；`runtime/` request path **禁用 `unwrap`/`expect`**。
- config 驗證在 boot 完成；錯誤的 Capability Pack 永遠不會上線服務。

### 2.5 完整 audit 設計

- **事件涵蓋每決策點**：RequestReceived / InputNormalized / InputRejected（含結構性 400）/ Refused / MemoryContext / ToolCalled / ToolResult / AnswerCleared / ResponseCompleted / ResponseFailed。
- **每筆**帶 requestId、sessionId、route、timestamp(+ms)、actor(redacted)、**單請求單調遞增 `seq`**。
- **遮罩**：PII（ip/UA）sha256；secrets 名單對齊**部署端**（`GLOBAL_TOKEN`/`OPENROUTER_API_KEY`/Bearer/api-key）；明文 preview 預設關閉、hash 永遠保留。
- **可插拔**：`AuditSink` 是 trait；stdout sink 為 append-only + seq；hash-chain / tamper-proof 留待持久化 sink（明述為 deferred）。寫入失敗由 `[runtime.audit] failure_policy = "fail-open"|"fail-closed"` 控制。
- **被擋請求也留痕**：空/超長/injection 在 LLM 前被擋，仍發 `InputRejected`/`Refused`——杜絕「濫用請求無軌跡」。

### 2.6 wire 契約穩定性

- 對外只認內部 frame：`token` / `done` / `error` / `clear`。換 LLM/後端不動前端。
- 拒絕＝refusal 文字當 token 串出；提示＝disclaimer 當開頭 token；**不新增事件型別**。
- 語意拒絕回 200（拒絕節點），結構性拒絕回 400。

### 2.7 新垂直應用 onboarding（通用流程）

```
1. 複製 config/runtime/ 成新 pack 目錄
2. 填領域資料：intents / lexicon / thresholds / injection
3. 宣告模組組裝：pipeline input_stages / answer_policy / memory / audit / guardrails / extractors（增刪既有 id）
4. 若需新模組：在 Registry 註冊新 id（唯一需要碰 Rust 之處）
5. 啟動 → validate() 把關 → 上線
```

不改 Core、不改 orchestrator，即得到一個新垂直應用——這就是「平台」的可複用性證明。

---

## 第三部分：測試邏輯（Testing Logic）

### 3.1 測試金字塔與 seam


| 層             | 對象                                                                        | 方式                                                              | 數量取向            |
| ------------- | ------------------------------------------------------------------------- | --------------------------------------------------------------- | --------------- |
| **單元**        | 純函式 stage（normalize/intent/slots/injection/answer_policy/memory::context） | 直接 in/out 斷言                                                    | 多、快             |
| **trait 契約**  | `SessionMemoryStore`/`AuditSink`/`AgentPort`                              | 對任一實作跑同一套契約測試                                                   | 每 trait 一套      |
| **整合**        | orchestrator 一輪 turn                                                      | 注入 fake（fake AgentPort / in-memory store / capturing AuditSink） | 覆蓋決策分支          |
| **config 契約** | RuntimeConfig 載入/驗證                                                       | 對 pack 跑 validate                                               | 覆蓋驗證不變量         |
| **parity**    | 對 TS 來源行為一致                                                               | 移植 TS 代表案例                                                      | parity diff = 0 |


**最高 seam = runtime 模組邊界**：優先測純函式與 trait，orchestrator 用注入 fake，避免碰 host/網路。

### 3.2 各目標的可驗證對應


| 目標         | 怎麼驗證                                                                                                          |
| ---------- | ------------------------------------------------------------------------------------------------------------- |
| ① 模組拔插     | config 改 `pipeline.input_stages` → 對應 stage 跑/不跑可被斷言；未知 id → `validate()` 失敗（測試）；新 backend 註冊後可被 build 出（測試）。 |
| ② 完整 audit | capturing AuditSink 斷言「每條 turn 路徑都產出預期事件序列」，**含被擋請求發 InputRejected**。                                         |
| ③ SOLID    | trait 契約測試對多實作通用通過＝里氏/開放封閉可驗；orchestrator 只吃 trait（編譯期保證依賴反轉）。                                                |
| ④ 全程可驗證    | 下列每條都有測試，無一靠手測。                                                                                               |


### 3.3 必測清單（含 Rust 獨有路徑）

**移植自 TS（行為 parity）**

- input pipeline：intent/slots 代表案例（含 text-override、時間範圍）
- answer-policy：4 級決策
- injection：版本化規則命中
- memory-context：sanitize / follow-up / budget / session-mismatch
- session store：append 上限 / clear / actor 隔離
- audit：遮罩 / 雜湊

**Rust 獨有（無法從 TS 移植，必補）**

- config 驗證：未知模組 id / 缺 `unknown` intent / option_prefixes 不在 allowlist
- `Clear` → buffer 清空
- refusal → emitted frames = [Token(refusal), Done]，且不開 upstream
- disclaimer → 第一個 Token 為提示
- abort：有 buffer → completed(aborted)；空 → failed
- 被擋請求（空/超長）→ capturing AuditSink 收到 `InputRejected`

### 3.4 驗收 green-bar（可測量）

- 所有單元 + 整合 + config 契約測試綠燈。
- parity：移植的 N 條 TS 斷言全數通過，diff = 0。
- 編譯：`cargo build` + `cargo clippy` 無 warning；`runtime/` request path 無 `unwrap/expect`（以 lint / grep 驗）。
- audit 覆蓋：每條決策路徑至少一個事件被測試斷言。

### 3.5 in-repo 測試範例

`src/llm_connector/agent.rs` 的 `#[cfg(test)]`（tool-call 組裝、request 建構）是要遵循的既有 Rust 風格；新 runtime 測試沿用同樣的 `#[cfg(test)] mod tests` 與 `#[tokio::test]` 慣例。

### 3.6 第二條驗證軸：模型 / skill eval

確定性測試驗**機制**，驗不到 LLM/skill 的**輸出品質**。eval 是獨立的第二軸，與其他模組同一套**可拔插**機制：

- **`Evaluator` trait**（經 Registry 註冊、由 config 選用）：
  - `PipelineEvaluator`：比對 intent/slots/template/action——**離線可跑、CI 必跑**。
  - `ResponseBaselineEvaluator` / `LlmJudgeEvaluator`：比對答案形狀、must(_not)_include 或用 rubric 評分 grounding/insight/relevancy；需 live LLM 或 recorded replay，**CI 選跑**。
- **fixtures 隨能力包**：`config/runtime/evals/inputs.json`（每 intent 數筆中英測資，涵蓋 root-option / free-form / ambiguous / injection / no-data）；換垂直應用＝換 golden set。
- **response baseline**：`response-baseline.json` 定 golden answer shape + latency/token/refuse/fallback budget + insight/anti-claim 檢查；`EvalReport::is_green(baseline)` 為 gate。
- **response baseline**：`response-baseline.json` 是本 migration 新產物；若 source repo 沒有同名 baseline，Phase 0/6 必須從 recorded TS responses 或 approved live sample 建立初版。
- **可重跑 + CI gate**：`cargo run --bin eval -- --pipeline-only` 必跑；`cargo run --bin eval -- --response --replay <file>` 或 live provider 選跑。
- **green-bar**：種子 fixtures 的 pipeline eval 全綠；response eval 無 baseline 退步。

> eval 對應 capability-map L14 的「可重跑 eval + response baseline」，是「全程可驗證」對模型行為的延伸；確定性測試 + eval 兩軸並行才算完整驗證。

---

## 附錄：本架構與既有 capability-layer-map 的對應


| 通用架構元件                            | capability-map 層                    |
| --------------------------------- | ----------------------------------- |
| Pipeline（input stages）            | L5 Input Engineering                |
| guardrails stages + answer_policy | L6 Guardrails                       |
| AgentPort（包現有 loop）               | L4 Orchestration / L10 LLM / L7 MCP |
| SessionMemoryStore                | L12 Memory                          |
| AuditSink                         | L14 Audit                           |
| Registry + Assembly config        | PX Platform Control Plane（雛形）       |


> 本架構是 capability-map「平台級終局」的可落地最小骨架：先把**可拔插 + 可治理（audit）+ 可驗證**三根支柱立起來，後續 L7/L8/L11 等層皆以「註冊新 stage / 新 trait 實作」方式增生，不需重構 Core。
