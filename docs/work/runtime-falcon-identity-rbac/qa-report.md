# Falcon 使用者身份與 RBAC 驗收報告

**PRD**: `prd.md` v2.0.0 ｜ **Spec**: `spec.md` v1.5.0 ｜ **QA plan**: `qa-plan.md`
**受測 commit**: `27ae390`（分支 `codex/runtime-falcon-identity-rbac`，1 ahead / 0 behind `origin/main`）
**判定**: **PASS**（第二輪，補件後）— 首輪為 CONDITIONAL FAIL，三個阻擋項已全部處理

---

## 1. Pre-flight Checklist

- [x] Type check: `cargo check` → 0 errors
- [x] Lint: `cargo clippy --all-targets -- -D warnings` → clean
- [x] Format: `cargo fmt --check` → clean
- [x] Coding rules: 未發現違反 `.agent/guardrails.md`（secret 只走 env、不 log token、AppState 為唯一共享狀態、probe 回應格式未動）

## 2. 測試執行

### Unit / Integration

- 命令：`cargo test`
- 結果：**318 passed, 0 failed**（14 個 test binary）

### E2E

`N/A (has_e2e=false)` — manifest 未宣告 `e2e_cmd`。

## 3. AC 逐條核對

「行為覆蓋」指有測試實際驗證該行為，與測試檔是否寫上 AC 編號無關。

| AC | 行為覆蓋 | 證據 |
|---|---|---|
| AC-001 權限來自 Falcon 不擴權 | ✅ | `documented_response_parses_role_objects_and_filters_to_readable_permissions`、`http_client_uses_documented_permissions_path_and_bearer_transport` |
| AC-002 token 不進 log／audit／儲存 | ✅ | cache（`positive_cache_reuses_a_lookup_and_never_stores_the_clear_token`）、audit（`audit_writer_assigns_monotonic_seq_and_redacts_actor`）、tracing（**新增** `ac002_user_token_never_reaches_the_tracing_stream`：成功與失敗兩條路徑都送出後，斷言 token 不在 process-wide 捕捉到的 log 內）|
| AC-003 cache 命中不打 Falcon | ✅ | `positive_cache_reuses_a_lookup_and_never_stores_the_clear_token` |
| AC-004 actor_key 穩定 opaque | ✅ | `derives_the_pinned_actor_key_for_a_known_input`（已知答案） |
| AC-005 session memory 以 actor_key 隔離 | ✅ | `memory_scope_uses_actor_key_so_same_session_cannot_cross_users`、`key_isolates_by_actor` |
| AC-006 無權限 intent 在 LLM／MCP 前被拒 | ✅ | `finance_permission_cannot_authorize_a_member_intent`、`authorization_refusal_exposes_its_machine_code_on_openai_completion` |
| AC-007 收窄只減不增 | ✅ | `wildcard_is_expanded_before_intersection_and_output_grants_are_preserved` |
| AC-008 事業部父層不隱含子頁 | ✅ | **新增** `ac008_business_unit_parent_permission_grants_no_tools`（四個 intent 逐一驗）與 `ac008_another_business_unit_permission_grants_no_tools` |
| AC-009 report 降級並聲明缺漏 | ✅ | `authorized_report_crosses_runtime_pipeline_with_terminal_degradation_notice` |
| AC-010 身份失敗 code 互異 | ⚠️ 部分 | `identity_middleware_distinguishes_missing_unauthorized_and_unavailable` 與 `identity_layer_only_covers_prompt_routes_and_returns_distinct_codes` 驗的是**三種**；401 已拆成 refreshable／terminal 後成為**四種**，middleware 層沒有驗到這一拆（wire 層有：`wire_401_with_token_invalid_is_refreshable`、`wire_401_with_another_code_is_terminal`）|
| AC-011 Falcon 不可用 fail-closed | ✅ | `identity_middleware_distinguishes_...unavailable`、`http_client_maps_request_timeout_to_unavailable`、`malformed_success_is_negative_cached_as_unavailable` |
| AC-012 per-actor 互不影響 | ✅ | `per_actor_buckets_are_independent_and_lru_bounded` |
| AC-013 audit 帶 actor 不帶敏感欄位 | ✅ | `per_actor_rejection_audits_the_actor_without_leaking_network_metadata`、`audit_record_carries_actor_key_as_an_independent_opaque_field` |
| AC-014 pepper 缺失即啟動失敗 | ✅ | `rejects_a_missing_pepper_without_echoing_a_value`（另含空值與過短兩例） |
| AC-015 補 header 向前相容 | N/A | 設計上是**部署前一次性檢查**，對象為當前 main 的 binary；D3 不設 flag，交付後無法重建該前提。見下方待辦 |
| AC-016 兩條路徑行為一致 | ⚠️ 部分 | `identity_uses_openai_error_envelope_for_v1_paths` 只驗信封形狀；沒有同一權限情境在兩條路徑上比對結果的測試 |
| AC-018 外層全域上限仍生效 | ⚠️ 部分 | `ac008_burst_rejection_happens_before_the_handler` 屬**前一刀**的全域 limiter；本刀新增的 `per_actor_limiter_runs_after_identity_on_the_real_router` 驗的是內層與 layer 順序。沒有測試在**雙層同時啟用**下驗證外層仍會先擋 |
| AC-020 不可續期 401 的 cache 重放 | ✅ | `unauthorized_cache_replays_the_same_failure_from_one_upstream_call` |
| AC-021 limiter 未啟用即拒絕啟動 | ✅ | **新增** `ac021_identity_layer_requires_both_rate_limit_layers`。開機檢查已抽成純函式 `require_rate_limit_for_identity`，可在不啟動 MCP／LLM／runtime 的情況下驗證三個分支 |
| AC-022 判定以 intent 所需 tool 為基準 | ✅ | `finance_permission_cannot_authorize_a_member_intent`、`strict_intent_requires_all_required_tools_not_just_one` |
| AC-023 權限撤銷後舊脈絡不進 LLM | ✅ | `authorized_memory_context_drops_revoked_topics_and_audits_the_count` |
| AC-024 client history 被忽略 | ✅ | `memory_enabled_injects_context_and_clears_upstream_history` |
| AC-025 可續期 401 的 cache 重放 | ⚠️ 部分 | 分類本身有 wire 層測試，但沒有測試驗證**可續期**那一類在負向 cache 命中時重放同一 code |
| AC-026 OpenAI 只採最後一則 user message | ✅ | `discards_all_prior_turns_even_when_they_are_well_formed`、`keeps_only_the_last_user_turn_when_users_are_adjacent`、`discards_earlier_turns_under_the_single_turn_contract` |

