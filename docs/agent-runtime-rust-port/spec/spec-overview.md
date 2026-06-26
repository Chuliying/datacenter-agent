# Agent Runtime Rust 移植 — 技術規格（總覽 / 索引）

**Story ID**: S-RUNTIME-01 ・ **Spec 版本**: v1.3.0 ・ **對應 PRD**: `docs/agent-runtime-rust-port/prd.md` @ v1.3.0

> 本規格已依分期拆檔，避免單檔過大。本檔為**薄索引 + 全域內容**；各層細節（型別/契約/步驟/測試）見對應分檔。

## 分檔索引

| 檔 | 分期 | 內容 |
|----|------|------|
| `spec-01-config-registry.md` | P1 | config 載入 / `Registry` 拔插 / `schema` / `RuntimeError` |
| `spec-02-input.md` | P2 | L5 input：normalizer / intent / slots / pipeline |
| `spec-03-guardrails.md` | P3 | L6：injection / input_guard / answer_policy + 拒絕/提示契約 |
| `spec-04-audit-memory.md` | P4 | L14 audit（trait + 完整事件）+ L12 memory（store + context）|
| `spec-05-orchestrator.md` | P5 | orchestrator / AgentPort / DTO / 串流契約 / 一輪 turn 資料流 / 接線 |
| `spec-06-eval.md` | P6 | 模型/skill eval：`Evaluator` trait / fixtures / baseline / `bin/eval` |
| `../file-structure.md` | 全期 | 嚴謹檔案結構、模組責任邊界、測試落點 |

---

## 版本歷史

| 版本 | 修改時間 | 修改內容（摘要） | 影響範圍 | 對應 PRD 版本 | 作者 |
|------|---------|----------------|---------|-------------|------|
| v1.0.0 | 2026-06-25 | 初版：portable-core（L5/L6/L12/L14）TS→Rust 移植 + config 驅動 | 全 runtime | PRD v1.0.0 | AI |
| v1.1.0 | 2026-06-25 | 依嚴格 review：registry 拔插 / AuditSink trait + 完整事件 / `RuntimeError` / 完整 classifier / orchestrator↔AgentPort 契約 / 移植缺陷修正 / Rust 獨有測試 | 全 runtime | PRD v1.1.0 | AI |
| v1.2.0 | 2026-06-25 | 新增 eval 子系統；並依分期拆檔（overview + 6 檔）| runtime + 新 bin + 文件結構 | PRD v1.2.0 | AI |
| v1.3.0 | 2026-06-26 | thin-proxy 移植觸發的契約同步：`intent.resolved` 升為 `/agent/stream` 正式 wire event（runtime；附加、向後相容；拒絕路徑不送）；`AgentRequest` 加 `session_id`/`option_id`、`AgentResponse` 加 `intent` | dto / handler / orchestrator + falcon thin-proxy | PRD v1.3.0 | AI |

---

## ⚠️ Gate 0 前置檢查結果（manifest 缺口揭露）

| 檢查項目 | 結果 |
|---------|------|
| Project Manifest（`.agent/project-manifest.md`）| ❌ 不存在 — repo 尚未 onboard 到 shared skills |
| Guardrails（`.agent/guardrails.md`）| ❌ 不存在 |
| System Context / Architecture Map | ❌ 無 manifest 指向；改以**實際讀過的程式碼**為依據（非臆測）|
| API Reference | N/A — 本案不接外部 API，契約即本 repo `dto.rs` + ported TS schema |

**決策（依 spec skill Step 0 選項 c）**：標記 `Architecture Map Missing` risk 並繼續。所有型別/行為依據均來自實際讀取的原始碼，不靠 manifest 推斷。manifest/guardrails 正式 onboarding 列為後續任務（Flow I）。

> 通用平台架構另見 `docs/agent-runtime-rust-port/runtime-architecture-spec.md`（領域無關骨架；本規格為其第一個實例）。

---

## 1. 變更檔案清單（全域）

