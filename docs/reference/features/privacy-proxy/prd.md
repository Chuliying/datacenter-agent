# Privacy Proxy（本地去識別化 / 逆向還原）功能 PRD

**Story ID**：S-PRIVACY-01
**版本**：v0.1.0
**狀態**：Target-state feature source of truth（待建置）
**對應全域 PRD**：[reference PRD](../../prd.md) FR-015 / NFR-010
**對應 Spec**：[privacy-proxy spec](./spec.md)
**Source**：需求來自本頁；技術設計見 [spec](./spec.md)，接縫對照 `src/runtime/orchestrator.rs`、`src/server/handler.rs`、`src/llm_connector/agent.rs`。設計已經獨立 subagent 對照 codebase review（verdict: approve-with-changes，2 critical + 6 major 已併入）

> 本 PRD 描述**完成後**的功能樣貌並逐項標建置狀態。現況：**尚未實作**，多數項目為 待建置／待決策。狀態標記定義沿用 [全域 PRD §1](../../prd.md)。

## 版本歷史

| 版本 | 日期 | 內容 |
|---|---|---|
| v0.1.0 | 2026-07-11 | 初版；由模組設計計畫 + 對照 codebase review 收斂為功能 PRD |

## 1. 背景與問題

情境：使用者上傳數份**合約**，要交給雲端 LLM（OpenRouter）做分析／出報表。合約含大量個資（人名、公司名、身分證、統編、電話、email、地址、銀行帳號）。目前 repo 的所有出境路徑（`/agent`、`/report`、tool result、greeting）都把原文直接送上雲端，沒有任何去識別化。

**絕對安全紅線**：啟用本功能後，**只有代稱 `[TAG]` 出境，原文永不離開本機**。

標準做法（Microsoft Presidio 等 GenAI 隱私模式）為 **anonymize → process → deanonymize**：出境前於本機遮罩 PII、以可逆代稱替換；雲端只見代稱；回應在本機還原。偵測採「規則抓結構化 PII + NER/LLM 抓非結構化」的混合法，並在 gateway 邊界統一強制遮罩、還原受稽核。

## 2. 完成後功能定位

一個 config 驅動、預設關閉的 **Privacy Proxy** runtime 能力包（`[runtime.privacy]`）：出境前 `mask`、回程 `restore`，對照表加密持久化。啟用時涵蓋所有出境路徑；停用時模組完全 inert、零行為改變、可獨立回滾。

流程對應設計圖 S1/S2/S3：

```text
合約/prompt ─►(S1 本地去識別化) mask: 偵測 PII → 一致代稱 ⟦TAG⟧ → 殘留掃描
           ─►(S2 雲端) 只有 masked 文本 + ⟦TAG⟧ 鎖定指令送 OpenRouter
           ─►(S3 逆向還原) restore: 對照表解密 → 還原原文（streaming 容錯）→ 出給前端
```

## 3. 範圍

**In scope**

- 去識別化模組：純文字 `mask` / `restore`（batch）＋ streaming 還原介面。
- 以 `AgentPort` decorator 掛入現有 runtime，保護 chat prompt、history、tool I/O。
- 台灣 + 多語（zh-TW / en / ja）PII 偵測；hybrid（規則 + 選配本地 Ollama NER）。
- 對照表 SQLite 加密持久化、TTL 清理、跨 session／重啟還原。
- `[runtime.privacy]` config 開關與 fail-closed 語意。

**Out of scope（本階段）**

- 文件上傳／解析端點（另案；本模組提供 `mask/restore` 公開 API 供其接線）。
- Session 級 tag 隔離（`scope="session"`）；v1 只做 `global`。
- 遮罩金額與日期（刻意保留給雲端分析）。
- 目標 Capability/Evidence 架構（見全域 PRD FR-013）；本功能與其正交，未來併入該流程。

**依賴與 breaking change**

- 新增 crate 依賴：`rusqlite`(bundled)、`aes-gcm`、`hmac`、`hkdf`。
- `AppRuntime` 新增欄位 → 影響既有 struct-literal 建構與測試（一併更新）。
- 新增 env：`PRIVACY_MASTER_KEY`（sqlite store 啟用時開機必填）。