**統計（補件後）**：完整 19 ／ 部分 4 ／ 無測試 0 ／ 設計上 N/A 1 = 24

## 4. AC 追溯性檢查（qa-plan Step 4）

**首輪 FAIL（0/24）→ 補件後 23/24。**

`grep -rhoE "AC-[0-9]{3}"` 在 `src/` 與 `tests/` 共命中 14 個編號，但逐一比對來源後，**全部屬於前一個 work item**（`runtime-user-session-rate-limit`）：

| 出現位置 | 所屬 |
|---|---|
| `src/runtime/store/{mod,month,sanitize,sqlite}.rs`、`tests/runtime_store_sqlite.rs` | 前一刀的 SQLite ledger |
| `src/server/rate_limit.rs` 的 `ac008_`／`ac009_`／`ac010_`／`ac014_`／`ac015_` | 前一刀的全域 burst limiter |
| `src/server/route.rs`、`tests/route_contract.rs` | 前一刀 |

本刀的四個新模組（`falcon.rs`、`identity.rs`、`authz.rs`、`actor.rs`）當時**沒有任何 AC 標記**。編號相同並不代表覆蓋——前一刀的 `AC-014` 是「未認證流量不能耗盡 bucket」，本刀的 AC-014 是「pepper 缺失即啟動失敗」，兩者無關。

