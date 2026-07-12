# Privacy Proxy 技術規格

**Spec 版本**：v0.1.0
**對應 Feature PRD**：[`./prd.md`](./prd.md) v0.1.0（S-PRIVACY-01）
**對應全域**：[reference PRD](../../prd.md) FR-015、[reference spec](../../spec/spec.md) §2.5/§6
**狀態**：Target-state design（待建置）
**Source**：接縫對照 `src/runtime/orchestrator.rs`、`src/config.rs`、`src/runtime/registry.rs`、`src/server/handler.rs`、`src/llm_connector/agent.rs`（見 §10）；設計已經獨立 subagent 對照 codebase review（verdict: approve-with-changes）

> 本 spec 描述**尚未實作**的目標設計，內容 self-contained。所有型別/檔案為規劃；接線點以 commit `813b275` 為基準。實作時以此為契約，落地後轉為 current-state。

## 版本歷史

| 版本 | 日期 | 內容 |
|---|---|---|
| v0.1.0 | 2026-07-11 | 初版；模組設計 + 對照 codebase review（2 critical + 6 major 已併入）落為技術 spec |

## 1. 架構總覽

```text
prompt/合約文字 ─► PrivacyEngine.mask(rules + 選配 Ollama NER) ─► 殘留掃描 ─► 雲端 LLM
                        │ tag↔原文 upsert                            │ ⟦TAG⟧ 鎖定指令
                   SQLite(AES-256-GCM) ◄── resolve ── StreamRestorer.restore(容錯)
                                                            │
                                                            ▼ SSE 還原後原文出給前端
tool 呼叫：args 先 restore（本地工具收到真名）、result 先 mask（DB 資料出境前遮罩）
```

偵測全程本機。停用時（`[runtime.privacy].enabled=false` 或缺 section）不建構任何元件。

## 2. 模組佈局 — 新增 `src/runtime/privacy/`

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
| `agent_port.rs` | `PrivacyAgentPort<P: AgentPort>` decorator + `PrivacyToolFilter` |

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
- `src/appstate.rs`：`AppRuntime` 加 `pub privacy: Option<Arc<PrivacyEngine>>`，`build_runtime_for_flag` 呼叫 `build_privacy`。
- `src/main.rs`：`if let Some(p) = &runtime.privacy { p.clone().spawn_ttl_cleanup(); }`。

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

## 7. Pipeline 整合 — `AgentPort` decorator（非 input stage）

理由：input stages 跑在 memory augmentation **之前**（`apply_memory_context` 於 orchestrator 後段才改 prompt），stage 會漏掉 memory 帶回的 PII；只有 decorator 能同時包住回應 frame stream 做 streaming 還原；input pipeline 是同步設計，mask 需要 async store + Ollama。

### 7.1 掛載點