## 4. 功能需求

### FR-P01：本地去識別化 mask — 待建置

完成樣貌：偵測 PII → 以一致代稱 `⟦KIND_N⟧` 遮罩 → 對出境文字做**殘留掃描**；偵測全程本機，永不呼叫雲端。同一實體在同一 scope 內恆得同一代稱（跨份合約指涉一致）。任何偵測／store 錯誤 fail-closed，不放行。

### FR-P02：逆向還原 restore（含 streaming）— 待建置

完成樣貌：以對照表把代稱還原成原文，支援 batch 與 token-by-token streaming（代稱可能被 token 切開，需邊界感知緩衝）；容錯 tag 變體（`[PERSON_1]`、`【PERSON_1】`、全形數字、大小寫、內部空白）；還原做完整性驗證，unresolved tag 照原樣輸出並稽核。

### FR-P03：PII 範圍與偵測法 — 待建置

遮罩：人名、公司/機構名、身分證/居留證、統編、電話、email、地址、銀行帳號/匯款資訊。**不遮金額與日期**。結構化證號用 regex + checksum（身分證加權 mod 10、統編財政部 2023 新制 mod 5 + context）；非結構化人名/公司名由規則錨定 + Ollama NER 補抓。

### FR-P04：多語（zh-TW / en / ja）— 待建置

規則按語言標記、config 選擇啟用語言；tag 與引擎語言中立。中文合約含英文公司名等混語文件不需語言偵測，所有啟用語言規則同時掃描。Ollama NER 天然多語，為非結構化實體的多語主保險。

### FR-P05：對照表持久化與加密 — 待建置／key 管理待決策

完成樣貌：SQLite 對照表，原文欄位 AES-256-GCM 加密（金鑰由 `PRIVACY_MASTER_KEY` 經 HKDF 派生 enc/mac 兩把）；dedup 用 keyed HMAC（防字典攻擊）；跨 session／重啟可還原；TTL 到期清理，過期代稱還原時列 unresolved。

### FR-P06：Config 驅動開關與可回滾 — 待建置

完成樣貌：`[runtime.privacy].enabled=false` 或缺 section 時完全不建構、不開 DB、不需 key、不連 Ollama，行為與今日逐位元組相同。`detector`（rules|hybrid）、`store`（in-memory|sqlite）、`on_detector_error`（block|rules-only）皆 config。

### FR-P07：出境路徑全覆蓋 — 待建置（含現況漏洞修補）

完成樣貌：啟用時，`/agent`、`/agent/stream`、**`/report`、`/report/stream`**、tool args/result、greeting 等所有把文字送雲端的路徑都經遮罩。特別修補現況兩個 fail-open 漏洞：

- `/report` 兩端點目前**繞過 runtime**（直呼 `llm_connector`），必須另行接線或啟用時先回 503 拒絕。
- `RUNTIME_ENABLED=false` legacy path 會靜默退回無遮罩路徑；需 boot 不變量：privacy 啟用 + runtime 停用 → 拒絕開機。

### FR-P08：去敏稽核 — 待建置

完成樣貌：mask/restore/residual/degraded 事件經既有 audit sink，只記 counts/kinds/duration，**永不記原文或代稱↔原文對應**（沿用 `hash_identifier` hash-never-log 慣例）。

## 5. 錯誤與 fail-closed 契約（ERR）

| 事件 | 行為 |
|---|---|
| 殘留掃描發現 checksum 級/高信心 PII | `residual.action`：`block`（預設，雲端呼叫前中止）/ `warn`（僅稽核） |
| 偵測錯誤（Ollama down） | `block`（整回合失敗）或 `rules-only`（降級 + 稽核 `PrivacyDetectorDegraded`），依 `on_detector_error` |
| store 錯誤 / 解密失敗 | block（fail-closed） |
| 還原時 unresolved tag | 照原樣輸出代稱 + 稽核 `PrivacyRestoreIncomplete`（代稱本身無害，不硬失敗傷可用性） |
| privacy 啟用但 runtime 停用 / key 缺失 | 拒絕開機（fail-closed at boot） |
| `PRIVACY_OLLAMA_URL` 指向非 loopback | 拒絕啟動（除非顯式 `allow_remote_ner=true`） |