### runtime 核心
| 路徑 | 操作 | 分期 |
|------|------|------|
| `src/runtime/mod.rs` | NEW | P1 |
| `src/runtime/config.rs` | NEW | P1 |
| `src/runtime/registry.rs` | NEW | P1 |
| `src/runtime/error.rs` | NEW | P1 |
| `src/runtime/schema.rs` | NEW | P1 |
| `src/runtime/input/{mod,normalizer,intent,slots,pipeline}.rs` | NEW | P2 |
| `src/runtime/guardrails/{mod,injection,input_guard,answer_policy}.rs` | NEW | P3 |
| `src/runtime/memory/{mod,store,context}.rs` | NEW | P4 |
| `src/runtime/audit.rs` | NEW | P4 |
| `src/runtime/llm_normalizer.rs` | NEW | P5 |
| `src/runtime/orchestrator.rs` | NEW | P5 |
| `src/runtime/eval/{mod,evaluator,fixtures,baseline,report,runner}.rs` | NEW | P6 |
| `src/bin/eval.rs` | NEW | P6 |

### 接線 / host（MODIFY）
| 路徑 | 操作 | 分期 |
|------|------|------|
| `src/config.rs` | MODIFY（`Manifest` 擴 `[runtime]` + 組裝段）| P1 |
| `src/lib.rs` | MODIFY（`pub mod runtime;`）| P1 |
| `Cargo.toml` | MODIFY（`uuid`/`sha2`/`unicode-normalization`/`regex`/`async-trait`/`thiserror`）| P1 |
| `src/appstate.rs` | MODIFY（加 `runtime`/`sessions?`/`audit`/`input_pipeline`/`answer_policy`/`llm_normalizer?`）| P5 |
| `src/server/dto.rs` | MODIFY（`AgentRequest` 加 `session_id` / `option_id`）| P5 |
| `src/server/handler.rs` | MODIFY（兩 route 改走 orchestrator）| P5 |

### 能力包 config（NEW）
| 路徑 | 分期 |
|------|------|
| `config/config.toml`（MODIFY：`[runtime]` + 組裝段）| P1 |
| `config/runtime/{intents,lexicon,thresholds,injection}.toml` | P1 |
| `config/runtime/evals/{inputs.json,response-baseline.json}` | P6 |

---

## 5. 實作步驟（分期總表）

| 分期 | 內容 | 依賴 | 估時 | 細節 |
|------|------|------|------|------|
| P1 | config + registry + schema + error | — | 5h | `spec-01` |
| P2 | L5 input pipeline | P1 | 4h | `spec-02` |
| P3 | L6 guardrails | P1,P2 | 3h | `spec-03` |
| P4 | L14 audit（完整）+ L12 memory | P1,P2 | 5h | `spec-04` |
| P5 | orchestrator + 接線 | P1–P4 | 6h | `spec-05` |
| P6 | eval 子系統 | P1–P5 | 5h | `spec-06` |
| | **總計** | | **28h** | |

**風險緩衝**：+20% ≈ **34h**

---

## 6. 技術決策（全域，8 條）

1. **模組化移植 + config 驅動領域 + registry 驅動模組組裝**：config 只外部化資料不夠（review BLOCKER）；加 registry 才能「config 選模組路徑」。輸入 pipeline、answer policy、LLM normalizer、memory、audit、eval 分別組裝，orchestrator 只依賴 trait。
2. **intent 用 String 非 enum + 開機驗證**：能力包可定義自己的 taxonomy；`validate()` 防 silent drift。
3. **拒絕/disclaimer 不加新 wire 事件；200/400 分流**：refusal 當 token、disclaimer 當開頭 token；語意拒絕 200、結構性拒絕 400。零破壞現有 client。
4. **memory 用 server session store（trait + registry）**：由 `[runtime.memory] enabled/backend` 選；disabled 時建成 `None`，server-memory 模式 upstream `history:[]`、記憶折進 prompt（對齊 TS、避免雙重注入）。
5. **audit 完整且可插拔**：`AuditSink` trait + StdoutAuditSink，10 種事件涵蓋每決策點（含被擋請求、tool、clear），帶 seq；寫入失敗由 `failure_policy` 控制，hash-chain/tamper 留待持久化 sink。
6. **錯誤策略**：`thiserror` `RuntimeError`；config 失敗中止開機、per-request 映射 `AppError`；`runtime/` request path 禁 `unwrap/expect`。
7. **移植缺陷處理（非 verbatim）**：asset 判定改 config allowlist；normalizer 連全形標點表移植；regex `/i`→`(?i)`、檢視 `\b`/anchor；`Date.now/randomUUID/sha256` → `SystemTime/uuid/sha2`。
8. **eval 為獨立第二驗證軸（可拔插）**：`Evaluator` trait 分 pipeline evaluator（離線 CI 必跑）與 response evaluator（baseline / LLM-judge，live 或 replay 選跑），fixtures 隨能力包、baseline 比對、`bin/eval` CI gate；經 Registry 註冊。對齊 falcon-client I12。