1. **`PrivacyAgentPort<P: AgentPort>`**（包 `LlmAgentPort`，`orchestrator.rs` `AgentPort` trait）：`stream_turn` 內先 mask `input.prompt` + history 各項（`user_prompt`/`model_response`），再把 inner frame stream 接上 `StreamRestorer`（Token → push、Clear → reset、**任何 stream 結束（Done/Error/截斷 Aborted）都要 flush holdback**）。mask 失敗 → Err → 整回合失敗，**fail-closed 不出境**。
2. **Tool I/O**：`src/llm_connector/mod.rs` 定義 `ToolIoFilter` trait（維持依賴方向，llm_connector 不 import runtime）——`filter_args`（工具參數先 restore：雲端問 `⟦ORG_1⟧`，本地工具收到真名；**必須解析 JSON 後只在 string value 內替換再序列化**，禁止 raw string 替換以免原文含 `"`/`\` 破壞 JSON；順序：先 log(masked) 後 restore，還原後參數永不落 log）、`filter_result`（tool result 出境前 mask）。`agent_stream` 加 `Option<Arc<dyn ToolIoFilter>>` 參數，其他呼叫端傳 `None` 零改變。
3. **Handler 組裝**（`handler.rs` `LlmAgentPort` 建構點）：`runtime.privacy` 為 `Some` 時包 decorator + 掛 filter + system prompt 附加 `system_directive()`；`None` 分支 = 今日原樣程式碼。

### 7.2 出境路徑全覆蓋（review 抓出的漏洞，必修）

| 路徑 | 現況 | 對策 |
|---|---|---|
| `/report`、`/report/stream` | **繞過 runtime，直呼 `generate`/`agent_stream`**（見 [reference spec §2.5](../../spec/spec.md)）— 合約報表正是目標情境，**critical** | privacy 啟用時在 `completion_inner`/report handler 內套 `engine.mask` + 出口 restore；或未接線前直接回 503 |
| `RUNTIME_ENABLED=false` legacy path | 運維 rollback 開關**靜默退回無遮罩路徑**（fail-open，**critical**） | boot 不變量：`privacy.enabled=true` 且 runtime 停用 → 拒絕開機；加測試 |
| Greeting（`src/server/greeting.rs`）呼叫 `generate` 帶完整工具集 | MCP tool result（可能含會員個資）未過濾出境 | greeting 路徑同樣掛 `ToolIoFilter`，或啟用時 greeting 停用工具 |
| Eval binary（`src/bin/eval.rs`） | 用合成 fixtures，可接受 | 文件註明「僅限合成資料」 |
| `LlmInputNormalizer`（在 decorator 之前跑） | 今日只有 Disabled 實作 | 此接縫永遠 local-only；未來雲端 normalizer 必須經過 PrivacyEngine |
| `PRIVACY_OLLAMA_URL` override | 可被誤設為非本機 → 原文出境 | 非 loopback 即拒絕啟動（或顯式 `allow_remote_ner=true`） |

### 7.3 Streaming 還原器（`⟦TAG⟧` 可能被 token 切開）

Holdback 演算法：掃到可能開 tag 的字元即扣住，增量文法匹配（`OPEN WS? KIND (SEP INDEX)? WS? CLOSE`）；完整匹配 → resolve 還原；有效前綴未完 → 續扣；超過 32 字上限未成 tag → 吐 1 字重掃（延遲有界，合約含 `[` 不會無限緩衝）；`finish()` flush + 完整性統計。

### 7.4 殘留掃描 + fail-closed 矩陣

| 事件 | 行為 |
|---|---|
| mask 後殘留掃描（排除 tag span 重跑 RuleDetector）發現 checksum 級/高信心 PII | `residual.action`：`block`（預設，雲端呼叫前中止）/ `warn`（僅稽核） |
| 偵測錯誤（Ollama down） | `block` 或 `rules-only`（per config） |
| store 錯誤 / 解密失敗 | block |
| 還原時 unresolved tag | 照原樣輸出 tag + 稽核 `PrivacyRestoreIncomplete`（代稱本身無害，不硬失敗傷可用性） |

### 7.5 稽核（仿 `audit.rs` hash-never-log）

`PrivacyMasked { entities, kinds, degraded, duration_ms }`、`PrivacyRestored { tags, unresolved }`、`PrivacyResidual { kinds, action }`、`PrivacyDetectorDegraded { reason }` — 只記 counts/kinds，永不記原文或代稱↔原文對應。

## 8. 實作順序（每階段獨立可編譯、可回退；1–3 不碰既有路徑）

1. **Phase 1 規則核心 + config 骨架**：checksum/detect/tags/store(in-memory)/engine mask+restore+殘留掃描、privacy.toml、`[runtime.privacy]` 解析、registry `build_privacy`、單元測試。**`mask()` 不得有內部長度假設，加 100K+ 字元效能測試**（合約情境未來走獨立入口，regex + ~70 chunks Ollama 併發 2 的延遲要先量）。
2. **Phase 2 加密持久化**：crypto.rs（HKDF）、SqliteMappingStore（transaction 原子 upsert）、新依賴、`PRIVACY_MASTER_KEY` boot 檢查、TTL 任務、重啟/parity/並發測試。
3. **Phase 3 Ollama hybrid**：ollama.rs（loopback 檢查）、HybridDetector、chunking、降級策略、fake detector 測試。
4. **Phase 4 pipeline 接線**：stream.rs、agent_port.rs、ToolIoFilter（JSON-aware restore）、handler 組裝 + system directive、**`/report` 兩端點與 greeting 覆蓋、`RUNTIME_ENABLED` fail-open boot 不變量**、稽核事件、SSE 整合測試；部署 config 最後才 `enabled=true`。

注意：`AppRuntime` 加欄位會破壞既有 struct-literal 建構與 handler/appstate 測試——一併更新。

**明確界定**：Phase 1–4 交付「模組 + chat/report pipeline 保護」；合約上傳端點另案，屆時直接呼叫 `PrivacyEngine.mask/restore` 公開 API。

## 9. 驗證方式（對應 qa）

- **單元**：身分證 checksum 向量（`A123456789` 有效、逐位變異無效、I/O/W/X/Y/Z 邊界、居留證格式）；統編新制含第 7 碼=7 分支；**金額 `NT$1,000,000元` 與日期 `2026年7月10日`/`20260710` 永不被遮**；**相鄰 PII 共用分隔字元** `0912345678,0987654321` 兩筆都抓；tag 穩定性（同實體同 tag、NFC 變體同 tag）；streaming split-tag 矩陣（`⟦PERSON_1⟧` 逐字元邊界切 2–4 段）+ **Error/截斷時 holdback 不丟失**；fuzzy 還原變體；`filter_args` 含引號/反斜線的 JSON 完整性；**三語向量** en（`Acme Inc.`、`Mr. John Smith`、`(02) 2345-6789` vs `+1 (415) 555-0100`）、ja（`株式会社山田製作所`、`田中太郎様`、`東京都千代田区丸の内一丁目1番1号`）、混語各 kind 正反例；SQLite 重開檔還原、TTL、並發 upsert、錯 key → 解密錯誤非亂碼。
- **整合**：`PrivacyAgentPort` 包 `FakeAgentPort`（echo prompt 驗證 PII 出境前已換 tag；吐 split-tag token 驗證 SSE 端重組出原文）；`FailingNerDetector` 驗兩種 `on_detector_error`；handler 級 SSE 全回合，斷言稽核 JSON 每行不含原文。
- **端到端**：啟 Ollama 後 `#[ignore]` live 測試；`enabled=false` 時行為 diff = 今日原樣；privacy 啟用 + `RUNTIME_ENABLED=false` → 拒絕開機；`/report` 路徑遮罩覆蓋測試。

## 10. 關鍵接縫（commit 813b275）

| 檔案 | 用途 |
|---|---|
| [`src/runtime/orchestrator.rs`](../../../../src/runtime/orchestrator.rs) | `AgentPort` trait / `LlmAgentPort`（decorator 接縫） |
| [`src/config.rs`](../../../../src/config.rs) | `RuntimeManifest`（`[runtime.privacy]` 解析） |
| [`src/runtime/registry.rs`](../../../../src/runtime/registry.rs) | backend id 註冊 + `build_privacy` |
| [`src/server/handler.rs`](../../../../src/server/handler.rs) | `LlmAgentPort` 組裝點（包 decorator）；`/report`、greeting 覆蓋 |
| [`src/llm_connector/agent.rs`](../../../../src/llm_connector/agent.rs) | 雲端呼叫點（tool filter hook） |
| [`src/runtime/memory/store.rs`](../../../../src/runtime/memory/store.rs) | 要仿照的 trait + store 模式 |
| [`config/runtime/injection.toml`](../../../../config/runtime/injection.toml) | pattern pack 模板 |

## 11. 相關文件

- [Privacy Proxy 功能 PRD](./prd.md)
- [全域 reference PRD FR-015](../../prd.md)｜[reference spec §2.5/§6](../../spec/spec.md)