## 6. 非功能需求

| ID | 目標 | 狀態 |
|---|---|---|
| NFR-P01 PII egress control | 原文 PII 不出境；只有代稱 + 殘留掃描 fail-closed（對應全域 NFR-010） | 待建置 |
| NFR-P02 可逆性正確 | 同實體同代稱、還原無誤、跨重啟一致 | 待建置 |
| NFR-P03 停用即 inert | 停用時零行為改變、可獨立回滾 | 待建置 |
| NFR-P04 效能 | `mask` 無內部長度假設；合約級（100K+ 字元）延遲可接受（regex 掃描 + Ollama chunk 併發） | 待建置 |
| NFR-P05 可用性 | 偵測降級與 unresolved 還原不無謂中斷；fail-closed 僅限真正洩漏風險 | 待建置 |
| NFR-P06 金鑰安全 | key 不落 log；HKDF 金鑰分離；HMAC 防 DB 字典攻擊 | 待建置 |

## 7. Acceptance criteria

| AC | 完成條件 | 狀態 |
|---|---|---|
| AC-P01 | 含各類 PII 的合約經 `mask` 後，殘留掃描無 checksum 級/高信心 PII；原文不出現在送雲端的 payload | 待建置 |
| AC-P02 | 代稱在 streaming（每字元邊界切割）與 batch 皆能還原；容錯變體被接受；Error/截斷時 holdback 不丟失 | 待建置 |
| AC-P03 | 身分證/統編 checksum 正確；**金額與日期永不被遮**；相鄰 PII 共用分隔字元兩筆都抓到 | 待建置 |
| AC-P04 | zh-TW / en / ja 及混語文件各 kind 正反例通過 | 待建置 |
| AC-P05 | 對照表重啟後仍可還原；錯 key → 解密錯誤（非亂碼）；TTL 過期清理；並發 upsert 同實體不失敗 | 待建置 |
| AC-P06 | `enabled=false` 時行為 diff = 今日原樣；不開 DB、不需 key | 待建置 |
| AC-P07 | `/report` 與 greeting 出境經遮罩；privacy 啟用 + `RUNTIME_ENABLED=false` 拒絕開機 | 待建置 |
| AC-P08 | 稽核 JSON 每行不含原文或代稱↔原文對應 | 待建置 |

## 8. 風險

| 風險 | 緩解 |
|---|---|
| 漏抓 PII（false negative）直接造成原文洩漏 | hybrid 偵測 + 出境前殘留掃描 fail-closed；相鄰 PII 邊界修正；三語向量測試 |
| Ollama 不穩／不可用 | `on_detector_error` 策略；rules-only 降級；`Detector` trait 可注入 fake 測試 |
| 對照表洩漏（DB 檔案外流） | 原文欄位 AEAD 加密、keyed HMAC dedup、AAD 綁 row；key 僅存 env 不落 DB |
| 誤設 `PRIVACY_OLLAMA_URL` 導致原文送外部 | 非 loopback 即拒絕啟動 |
| 過度遮罩（如「大同」誤中「大同區」）損可用性 | 重疊解析規則 span 優先、LLM 實體最小長度與子字串排除 |
| global scope 讓雲端跨 session 關聯代稱 | 文件註明 trade-off；未來提供 session scope |

## 9. Non-goals

- 不在雲端做任何 PII 偵測或還原。
- 不承諾遮罩金額/日期。
- 不在本階段做文件上傳端點或 session 級隔離。
- 不以代稱取代目標 Evidence Pack / Final LLM isolation（兩者正交，未來整合）。

## 10. 相關文件

- [全域 PRD FR-015 / NFR-010](../../prd.md)
- [Privacy Proxy 技術 spec](./spec.md)
- 業界實踐：Microsoft Presidio anonymize→process→deanonymize、OWASP LLM01、MCP Security Best Practices
