# Implement Report — reference-docs-040-sync

**日期**：2026-08-20
**Execution mode**：refactor（純文件，無 runtime 行為變更；`cargo check` 驗證 doc-comment 變更不影響編譯）
**PRD**：[prd.md](./prd.md) v1.1.0

## 交付內容

### FR-001~004：三份文件同步到 crate 0.4.0

- `docs/reference/spec/spec.md` → v1.4.0：路由表收斂為 5 條＋per-group auth/timeout；outer
  fallback 統一 404 與 `Router::merge` 成因（FR-002）；`AgentResponse` 與 legacy serving path
  移除；§3 改寫（validation error 已是 pre-stream HTTP status）；§4.2 記 `run_agent_turn`
  dormant、channel bounded（8192）；§10 換 2026-08-20 fresh 快照。
- `docs/reference/tests/qa-plan.md` → v1.4.0：快照更新（`cargo test` **214 passed / 0 failed /
  3 ignored**、clippy 0、pipeline eval 3/0、replay smoke 2/0）；新增 §4.7 route-level TC-R01~R05；
  AC-001/AC-010 verdict 重判；TC-CT01 更新、TC-CT02/TC-U03/TC-U04/TC-U05 標已刪除（附原因）；
  `orchestrator::` 前綴改 `turn::`、TC-I02/TC-C01 修正改名後的 fn 引用。
- `docs/reference/prd.md` → v1.4.0：只動狀態標記（AC-004 驗證：diff 全部落在版本標頭、版本
  歷史、現況/現況缺口行與原則表狀態欄）。實質重判：FR-001/FR-002 的「SSE 200+error frame」
  缺口已修；FR-008 待建置 → 部分完成（bounded channel、JoinError 觀察）；FR-011/FR-012 記
  fallback 修正與 rollback 語意；NFR-002/NFR-006 更新。
- `docs/reference/index.md`：⚠ 整段警告改為逐檔同步狀態表（FR-004）。

### FR-005：module 頁縮減（2026-08-20 拍板：modules 縮、endpoints 留）

- `agent.md` 187 → 52 行：子模組表、核心抽象、pipeline 表、grant 段刪除（每項都在對應
  `//!` 驗證有同義事實：payload/engine/tools/events/clock/pipeline/chart/report/wiring）；
  保留落差（AgentPort seam、ToolRegistry dormant、有損事件、未外送事件）與決策/陷阱
  （async-openai 0.40、config grant、模板固定）。
- `server.md`：**ERR-003 生效案例**——`src/server/mod.rs` 的 `//!` 只有一行，表格事實不可
  直接刪。處置：把結構描述搬進其正確擁有者（充實 `src/server/mod.rs` `//!`，並修正
  `auth.rs` `//!` 漏記 401 變體的過時敘述；`cargo check` 通過），頁面表格再縮成 bullets。
  另修 0.4.0 過時的「standard 8 條」→ 4 條。
- `runtime-input.md` / `runtime-guardrails.md` / `llm-connector.md`：子檔案表併入 bullets
  （runtime 子模組 `//!` 為單行，事實依 ERR-003 就地保留、只改格式）；guardrails 頁修掉
  「SSE 外部 status 是 200 + error frame」的 0.4.0 過時敘述。
- `runtime-audit/registry/memory/eval/schema/error/llm-normalizer/turn/mcp-client`：已是目標
  形狀（落差／決策為主），未動或未需動。
- modules 合計 839 → 695 行。縮幅小於預估，原因是 ERR-003 正確擋下了「//! 沒有同義事實」
  的刪除——這是規則按設計運作，不是未完成。

## AC 驗證

| AC | 結果 | 證據 |
|---|---|---|
| AC-001 | PASS | spec §2.1 含統一 404、與 Authorization 無關、`Router::merge` 成因；§2.1/§7 保留 418 描述 |
| AC-002 | PASS | 47 個 live fn 引用逐一 `grep -rql "fn <name>" src tests` 全命中（初查抓到 4 個 stale 引用已修：TC-U03/U04/U05 標刪、TC-I02/TC-C01 改名） |
| AC-003 | PASS | index 逐檔表與三份標頭同為 v1.4.0／crate 0.4.0；落差欄只剩 docker image 證據一項 |
| AC-004 | PASS | `git diff docs/reference/prd.md` 全部 13 條刪除行皆為標頭/狀態行，需求本文零改動 |
| AC-005 | PASS | `grep -ln "\| 檔案 \| 職責" docs/reference/modules/*.md` 清零；AgentPort/redact_secrets/NFKC/ToolRegistry dormant 四黃金存活；endpoints diff 為空 |

## Fresh 驗證（2026-08-20）

`cargo fmt --check` ✓、`cargo clippy --all-targets --all-features -- -D warnings` ✓、
`cargo test` 214/0/3 ✓、`eval --pipeline-only` 3/0 ✓、replay smoke 2/0 ✓、
`bash scripts/check-doc-links.sh` PASS（63 檔）、`work-items.sh check` ✓、`scan-secrets.sh` PASS。
