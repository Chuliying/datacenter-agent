# Privacy Proxy 技術規格

**Spec 版本**：v0.2.0
**對應 Feature PRD**：[`./prd.md`](./prd.md) v0.1.0（S-PRIVACY-01）
**對應全域**：[reference PRD](../../prd.md) FR-015、[reference spec](../../spec/spec.md) §2.5/§6
**狀態**：Target-state design（待建置）
**Source**：接縫對照 `src/agent/payload.rs`、`src/agent/wiring.rs`、`src/agent/events.rs`、`src/server/handler.rs`、`src/config.rs`、`src/runtime/registry.rs`（見 §10）；設計已經獨立 subagent 對照 codebase review（verdict: approve-with-changes）

> 本 spec 描述**尚未實作**的目標設計，內容 self-contained。所有型別/檔案為規劃。
>
> **⚠️ Baseline 更新（v0.2.0）**：接線基準由 v0.1.0 的 `813b275`（pre-subagent）改為 **`feature/subagent-separation` @ `02cdd41`（PR #5）**。PR #5 把答案生成從單一 `runtime::orchestrator` 的 `LlmAgentPort` 換成多階段 sub-agent pipeline（`src/agent/`），並把 `orchestrator` 模組改名為 `runtime::turn`。原 v0.1.0「包 `AgentPort` decorator」策略在新生產路徑已無對應接縫，§2/§7/§8/§10 據此重寫；§4–6 的偵測/store/crypto 引擎不受影響。

## 版本歷史

| 版本 | 日期 | 內容 |
|---|---|---|
| v0.1.0 | 2026-07-11 | 初版；模組設計 + 對照 codebase review（2 critical + 6 major 已併入）落為技術 spec |
| v0.2.0 | 2026-07-13 | 對齊 PR #5 sub-agent 架構：整合策略由「`AgentPort` decorator」改為「sub-agent 出境邊界 decorator」（`LlmCapability`/`Tool` + handler 還原）；§2 模組佈局、§7 Pipeline 整合、§8 Phase 4、§9 整合測試、§10 接縫全面重寫。引擎（§4–6）不變。經一輪對抗 review 修正：privacy 掛 `AppState`（非 `AppRuntime`，否則 rollback fail-open）、greeting 需改走 `build_stage_llm`、還原掛 async drain loop（非 sync `insight_frames`）、補 lossy-sink 終局處置、`PrivacyTool` 只包 `McpTool`。 |

## 1. 架構總覽

```text
prompt/合約文字 ─► PrivacyEngine.mask(rules + 選配 Ollama NER) ─► 殘留掃描 ─► 雲端 LLM
                        │ tag↔原文 upsert                            │ ⟦TAG⟧ 鎖定指令
                   SQLite(AES-256-GCM) ◄── resolve ── StreamRestorer.restore(容錯)
                                                            │
                                                            ▼ SSE 還原後原文出給前端
tool 呼叫：args 先 restore（datacenter 工具收到真名）；tool result 是本機資料，留 artifact 明文——其 egress 由「LLM 出境邊界」統一遮罩（pipeline 唯一上雲點，見 §7）
```

偵測全程本機。停用時（`[runtime.privacy].enabled=false` 或缺 section）不建構任何元件。

## 2. 模組佈局 — 引擎 `src/runtime/privacy/` + pipeline decorator `src/agent/privacy.rs`

| 檔案 | 內容 |
|---|---|
| `mod.rs` | `PrivacyEngine` facade、`PrivacyScope`、`MaskOutcome`/`RestoreOutcome`、`PRIVACY_SYSTEM_DIRECTIVE` |
| `config.rs` | `PrivacyPack`（解析 `config/runtime/privacy.toml`，`deny_unknown_fields`）、`OnDetectorError`/`ResidualAction` |
| `detect.rs` | `EntityKind`(Person/Org/Twid/Ubn/Phone/Email/Addr/Bank)、`EntitySpan`、`Detector` trait、`RuleDetector`、重疊解析 |
| `checksum.rs` | `twid_is_valid`、`ubn_is_valid` 純函式 |
| `ollama.rs` | `OllamaNerDetector`(reqwest → 本地 Ollama)、`HybridDetector`(合併 + 降級策略) |
| `tags.rs` | `TagGrammar`：canonical tag 渲染、fuzzy 解析、streaming 增量前綴匹配 |
| `store.rs` | `MappingStore` trait、`InMemoryMappingStore`、`SqliteMappingStore` |
| `crypto.rs` | env 金鑰載入、HKDF 派生、AES-256-GCM seal/open、HMAC lookup hash |
| `stream.rs` | `StreamRestorer` — 邊界感知的 streaming 還原器 |
| `src/agent/privacy.rs` | **（PR #5 對齊，非 `runtime/privacy/`）** `PrivacyLlm<L: LlmCapability>`（出境 mask + system directive + 殘留掃描）、`PrivacyTool`（tool args JSON-aware restore）——實作 `src/agent/payload.rs` 的 `LlmCapability`/`Tool`，持有 `Arc<PrivacyEngine>`（見 §7.1）|