---

## 10. 相關參考

### in-repo Rust 慣例
| 檔案 | 參考重點 |
|------|---------|
| `src/llm_connector/agent.rs` | `#[cfg(test)]`、stream + `LlmEvent`、`Clear`(L376)→`generate` 清 buffer(L452)、tool 呼叫(L392-414) |
| `src/config.rs` | TOML manifest + file-ref + `deny_unknown_fields`(L90-97) |
| `src/appstate.rs` | `Arc` 共享 + `#[derive(Clone)]`(L111)、env 載入、`generation_config`(L176) |
| `src/server/handler.rs` | `prepare_config` 插入點(L193-208)、empty/2000 cap |

### 移植來源（TS）
| 檔案 | 對應 Rust | 備註 |
|------|----------|------|
| `schemas.ts` | `runtime/schema.rs` | intent enum→String |
| `input-normalizer.ts` | `runtime/input/normalizer.rs` | NFKC **+ 全形標點表** |
| `lexicon.ts`/`constants.ts` | `config/runtime/*.toml` | 含完整 COS_CLASSIFIER |
| `slot-extractor.ts` | `runtime/input/slots.rs` | asset 硬編→**config allowlist** |
| `injection-patterns.ts` | `runtime/guardrails/injection.rs` | `/i`→`(?i)` |
| `answer-policy.ts` | `runtime/guardrails/answer_policy.rs` | |
| `session-memory.ts`/`memory-context.ts` | `runtime/memory/*` | |
| `audit-log.ts` | `runtime/audit.rs` | secret 名重指 Rust 端 + 新增事件/seq |
| `run-agent-turn.ts` | `runtime/orchestrator.rs` | history:[]、Clear 映射、intent.resolved 首幀升為 `/agent/stream` wire event（runtime）|
| `evals/chief-of-staff-inputs.json`（I12）| `config/runtime/evals/*` + `runtime/eval/*` | |

### 文件
| 文件 | 連結 |
|------|------|
| PRD | `docs/agent-runtime-rust-port/prd.md` |
| 通用平台架構 | `docs/agent-runtime-rust-port/runtime-architecture-spec.md` |
| 能力分層地圖 | `falcon-client/docs/plans/chief-of-staff-agent-runtime/capability-layer-map.md` |

---

## Gate 2 自檢清單（全域）

- [x] 每個能力（L5/L6/L12/L14）+ 模組拔插 + 完整 audit + eval 都有對應檔案變更
- [x] 每個分期都有估時與依賴
- [x] 每個技術決策都有選擇原因（8 條）
- [x] Interface 定義完整（分散於各分檔；含 registry/AuditSink/RuntimeError/LlmInputNormalizer/Evaluator trait）
- [x] 有契約範例（wire frame + 完整 config）
- [x] 有資料流 / 轉換邏輯（含 audit 事件落點）
- [x] 有邊界條件（空/超長/injection/session-mismatch/clear/abort/未知模組 id）
- [x] 有具體測試案例（移植 + Rust 獨有 + eval）
- [x] 參考既有 in-repo 實作（≥4）
- [x] 版本歷史已填 v1.2.0
- [ ] ⚠️ Environment：manifest `environment_rules` 不存在（Gate 0 risk，已揭露）
- [ ] ⚠️ Tech Research：本輪未跑 `search_web`（純內部移植，行為以 TS 來源為 SSOT；移植缺陷已逐條標非 verbatim）