**補件**：改用 `S-RUNTIME-SEC-02 AC-xxx` 這種帶 story id 的標記，與前一刀的裸編號不會混淆。目前 23/24 已標記，唯一未標的 AC-015 設計上就沒有測試（部署前一次性檢查）。驗證指令：

```bash
grep -rhoE "S-RUNTIME-SEC-02 [A-Z0-9, -]+" --include='*.rs' src | grep -oE "AC-[0-9]{3}" | sort -u
```

## 5. 未執行項目

| 項目 | 原因 |
|---|---|
| TC-M01 降級報告的數字洩漏人工抽驗 | 需要真實 LLM 產出降級報告；本次未執行 |
| TC-D01 AC-015 部署前檢查 | 需要部署當前 main 的 binary 並實際送一個帶 header 的請求；本次未執行 |
| 對 `http://10.2.67.50:18081` 的實機驗證 | 網路不可達（ping 100% 遺失，無該網段路由）|

## 6. 首輪阻擋項與處置

| # | 首輪阻擋項 | 處置 |
|---|---|---|
| 1 | AC-008 與 AC-021 完全沒有測試（兩者都是 fail-closed 行為，回歸時的症狀是**默默放行**而非報錯）| 已補三條測試；`require_rate_limit_for_identity` 抽成純函式以便測試 |
| 2 | AC 追溯標記缺失 | 已補 23/24，採 `S-RUNTIME-SEC-02 AC-xxx` 形式避免與前一刀混淆 |
| 3 | AC-002 的 tracing 半邊沒被驗證 | 已補 `ac002_user_token_never_reaches_the_tracing_stream` |

**仍為部分覆蓋（不阻擋）**：AC-010（middleware 層未驗 refreshable／terminal 拆分，wire 層已驗）、AC-016（只驗信封形狀）、AC-018（未在雙層同時啟用下驗外層）、AC-025（未驗可續期類的 cache 重放）。建議在後續 slice 補齊。

## 7. 品質閘門總結

| Gate | 指令 | 結果 |
|---|---|---|
| type-check | `cargo check` | PASS |
| lint | `cargo clippy --all-targets -- -D warnings` | PASS |
| format | `cargo fmt --check` | PASS |
| test | `cargo test` | PASS（318／0）|
| PRD 形狀 | `check-prd.py --tier team` | PASS |
| AC traceability（文件層）| `check-traceability.py` | PASS（24／24 對到 TC）|
| Selected Seam | `check-selected-seam.py` | PASS（SEAM-001）|
| 文件連結 | `lint-docs.sh` | PASS（77 檔）|
| work item metadata | `work-items.sh check` | PASS（6 valid）|
| UI mockup | — | N/A (has_ui=false) |

文件層的 traceability gate 是 PASS，因為它比對的是「PRD 的 AC 有沒有對到 qa-plan 的 TC」；**程式碼層的 AC 標記缺失它抓不到**，這正是第 4 節單獨列出的原因。

---

## 8. Security Preflight