核心 API：

```rust
impl PrivacyEngine {
    async fn mask(&self, scope: &PrivacyScope, text: &str) -> RuntimeResult<MaskOutcome>;    // 偵測→配tag→殘留掃描，fail-closed
    async fn restore(&self, scope: &PrivacyScope, text: &str) -> RuntimeResult<RestoreOutcome>; // 容錯還原 + 完整性驗證
    fn stream_restorer(self: &Arc<Self>, scope: PrivacyScope) -> StreamRestorer;
    fn system_directive(&self) -> &'static str;  // 告知雲端 LLM tag 不可改
    fn spawn_ttl_cleanup(self: Arc<Self>);
}

#[async_trait]
pub trait MappingStore: Send + Sync {
    async fn upsert(&self, scope, kind, original) -> RuntimeResult<u32>;      // 同實體→同 index（HMAC dedup）
    async fn resolve(&self, scope, kind, index) -> RuntimeResult<Option<String>>;
    async fn purge_expired(&self, now_ms: u64) -> RuntimeResult<u64>;
}
```

新增 `RuntimeError` variants（`src/runtime/error.rs`）：`PrivacyStore`、`PrivacyDetector`、`PrivacyResidual`。

> **依賴方向（PR #5 對齊）**：偵測/store/crypto 引擎自成一體放 `src/runtime/privacy/`（不依賴 `agent`）；pipeline decorator（`PrivacyLlm`/`PrivacyTool`）放 `src/agent/privacy.rs`，實作 `agent::payload` 的 trait 並持有 `Arc<PrivacyEngine>`——即 `agent` → `runtime::privacy` 單向依賴。`StreamRestorer`（`stream.rs`）留在引擎側，由 `handler.rs::insight_frames` 呼叫。

### 2.1 Tag 格式：`⟦PERSON_1⟧`

- `⟦⟧`（U+27E6/27E7）幾乎不出現在合約原文，避開中文合約的 `[]`／法條引用／Markdown 誤判。
- 內文純 ASCII（`PERSON_1`），tokenizer 穩定、LLM 照抄可靠。
- 同實體同 scope → 同 tag（store dedup），跨份合約指涉一致。
- 還原容錯：接受 `[PERSON_1]`、`【PERSON_1】`、`⟦ PERSON_1 ⟧`、`⟦person_1⟧`、全形數字等變體。
- System directive（啟用時附加至 system prompt）：「文中形如 ⟦PERSON_1⟧ 的符號是隱私代稱，回答時必須原樣照抄，不得改寫、翻譯、拆解或猜測其原始內容」。

## 3. Config 設計

### 3.1 `config/config.toml`（仿 `[runtime.memory]`；`src/config.rs` `RuntimeManifest` 加 `#[serde(default)] privacy: Option<…>` — optional 不破壞既有部署）

```toml
[runtime.privacy]
enabled = false               # 總開關：false/缺 section = 完全不建構，零行為改變
detector = "rules"            # "rules" | "hybrid"（hybrid = rules + Ollama）
store = "sqlite"              # "in-memory" | "sqlite"
on_detector_error = "block"   # "block" | "rules-only"（Ollama 掛掉時的策略）
patterns = "runtime/privacy.toml"
db_path = "data/privacy.db"
```

### 3.2 `config/runtime/privacy.toml` pattern pack（仿 `injection.toml`，boot 時編譯 regex、fail-loud）

含：`[tags]`（括號定義與 fuzzy 變體）、`[detection]`（`languages`、`scope`、統編 context 要求）、`[[rules]]`（各 PII 規則，見 §4，`lang` 標記）、`[llm]`（Ollama url/model/timeout/chunking）、`[residual]`（`action = "block"|"warn"`）、`[store]`（`ttl_hours=168`、`cleanup_interval_secs`）。

### 3.3 環境變數

| Var | 用途 |
|---|---|
| `PRIVACY_MASTER_KEY` | 64 hex → 32-byte 主金鑰；sqlite store 啟用時**開機必填，缺少即 boot 失敗** |
| `PRIVACY_DB_PATH` / `PRIVACY_OLLAMA_URL` | 選填 override（容器掛載逃生口）；`PRIVACY_OLLAMA_URL` 非 loopback 即拒絕啟動 |

