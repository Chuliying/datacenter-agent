# docs/reference 同步到 crate 0.4.0 現況 PRD

| Field | Value |
|-------|-------|
| Story ID | S-DOCS-01 |
| Version | v1.1.0 |
| Status | Ready |
| Sprint | N/A |
| has_ui | false |
| Tickets | N/A |

---

## 1. Follow-ups

無 Blocking FU。Non-blocking FU: N/A。

本頁只記錄「文件與程式碼的落差」，不決定程式行為；任何需要改變 runtime 行為的判斷
（例如 bearer 失敗要不要從 418 遷到 401）不在本 work item，維持 `docs/reference/prd.md`
既有的「待決策」標記。

---

## 2. Context

### Goal

`docs/reference/` 是本專案唯一的現況權威，但它的 `prd.md`、`spec/spec.md`、
`tests/qa-plan.md` 三份仍停在 v1.3.0（2026-06-30）的版本標頭，`endpoints/` 與
`modules/` 已於 2026-08-17 校正兩次而它們沒有跟上。0.4.0 的 router fallback 安全修正
（commit `18c1d5c`）與隨之新增的 router-level 測試都還沒被寫進 Spec 與 QA 頁，讀者
會從權威文件得到「沒有 router-level 測試」這個已經不成立的結論。本 work item 把三份
文件重新對齊到 crate 0.4.0 的可觀察行為，並讓 `docs/reference/index.md` 的同步狀態
註記逐檔陳述而非整段警告。

### Risk and evidence

| Item | Trigger / Source | Mitigation / Decision |
|------|------------------|-----------------------|
| 同步時把 PRD 的 target state 改寫成現況，讓 PRD 失去「未完成」的標記 | 三份文件角色不同，一起改時的混用成本最高 | 沿用 `docs/reference/index.md` §1 的分工：`prd.md` 只標建置狀態，Spec/QA 只寫已實作行為 |
| 引用測試作為證據，後續重構改名後證據失效 | QA 頁以測試名當 evidence | 只引測試函式名 + 檔案路徑，不引行號（沿用 `docs/reference/**` 現行慣例） |
| 三份文件共 872 行，一次改寫時只改標頭、不動內文即可看起來完成 | 版本標頭與內文分離 | AC 逐份要求可 grep 的具體事實，不接受只改版本號 |
| module 頁縮減時把落差／決策／陷阱誤當可推導內容刪掉 | 縮減與同步同一輪進行，黃金內容混在可推導表格附近（例：agent.md 子模組表格內嵌「ToolRegistry dormant」） | 每條刪除必須能在對應 `//!` doc comment 找到同義事實才可刪；找不到就地保留並歸類為落差／決策／陷阱（見 ERR-003） |
| Evidence | `docs/reference/index.md` 的 ⚠ 同步狀態段；`docs/reference/spec/spec.md:49`（只記 418，未記 outer fallback）；`docs/reference/tests/qa-plan.md:154`（`TC-CT01 auth 418 讀碼 / 沒有 HTTP characterization test`）；`src/server/route.rs`（`retired_paths_return_404_regardless_of_authorization`、`unmatched_paths_do_not_leak_token_validity`、`surviving_paths_are_still_routed`、`openai_timeout_returns_openai_error_envelope`、`per_group_timeout_layers_survive_a_merge`）；`src/test_support.rs` | 以 worktree 程式碼為現況證據，衝突時改文件 |

---

## 3. Scope

### In scope

- `docs/reference/prd.md`：AC 狀態欄位依 0.4.0 證據重判，版本標頭與「對應現況 Spec/QA」連結更新。
- `docs/reference/spec/spec.md`：記入 0.4.0 的 outer `fallback` 統一 404 行為，以及 `Router::merge` 會帶走 sub-router fallback 這個成因。
- `docs/reference/tests/qa-plan.md`：把 evidence 欄位從「讀碼」改指 0.4.0 新增的 router-level 測試，並更新剩餘 gap 清單。
- `docs/reference/index.md`：⚠ 同步狀態改為逐檔陳述，同步完成的檔案從警告中移除。
- `docs/reference/modules/**`：縮到「程式說不出口的那一半」——每頁只留**落差**（宣告未接線、dormant）、
  **決策**（為何如此設計）、**陷阱**（移植注意），以及跨模組 wiring 現實；「子檔案／子模組職責表」與
  型別重講刪除，改一行指向對應 `src/**/mod.rs` 的 `//!` doc comment（結構的唯一擁有者）。
  （2026-08-20 使用者拍板：modules 縮、endpoints 留。）

### Out of scope

- 改變任何 runtime 行為，包含 418 → 401 的遷移決策。
- 歷史移植文件（已自 worktree 移除，git 保存）與本機 `docs/archives/`（未版控）——兩者都不是現況權威，不在同步範圍。
- `docs/reference/endpoints/**`：**原樣保留**。endpoint 頁是 wire contract（request/response 形狀、status code、timeout）唯一的家，rustdoc 與 OpenAPI 都不涵蓋；縮它等於刪掉外部消費者的 API reference。只在與三份文件衝突時順手修正。
- 補測試。QA 頁只記錄現有 evidence 與 gap，不在本 work item 寫 Rust 測試。