```text
Skill: security
Executed At: 2026-08-31 10:30
Change Scope: origin/main...HEAD（commit 7e0dc61 + 一項本節產生的修正），
              1,881 行動到 auth/authz surface（auth.rs, identity.rs, falcon.rs,
              actor.rs, authz.rs, route.rs, codes.rs）

Machine evidence:
- Secret preflight: FINDINGS → 已修 → CLEAR
  Evidence: `bash .claude/skills/security/scripts/scan-secrets.sh`
            targets=src config, extensions=rs toml json md, 85 files, exit 0
            結果 PASS，但輸出 `WARN: 1 suppressed secret candidate(s)`。
            不盲信該抑制，另以 `git diff origin/main...HEAD | grep -Ei ...` 獨立掃描，
            找到 scanner 抑制掉的那一筆確實該報：
            `scripts/dev-falcon-stub.md:134` 曾提交一個字面 pepper
            （`export ACTOR_KEY_PEPPER='local-only-...'`）。
            該檔自己第 154 行寫著「Do not paste the output into the repository」，
            與第 134 行自相矛盾。已改為 `export ACTOR_KEY_PEPPER="$(openssl rand -hex 32)"`。
            嚴重度：非生產憑證外洩，但那是可被直接複製到真實部署的值；
            pepper 一旦已知，每個 `actor_key` 都能由 `user_id` 反推，
            正好廢掉假名化這唯一的設計性質。

- Dependency audit: SKIP
  Evidence: `git diff --name-only origin/main...HEAD` 顯示 `Cargo.toml` 與
            `Cargo.lock` **均未變更**，本 commit 未引入或升級任何依賴；
            manifest 亦未宣告 `dependency_audit_cmd`，且 `cargo-audit` 未安裝。
            依 skill 規定不自動安裝工具，記錄 SKIP 與原因。

Recorded/manual evidence:
- Auth review: RECORDED
  Reviewer: Claude Opus 5（AI 審查，非人工簽核；下述為 scoped attestation）
  Diff scope: `git diff origin/main...HEAD -- src/server/{auth,identity,falcon,actor,authz,route,codes}.rs`

  | 檢查項 | 結果 |
  |---|---|
  | authority 由可信 server-side boundary 建立 | ✅ 身份不由 client 宣告：runtime 拿 BFF 轉送的 token 自行向 Falcon 驗證，`actor_key` 由 server 端 pepper 推導，client 無法指定 |
  | default-deny | ✅ `authz.rs:96-102`：`required_tools` 為空即 `allowed = false`；非 report intent 要求 `omitted_tools.is_empty()`；report 要求交集非空。三條路徑都是先證明允許才放行，不存在「預設允許再排除」 |
  | 拒絕路徑確實中止 | ✅ 兩個 handler（`handler.rs:325`、`:829`）在 `!decision.allowed` 時都走拒答並寫 audit，不進 pipeline |
  | token 傳遞 | ✅ 僅以 `bearer_auth(token)` 進 header（`falcon.rs:288`）；URL 由 `self.endpoint` 固定組成，token 不進 URL、不進 query |
  | token 過期／撤銷處理 | ✅ 401 依 `error_code` 白名單分類；僅 `auth.token_invalid` 可續期，未知與缺漏皆終端。此設計使未知碼無法誘發無限 refresh |
  | 錯誤訊息不洩漏 | ✅ 五個 `identity_error` 訊息皆為固定字串，不含 token 或使用者資料 |
  | log 不洩漏 | ✅ 新增測試 `ac002_user_token_never_reaches_the_tracing_stream` 以 process-wide 捕捉驗證 |
  | 新增端點的授權政策 | ✅ **未新增任何端點**（`git diff` 對 `route.rs` 無新增 `.route(`）；既有兩條 prompt 路由新增身份層，probe 三條不受影響 |
  | client-exposed env | N/A (has_ui=false) |

- Operation guard: SKIP
  Evidence: 本次未執行任何高風險操作（無刪除、無 history rewrite、無 push）。

Findings:
1. 已修：`scripts/dev-falcon-stub.md` 提交字面 pepper（見上）。
2. 既有殘留（不阻擋，來自審查建議）：`config/config.toml` 的 `[identity].base_url`
   預設指向 dev host。可由 `FALCON_API_BASE_URL` 覆寫，但 production binary 的
   預設值指向一個生產環境不該信任的來源。建議改為無預設值並強制部署時指定。
3. 既有殘留（不阻擋）：`[runtime.llm_normalizer]` 若被啟用，normalizer 會在授權判定
   **之前**呼叫 LLM（`turn.rs:290-294` 早於 `handler.rs` 的 `authorize_pipeline`）。
   出貨設定為停用，故「授權先於任何 LLM 呼叫」目前成立，但那是條件式而非結構性保證。

Unverified scopes:
- Git history 是否曾提交過真實憑證（scanner 明載不涵蓋 history）。
- 依賴漏洞（未執行 audit，理由如上）。
- 生產部署組態、secret manager 狀態、網路邊界（MCP server 是否對外暴露）。
- 上述 auth review 是 AI 產出的 scoped attestation，**不是人工資安簽核**。