### 3.4 註冊與組裝

- `src/runtime/registry.rs`：`BuiltinRegistry` 加 `privacy_detectors`/`privacy_stores` id 集合、`require_privacy_*` 驗證、`build_privacy(&cfg) -> RuntimeResult<Option<Arc<PrivacyEngine>>>`（disabled → `None`）。
- `src/appstate.rs`：**⚠️（對抗 review 更正）privacy engine 掛在 `AppState`（由 `AppState::new` 無條件建構），不是 `AppRuntime`。** 因為 `/insight`、`/report` 不 gate 於 runtime，而 `AppRuntime` 在 `RUNTIME_ENABLED=false` 時為 `None`（`build_runtime_for_flag`，`appstate.rs:306`）——若放 `AppRuntime`，rollback 會讓 direct 端點無遮罩出境（fail-open，與 FR-P07 矛盾）。PR #5 的 `insight_grants`/`report_template` 也都在 `AppState`（`appstate.rs:179/182`），新增 `privacy` 欄位一併更新 struct-literal 與測試。`build_privacy` 仍放 `registry.rs`，但由 `AppState::new` 呼叫。
- `src/main.rs`：`if let Some(p) = &runtime.privacy { p.clone().spawn_ttl_cleanup(); }`。
- **Pipeline 注入**：`src/agent/wiring.rs` 的 `build_insight_pipeline` / `build_report_pipeline` / `build_greeting_pipeline` 各加 `privacy: Option<&Arc<PrivacyEngine>>` + `scope`；`handler.rs` 的各 pipeline 端點與 greeting 在 `state.runtime.privacy` 為 `Some` 時傳入（見 §7.1.4）。

## 4. 規則引擎（台灣 PII）

Rust `regex` 無 lookaround、`\b` 對 CJK/數字交界無效。**邊界檢查不可用 guard group 消耗字元**（`(?:^|[^0-9A-Za-z])(pattern)(…)` 會吃掉分隔字元，`find_iter` 非重疊掃描導致相鄰 PII 如 `0912345678,0987654321` 第二筆漏抓 → 洩漏）——改為：core pattern 直接匹配，再以 `char_indices` 手動驗證 span 前後邊界字元。偵測前先 NFC 正規化。

| Kind | 方法 | 重點 |
|---|---|---|
| `twid` 身分證/新式居留證 | `[A-Z][1289]\d{8}` + checksum | 字母代碼表（I=34, O=35, W=32…），加權和 mod 10 == 0；checksum 不過即不遮（避免誤遮案號） |
| `ubn` 統編 | 8 碼 + checksum + **context keyword** | 財政部 2023 新制：權重 [1,2,1,2,1,2,4,1] 位數和 mod 5 == 0（或第 7 碼 = 7 時 +1 成立）；mod-5 誤中率 1/5，必須配「統編/統一編號/發票」等關鍵字才遮 → 保護日期不被誤遮 |
| `phone` | regex | 手機 09XX/+886-9、市話 0[2-8]，容忍空白/連字號 |
| `email` | regex | 標準 pattern |
| `addr` | 啟發式 regex | 兩層：縣市錨定（高信心）、路街錨定（低信心）；全形/中文數字巷弄號 |
| `bank` | context 兩段式 | 先找「帳號/匯款/受款/銀行」關鍵字，後方 ~40 字掃 7–16 位數字串；後綴「元/萬/億」或前綴「NT$」即排除 → **金額永不遮** |
| `org` | regex | 公司後綴捕捉（股份有限公司/有限公司/事務所…） |
| `person` | regex + LLM | 稱謂錨定（先生/小姐/經理…）、合約角色錨定（甲方/乙方/負責人:XXX）；自由文本人名靠 Ollama 層 |

重疊解析：checksum 類 > email > phone > addr > bank > org > person；同 kind 取最長 span；LLM span 不覆蓋規則 span。

### 4.1 多語支援（zh-TW / en / ja）

原則：**規則按語言標記、config 選擇啟用、tag 與引擎語言中立**（`⟦PERSON_1⟧` 純 ASCII；NFC 對三語皆適用）。