---

## 4. Flow

Flow: N/A，因為這是文件同步工作，沒有使用者互動流程；驗證面是三份文件與 worktree 的一致性。

---

## 5. Functional Requirements (FR)

### FR-001: 三份文件的版本標頭反映實際同步基準

**使用者價值**: 讀者能一眼判斷手上的文件對應哪一版程式碼，不需要自己比對 git log。

**Behavior**: `prd.md`、`spec/spec.md`、`tests/qa-plan.md` 的版本標頭改為對應 crate 0.4.0 與同步日期，並在版本歷史區記一列本次同步。

**Data source**: Existing `Cargo.toml`（crate 版本）、`CHANGELOG.md`（0.4.0 條目）

**Permissions / Visibility**: 無存取控制，文件公開於 repo

**Boundary conditions**:

- 只有內文確實同步的檔案才改標頭；未同步的檔案保留舊標頭並留在 `index.md` 的警告清單。

### FR-002: Spec 記入 0.4.0 的 unmatched-path 行為

**使用者價值**: Spec 是現況行為權威，缺了 fallback 行為會讓實作者以為未匹配路徑的回應仍依 `Authorization` 而定。

**Behavior**: `spec/spec.md` 的 route/middleware 段記入：外層 `fallback` 對所有未匹配路徑回統一 404、與 auth layer 無關；並記下成因是 `Router::merge` 會把 sub-router 的 fallback 一起帶走。既有的 418 描述維持，因為那是有匹配路由時的 auth 行為。

**Data source**: Existing `src/server/route.rs`（`build_router`、`unmatched_path`）

**Permissions / Visibility**: 同上

**Boundary conditions**:

- 不把 404 描述成 auth 行為；它是路由層行為，與 bearer 有效性無關。

### FR-003: QA 頁的 evidence 欄位指向實際存在的 router-level 測試

**使用者價值**: QA 頁被用來判斷哪裡還缺測試；把已經補上的測試留在「沒有測試」欄位會導致重複補測或錯誤的風險評估。

**Behavior**: 把 `TC-CT01` 一類「讀碼 only」的 evidence 改為 `src/server/route.rs` 中對應的測試函式名，並重列剩餘 gap（哪些 AC 仍只有讀碼證據）。

**Data source**: Existing `src/server/route.rs`、`src/test_support.rs`

**Permissions / Visibility**: 同上

**Boundary conditions**:

- 測試只覆蓋部分斷言時記為 partial 並寫出缺哪一項，不記為完成。
- 只引函式名與檔案路徑，不引行號。

### FR-004: `index.md` 的同步狀態逐檔陳述

**使用者價值**: 現在的整段 ⚠ 警告讓四份已校正的文件也被讀成不可信。

**Behavior**: `docs/reference/index.md` 的同步狀態段改為逐檔列出「同步基準 + 已知落差」，同步完成的檔案不再出現在落後清單。

**Data source**: Existing `docs/reference/**`

**Permissions / Visibility**: 同上

**Boundary conditions**:

- 清單為空時明寫「無已知落差」，不刪整段——下次落後時要有地方寫。

### FR-005: module 頁只承載程式說不出口的內容

**使用者價值**: 「模組是什麼」在 `//!` doc comment 與 module 頁各有一份時，改 code 的人只會同步前者（spec-05 的 orchestrator→turn 改名即前例）；把可推導內容的唯一擁有者定為 doc comment 後，module 頁剩下的每一行都是必須人腦維護、也值得人腦維護的內容。

**Behavior**: `docs/reference/modules/*.md`（`index.md` 除外，它是地圖）逐頁改寫：刪除子檔案／子模組職責表與型別結構重講，改為一行指向對應 `src/**/mod.rs`；保留並前置「Production reality／落差／關鍵點／陷阱」段。`agent.md`、`server.md` 為主要縮減對象；`runtime-audit.md` 的形狀（已實作五行 + Production reality 六行）是目標範本。

**Data source**: Existing `src/**/mod.rs` 與各檔 `//!` doc comment（刪除前的比對基準）

**Permissions / Visibility**: 同上

**Boundary conditions**:

- 只刪「對應 `//!` comment 已有同義事實」的內容；doc comment 沒有的事實不刪，就地歸類。
- 內嵌在表格裡的落差註記（如 dormant 標記）先抽出保留，再刪表格。
- `endpoints/**` 完全不動（見 Out of scope）。
- `docs/reference/index.md` §1 的分工規則（結構歸 rustdoc、落差歸 module 頁）與本 FR 一致。

---

## 7. Error Scenarios (ERR)

### ERR-001: 同步時把 PRD 的 target state 覆寫成現況