| Kind | en | ja |
|---|---|---|
| 結構化證號 | (選配) US SSN/EIN | (選配) マイナンバー 12 碼、法人番号 13 碼（皆 checksum 可驗） |
| `phone` | `+1`/E.164、`(XXX) XXX-XXXX` | `0X0-XXXX-XXXX`、`0X-XXXX-XXXX`、`+81` |
| `org` | 後綴 `Inc.\|Ltd.\|LLC\|Corp.\|Co.,? Ltd.\|GmbH` | 前/後綴 株式会社/有限会社/合同会社 |
| `person` | `Mr./Ms./Dr. + 大寫名`、`Party A: John Smith` | 敬稱 `〜様/〜殿/〜氏`、役職（代表取締役 XXX） |
| `addr` | `數字 + 街名 + St/Ave/Rd/Blvd, City, State ZIP` | `都道府県…市区町村…丁目N番N号`（含全形數字） |
| `email` | 三語共用同一 pattern | 同左 |

- 字元類：日文加 `\p{Hiragana}\p{Katakana}`；邊界驗證統一為「span 前後字元不得與 span 同 script 類」。
- **Ollama NER 天然多語**：`qwen2.5:7b-instruct` 支援中/英/日，system prompt 多語版；`person`/`org` 自由文本抓取三語共用同一 LLM 層。
- 混語文件不需語言偵測：所有啟用語言規則同時掃描，重疊解析統一處理。

## 5. Ollama 整合（hybrid 模式）

- `POST {url}/api/chat`，`stream:false`，`temperature 0`，用 Ollama **schema-constrained `format`** 強制回 `{entities:[{text, type: person|org}]}`。
- 預設模型 `qwen2.5:7b-instruct`（繁中 NER 佳）；低記憶體替代 `qwen2.5:3b-instruct`；可 config。
- **不信任 LLM offset**：對每個回傳 `text` 在 chunk 內字串搜尋找所有出現位置產 span（confidence 0.7）；非逐字出現即丟棄；**實體長度 ≥2 字**，且 LLM 實體若是既有地址 span 的子字串即丟棄（避免「大同」誤中「大同區」）。
- 大合約 chunking：1500 字 + 200 overlap，斷點偏好 `\n`/`。`，overlap 區去重，並發上限 2。
- 故障策略依 `on_detector_error`：`block` → 整回合失敗、不出境；`rules-only` → 規則 span 繼續 + 稽核 `PrivacyDetectorDegraded`。
- 測試不需 Ollama：`Detector` 是 trait，注入 `FakeNerDetector`。

## 6. SQLite 對照表 + 加密

**選型：`rusqlite`(bundled) + RustCrypto `aes-gcm`，不用 SQLCipher** — 專案刻意全 rustls/無 OpenSSL 連結，SQLCipher 會破壞 hermetic/musl build；欄位級 AEAD 只加密敏感原文，kind/index/TTL 保持可查詢。

新依賴：`rusqlite = { version = "0.37", features = ["bundled"] }`、`aes-gcm = "0.10"`、`hmac = "0.12"`、`hkdf = "0.12"`。

```sql
CREATE TABLE privacy_mappings (
  id INTEGER PRIMARY KEY,
  scope_id TEXT NOT NULL, entity_kind TEXT NOT NULL, tag_index INTEGER NOT NULL,
  original_hash BLOB NOT NULL,   -- HMAC-SHA256(mac_key, scope||kind||NFC(原文))：dedup 兼防字典攻擊
  original_nonce BLOB NOT NULL, original_ct BLOB NOT NULL,  -- AES-256-GCM，每列隨機 nonce
  created_at_ms INTEGER NOT NULL, expires_at_ms INTEGER NOT NULL,
  UNIQUE (scope_id, entity_kind, tag_index),
  UNIQUE (scope_id, entity_kind, original_hash)
);
```

- **金鑰分離**：`PRIVACY_MASTER_KEY` 經 HKDF-SHA256 派生 `enc_key`(AES-GCM) 與 `mac_key`(HMAC)，不同 info label；HMAC 輸入與 AAD 各欄位 length-prefix 分隔（避免 `("a","bc")` vs `("ab","c")` 歧義）。
- AAD = `scope_id||kind||tag_index`（防 ciphertext 換列）；同一 transaction 內先配 index → 加密 → insert；解密失敗 → fail-closed。
- **upsert 原子性**：單一 transaction（`BEGIN IMMEDIATE`）完成 SELECT → MAX+1 → INSERT；撞到 `original_hash` UNIQUE 衝突時重新 SELECT 既有列回傳（並發同實體不失敗）。`InMemoryMappingStore` 鏡射同語意。
- rusqlite 是 sync：`Arc<Mutex<Connection>>` + `spawn_blocking`；WAL mode。
- TTL：背景任務每 `cleanup_interval_secs` 清理；過期 tag 還原時列 `unresolved`，不回舊資料。
- Scope：**v1 只做 `"global"`**（跨 session 跨合約 tag 一致 — 符合「數份合約」需求）；`"session:<id>"` 延後。trade-off：global 讓雲端可跨 session 關聯 `⟦PERSON_1⟧`。

## 7. Pipeline 整合 — 在 sub-agent 出境邊界做 decorator（對齊 PR #5）

**為何不再包 `AgentPort`**：v0.1.0 假設答案由 `runtime::orchestrator` 的單一 `LlmAgentPort` 產生，故包一層 `PrivacyAgentPort` 即可攔全部出境。PR #5 之後前提不成立——答案改由 `src/agent/` 的多階段 sub-agent pipeline（fetcher→analyst→charter/composer→finalizer/renderer）產生，`run_agent_turn`/`LlmAgentPort` 在生產路徑已不再被呼叫（僅存於 `runtime::turn` 的測試）。因此改在 pipeline 的**三個出境 chokepoint**掛 decorator。

**關鍵洞見**：整條 pipeline 只有一種東西會上雲——每個 stage 的 `LlmCapability::chat()` 呼叫 OpenRouter。stage 之間的 artifact store 只在本機記憶體傳遞。所以「原文不出境」只要守住 **LLM 邊界一處**即可，比 v0.1.0 的 per-path 遮罩更集中、更難漏。

### 7.1 三個掛載點（全在 `src/agent/` + `handler.rs`，不再碰 `runtime::turn`）

1. **LLM 出境遮罩 — `PrivacyLlm<L: LlmCapability>`**
   包 `Arc<dyn LlmCapability>`（trait 定義於 `src/agent/payload.rs`，`chat(&[LlmMessage], &[ToolSchema]) -> LlmResponse`；decorator 可 clone+mask 出境 messages、在 `System` message 附 directive、回傳 inner 回應不變）。**主插入點：`src/agent/wiring.rs::build_stage_llm()`**——insight/report pipeline 的每個 stage LLM（含預設 `llm` 與 minimal-reasoning 的 `llm_low`）都由這個工廠建立，privacy 啟用時把回傳的 inner 包一層即可。
   **⚠️ 例外（對抗 review 抓出）：`build_greeting_pipeline` 不走 `build_stage_llm`，而是直接 `OpenAiLlm::from_resolved(...)`（`wiring.rs:345-350`）。** 故 Phase 4 必須一併把 greeting 的兩個 LLM 改由 `build_stage_llm` 建立（或就地包 `PrivacyLlm`），否則 boot-time greeting 成為未遮罩的出境漏洞。
   `chat(&[LlmMessage], &[ToolSchema])` 行為：出境前對每則 `System/User/Assistant{content}/Tool` message 內容 `engine.mask` → 在 System message 附加 `system_directive()`（tag 不可改）→ 殘留掃描；任一步 fail → 回 `AgentError`（stage fail）→ **fail-closed，原文不出境**。回應原樣返回（tag 還原在 chokepoint 3）。
   **這一個邊界即涵蓋**：user prompt、跨 stage 傳給 LLM 的 material artifact、以及 fetcher tool result 回餵給 LLM（`run_llm_loop` 會把 tool 輸出當 tool message 再送一輪 chat）——全部同一條路上雲。