**Trigger**: 三份文件一起改寫，把 `prd.md` 裡「待建置」的需求依現況降級成已完成或直接刪除。

**Expected behavior**: `prd.md` 只允許改動每條需求的**狀態標記**與版本標頭，需求本文與 target 敘述不動。

**Recovery**: 以 `git diff docs/reference/prd.md` 檢查是否有需求本文被改寫；有就還原該段，只保留狀態標記變更。

### ERR-002: 引用的測試名稱與 worktree 不一致

**Trigger**: 同步時憑記憶或憑舊 QA 頁寫測試名，實際 worktree 已改名或不存在。

**Expected behavior**: QA 頁引用的每個測試名都能在 worktree 中找到。

**Recovery**: 對每個引用執行 `grep -rn "<test_name>" src tests`，找不到就改回 gap 敘述。

### ERR-003: 縮減時黃金內容被連帶刪除

**Trigger**: 刪除子模組表格或型別重講時，混在其中的落差／決策／陷阱敘述（dormant 標記、wiring 現實、移植注意）一併消失。

**Expected behavior**: 每條被刪的敘述都能在對應 `//!` doc comment 指出同義事實；指不出來的敘述不刪。

**Recovery**: 以 `git diff docs/reference/modules/` 逐條檢視刪除行，對每行執行「`//!` 有沒有這件事」的比對；沒有就還原該行並歸入落差／決策／陷阱段。

---

## 8. Acceptance Criteria (AC)

### AC-001: Spec 描述 unmatched-path 的統一 404

```gherkin
Given docs/reference/spec/spec.md 已完成本次同步
When 讀者在該檔搜尋未匹配路徑的行為描述
Then 找得到「外層 fallback 回統一 404、與 Authorization 無關」與其成因（Router::merge 帶走 sub-router fallback）
And 既有「有匹配路由時 bearer 失敗回 418」的描述仍在
```

### AC-002: QA 頁的 evidence 都能在 worktree 找到

```gherkin
Given docs/reference/tests/qa-plan.md 已完成本次同步
When 對該檔引用的每個 Rust 測試函式名執行 grep -rn "<name>" src tests
Then 每一個都至少命中一次
And 原本記為「沒有 HTTP characterization test」的項目改為指向 src/server/route.rs 中的實際測試或明確保留為 gap
```

### AC-003: 版本標頭與同步狀態一致

```gherkin
Given docs/reference/index.md 已完成本次同步
When 讀者比對 index.md 的逐檔同步狀態與三份文件各自的版本標頭
Then 兩邊對同一份文件的同步基準一致
And 未同步的文件仍出現在 index.md 的落差清單裡
```

### AC-004: PRD 的需求本文未被改寫

```gherkin
Given docs/reference/prd.md 已完成本次同步
When 執行 git diff 檢視該檔變更
Then 變更只出現在版本標頭、版本歷史與每條需求的狀態標記
And 沒有任何需求敘述或 AC 條文本身被刪除或改寫
```

### AC-005: module 頁縮減後黃金內容仍在、可推導內容已交還

```gherkin
Given docs/reference/modules/*.md 已完成本次縮減
When 在 modules/ 目錄搜尋子檔案／子模組職責表（「| 檔案 | 職責 |」表頭）
Then 除 index.md 外找不到任何一個
And grep 仍找得到已知黃金事實：AgentPort（agent.md 的 wiring 落差）、redact_secrets（runtime-audit.md 的未呼叫落差）、NFKC（runtime-input.md 的手工對照表陷阱）、ToolRegistry dormant（自表格抽出後保留）
And 每頁含一行指向對應 src/**/mod.rs 的結構指針
And docs/reference/endpoints/**（chat-completions.md 的 mapping 規則與「三個不同」表）無任何刪除
```

---

## 9. UI / UX

UI: N/A (has_ui=false)

---

## 10. Dependencies & Constraints

- **Upstream**: crate 0.4.0 的 worktree 與 `CHANGELOG.md`；`docs/reference/endpoints/index.md`（2026-08-17 已校正，作為端點現況基準）。
- **Downstream**: 任何以 `docs/reference/` 為輸入的 `spec` 與 `qa` 工作；`.agent/project-manifest.md` 的 `api_reference` 指向本目錄。
- **Breaking change**: No（純文件）。
- **Assumptions**: `docs/reference/endpoints/**` 與 `modules/**` 於 2026-08-17 的校正結果正確，本次以它們為端點與模組現況的基準。

---

## 12. Gate 1 Check

- [x] Every FR has user value, data source, permissions, and boundary conditions.
- [x] Every AC uses Given-When-Then and has an executable precondition.
- [x] ERR covers the main failure and recovery path.
- [x] Scope, dependencies, breaking change, and assumptions are explicit.
- [x] Blocking FU is closed; non-blocking FU has owner and close-by point.
- [ ] NFR has measurable target or N/A + reason.（core tier，未填 NFR）
- [x] UI evidence matches `has_ui`.
- [x] `scripts/check-prd.py` result is PASS.