2. **工具參數還原 — `PrivacyTool`**
   包 `Box<dyn Tool>`（trait `async call(&self, arguments: serde_json::Value) -> ToolOutcome`——args 是 **per-call 參數，可攔截**；對照 `StreamingTool` decorator 寫法）。**插入點：`src/agent/wiring.rs::build_tool()`/`build_stage_tools()` 建 `McpTool` 之處**。（註：別以 `StreamingTool::wrap_all` 為錨——它只在測試用，production 的 `ConfiguredAgent` 直接跑 `self.tools`。）
   **只包 `McpTool`（datacenter 查詢），不包 code-backed sink `emit_chart`/`emit_report`**——後者是 `SchemaTool<T>` sink，其 args 本身就是模型產出的 artifact（`ChartBatch`/`ReportData`），沒有 PII 代稱要還原，包了反而誤改。
   `call()`：先把 LLM 提出的 tool args 內 `⟦TAG⟧` **restore 回原文**（masked context 下 LLM 只知代稱，datacenter 需真實 id 才查得到），再呼叫 inner。**JSON-aware**：parse 成 `serde_json::Value` 後只在 string value 內替換再序列化，禁止 raw string 取代（原文含 `"`/`\` 會破壞 JSON）。tool result **不遮**（本機資料，留 artifact 明文；egress 由 chokepoint 1 統一處理）。
   註：datacenter MCP 是資料擁有者（可信端），故 args restore 是**正確性**需求而非洩漏防護；威脅模型唯一 untrusted 端是雲端 LLM。若未來某 tool 呼叫 untrusted 外部服務，該 tool 需自帶 args/result 遮罩（follow-up）。

3. **輸出還原 — `StreamRestorer` 掛在 handler 的 async drain loop**
   還原**必須**在 async 情境做（`store.resolve` 是 async），而 `EventSink::emit`（`src/agent/events.rs`）是 sync fire-and-forget——**不能包 sink**。**⚠️（對抗 review 更正）`insight_frames` 本身是 sync `fn(AgentEvent) -> Vec<StreamFrame>`（`handler.rs:765`），也不能 await。** 正確掛載點是它外層的 async `async_stream::stream! { while let Some(event) = rx.recv().await { … } }` drain loop（`handler.rs:274/353/600`）——在 loop 內、呼叫 `insight_frames` 之前對事件還原：
   - `Finished`（→ `clear + token(完整答案)`）：對完整答案做一次 batch `engine.restore`（權威輸出）。
   - live-preview `ContentDelta`：privacy 啟用時**建議一併**經 `StreamRestorer`（§7.3）邊界感知還原（理由見下）。
   - **lossy sink 交互（對抗 review 補）**：`ChannelSink::emit` 在 buffer 滿時 `try_send` 丟棄（`events.rs:186`），**終局 `Finished` 也可能被丟**。若只還原 post-`clear` 完整答案，一旦 `Finished` 掉了，client 只剩未還原的 `⟦TAG⟧` 預覽且永不還原。故 privacy 啟用時要嘛還原 live `ContentDelta`（degrade gracefully），要嘛保證終局 frame 無損送達。
   buffered 端點（`/insight`、`/report`）在 `Orchestrator::run` 回傳的 `Final` 上做 batch restore；boot-time greeting 在 `server/greeting.rs::build_one_greeting` 的 `Final.assistant` 上做 batch restore。

4. **privacy engine 注入路徑**
   `build_insight_pipeline` / `build_report_pipeline` / `build_greeting_pipeline` 各加 `privacy: Option<&Arc<PrivacyEngine>>` + `scope: PrivacyScope`，往下傳給 `build_stage_llm` 與 `build_stage_tools`；`None` 分支 = 今日逐位元組原樣。呼叫端 `handler.rs`（`/insight`、`/insight/stream`、`/report`、`/report/stream`、`/agent/stream` 與 boot-time greeting）在 `state.runtime.privacy` 為 `Some` 時傳入。

### 7.2 出境路徑覆蓋（PR #5 後重新盤點）

**最大改善**：PR #5 後所有會上雲的端點都共用 `src/agent/wiring.rs` 的 builder，因此 **在 `build_stage_llm` 單點注入 = 全端點自動覆蓋**。v0.1.0 標為 critical 的「`/report` 繞過 runtime 直呼 `llm_connector`」已消失——`/report` 現在走 sub-agent report pipeline，與其他端點同一條 builder。

| 路徑 | PR #5 後現況 | 對策 |
|---|---|---|
| `/insight`、`/report` 及其 `/stream` | 直接 pipeline，**繞過 runtime turn 的 guardrail**，但仍過 builder | privacy 在 builder 層注入即覆蓋；遮罩與 guardrail **解耦**（privacy 不依賴 runtime 開關）— 反而是好事 |
| `/agent/stream` | `plan_stream_turn` 前導（本機 guardrail/intent/memory，不上雲）+ pipeline | prompt 遮罩由 chokepoint 1 處理；無需在 prelude 另外遮罩 |
| `RUNTIME_ENABLED=false` | `/agent/stream` 回 503，client 落到 direct 端點 | direct 端點過 builder，privacy 仍生效——**前提是 privacy 掛 `AppState` 而非 `AppRuntime`**（見 §3.4 對抗 review 更正；否則 rollback 反而 fail-open）。成立後，v0.1.0「runtime 停用 → 拒絕開機」不變量可放寬 |
| boot-time greeting | 直建 LLM，未走 `build_stage_llm`（`wiring.rs:345`）| **必修**：改走 `build_stage_llm` + 對 greeting `Final` batch restore，否則未遮罩出境 |
| Boot 不變量（改） | — | 保留較弱一條：privacy 啟用時，任何 pipeline builder 拿不到 engine 即 fail-closed，**絕不建出無 privacy 的上雲 pipeline** |
| MCP tool（datacenter） | 資料擁有者，可信端 | 只需 args restore（正確性）；非洩漏面 |
| Ollama NER | 本機偵測 | `PRIVACY_OLLAMA_URL` 非 loopback 即拒絕啟動（不變）|
| Legacy `llm_connector`（`generate`/`agent_stream`） | PR #5 後在生產已非答案路徑 | **待 reviewer 確認**：無任何 production caller 繞過 builder 直呼 `llm_connector` 上雲（若有，需補遮或標 dead）|

### 7.3 Streaming 還原器（`⟦TAG⟧` 可能被 token 切開）

演算法不變：掃到可能開 tag 的字元即扣住，增量文法匹配（`OPEN WS? KIND (SEP INDEX)? WS? CLOSE`）；完整匹配 → resolve 還原；有效前綴未完 → 續扣；超過 32 字上限未成 tag → 吐 1 字重掃（延遲有界，合約含 `[` 不會無限緩衝）；stream 結束（`done`/`error`/中途 abort）→ flush holdback + 完整性統計。差異僅**掛載點**：由 v0.1.0 的 `PrivacyAgentPort` frame stream 改為 `handler.rs::insight_frames`（見 §7.1.3）。

### 7.4 殘留掃描 + fail-closed 矩陣

| 事件 | 行為 |
|---|---|
| `PrivacyLlm::chat` 內、mask 後殘留掃描（排除 tag span 重跑 RuleDetector）發現 checksum 級/高信心 PII | `residual.action`：`block`（預設，chat 送出前回 `AgentError`，stage fail）/ `warn`（僅稽核）|
| 偵測錯誤（Ollama down） | `block` 或 `rules-only`（per config） |
| store 錯誤 / 解密失敗 | block |
| 還原時 unresolved tag | 照原樣輸出 tag + 稽核 `PrivacyRestoreIncomplete`（代稱本身無害，不硬失敗傷可用性） |

### 7.5 稽核（仿 `audit.rs` hash-never-log）

`PrivacyMasked { entities, kinds, degraded, duration_ms }`、`PrivacyRestored { tags, unresolved }`、`PrivacyResidual { kinds, action }`、`PrivacyDetectorDegraded { reason }` — 只記 counts/kinds，永不記原文或代稱↔原文對應。事件由 sub-agent decorator 發出。**附帶收穫**：PR #5 的 `/agent/stream` 稽核比舊 `run_agent_turn` 薄（無 `ToolCalled`/`ToolResult`），這些 privacy 事件正好補上「工具/出境」層的可觀測性。

## 8. 實作順序（每階段獨立可編譯、可回退；1–3 不碰既有路徑）

1. **Phase 1 規則核心 + config 骨架**：checksum/detect/tags/store(in-memory)/engine mask+restore+殘留掃描、privacy.toml、`[runtime.privacy]` 解析、registry `build_privacy`、單元測試。**`mask()` 不得有內部長度假設，加 100K+ 字元效能測試**（合約情境未來走獨立入口，regex + ~70 chunks Ollama 併發 2 的延遲要先量）。
2. **Phase 2 加密持久化**：crypto.rs（HKDF）、SqliteMappingStore（transaction 原子 upsert）、新依賴、`PRIVACY_MASTER_KEY` boot 檢查、TTL 任務、重啟/parity/並發測試。
3. **Phase 3 Ollama hybrid**：ollama.rs（loopback 檢查）、HybridDetector、chunking、降級策略、fake detector 測試。
4. **Phase 4 pipeline 接線（對齊 PR #5）**：`PrivacyLlm`/`PrivacyTool` decorators（`src/agent/privacy.rs`）、`StreamRestorer`（`stream.rs`，由 handler 使用）；在 `build_stage_llm`/`build_stage_tools` 插入、`build_*_pipeline` 加 `privacy` 參數、`handler.rs` 各 pipeline 端點 + greeting 注入 + `insight_frames` 還原、system directive（由 `PrivacyLlm` 注入 System message）、稽核事件、SSE 整合測試；部署 config 最後才 `enabled=true`。

注意：`AppRuntime` 加欄位會破壞既有 struct-literal 建構與 handler/appstate 測試——一併更新。

**明確界定**：Phase 1–4 交付「模組 + chat/report pipeline 保護」；合約上傳端點另案，屆時直接呼叫 `PrivacyEngine.mask/restore` 公開 API。

## 9. 驗證方式（對應 qa）

- **單元**：身分證 checksum 向量（`A123456789` 有效、逐位變異無效、I/O/W/X/Y/Z 邊界、居留證格式）；統編新制含第 7 碼=7 分支；**金額 `NT$1,000,000元` 與日期 `2026年7月10日`/`20260710` 永不被遮**；**相鄰 PII 共用分隔字元** `0912345678,0987654321` 兩筆都抓；tag 穩定性（同實體同 tag、NFC 變體同 tag）；streaming split-tag 矩陣（`⟦PERSON_1⟧` 逐字元邊界切 2–4 段）+ **Error/截斷時 holdback 不丟失**；fuzzy 還原變體；`filter_args` 含引號/反斜線的 JSON 完整性；**三語向量** en（`Acme Inc.`、`Mr. John Smith`、`(02) 2345-6789` vs `+1 (415) 555-0100`）、ja（`株式会社山田製作所`、`田中太郎様`、`東京都千代田区丸の内一丁目1番1号`）、混語各 kind 正反例；SQLite 重開檔還原、TTL、並發 upsert、錯 key → 解密錯誤非亂碼。
- **整合**：`PrivacyLlm` 包 fake `LlmCapability`（記錄收到的 `LlmMessage`，斷言 PII 出境前已換 tag、System 已附 directive、殘留 PII 觸發 fail-closed）；`PrivacyTool` 包 fake `Tool`（斷言 args 的 `⟦TAG⟧` 已 restore、JSON string value 內含 `"`/`\` 仍完整）；`handler.rs::insight_frames` 對含 split-tag 的 `Finished`/`ContentDelta` 還原出原文；`FailingNerDetector` 驗兩種 `on_detector_error`；handler 級 SSE 全回合，斷言稽核 JSON 每行不含原文。
- **端到端**：啟 Ollama 後 `#[ignore]` live 測試；`enabled=false` 時行為 diff = 今日原樣；privacy 啟用 + `RUNTIME_ENABLED=false` → 拒絕開機；`/report` 路徑遮罩覆蓋測試。

## 10. 關鍵接縫（baseline：`feature/subagent-separation` @ `02cdd41`，PR #5）

| 檔案 | 用途 |
|---|---|
| [`src/agent/payload.rs`](../../../../src/agent/payload.rs) | `LlmCapability` trait（`PrivacyLlm` 包）、`Tool` trait（`PrivacyTool` 包）、`LlmMessage`/`ToolCall`/`ToolOutcome` 型別 |
| [`src/agent/wiring.rs`](../../../../src/agent/wiring.rs) | `build_stage_llm`（LLM decorator 主插入點）、`build_tool`/`build_stage_tools`（`McpTool` 包 `PrivacyTool`）、`build_insight/report/greeting_pipeline`（加 `privacy` 參數；**greeting 現直建 LLM 未走 `build_stage_llm`，需改**）|
| [`src/agent/events.rs`](../../../../src/agent/events.rs) | `EventSink`/`AgentEvent`（理解 frame 流；**還原不在此做**，因 `emit` 為 sync + lossy `try_send`）|
| [`src/server/handler.rs`](../../../../src/server/handler.rs) | pipeline builder 呼叫點（注入 `state.privacy`）、async `stream!` drain loop（`StreamRestorer` 掛載；**非** sync `insight_frames`）、各端點覆蓋 |
| [`src/server/greeting.rs`](../../../../src/server/greeting.rs) | boot-time greeting：改由 `build_stage_llm` 建 LLM + 對 `Final` batch restore |
| [`src/config.rs`](../../../../src/config.rs) | `RuntimeManifest` 加 `[runtime.privacy]`（注意 PR #5 已把 `[runtime.pipeline]`→`[runtime.input]`）|
| [`src/runtime/registry.rs`](../../../../src/runtime/registry.rs) | backend id 註冊 + `build_privacy`（mirror `build_memory`）|
| [`src/appstate.rs`](../../../../src/appstate.rs) | **`AppState`**（非 `AppRuntime`）加 `privacy`，`AppState::new` 無條件建構（`insight_grants`/`report_template` 也在 `AppState`；見 §3.4 對抗 review 更正）|
| [`config/runtime/injection.toml`](../../../../config/runtime/injection.toml) | pattern pack 模板 |

> 已淘汰的 v0.1.0 接縫：`src/runtime/orchestrator.rs`（已改名 `runtime::turn` 且非生產答案路徑）、`src/llm_connector/agent.rs`（PR #5 後非生產出境路徑）。`src/runtime/memory/store.rs` 仍是 store/trait 模式的參考範例。

## 11. 相關文件

- [Privacy Proxy 功能 PRD](./prd.md)
- [全域 reference PRD FR-015](../../prd.md)｜[reference spec §2.5/§6](../../spec/spec.md)
