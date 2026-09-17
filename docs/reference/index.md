# datacenter-agent 現況參考（Documentation Source of Truth）

> **文件類型**：documentation source of truth。PRD 定義完成後的 target state 並逐項標建置狀態；Spec、QA、endpoint 與 module 頁描述目前 worktree。  
> **Source**：[`README.md`](../../README.md)、[`Cargo.toml`](../../Cargo.toml)、[`src/main.rs`](../../src/main.rs)、[`src/appstate.rs`](../../src/appstate.rs)、[`src/server/`](../../src/server/mod.rs)、[`src/agent/`](../../src/agent/mod.rs)、[`src/runtime/`](../../src/runtime/mod.rs)  
> **對應版本**：PRD v1.4.0 · Spec v1.4.0 · QA v1.5.0（2026-09-09 快照，對應 crate 0.5.0；PRD / Spec 仍為 0.4.0 基準）
>
> **同步狀態（逐檔）**：
>
> | 檔案 | 同步基準 | 已知落差 |
> |---|---|---|
> | `prd.md` | crate 0.4.0（2026-08-20） | 無已知落差 |
> | `spec/spec.md` | crate 0.4.0（2026-08-20） | 無已知落差 |
> | `tests/qa-plan.md` | crate 0.5.0（2026-09-09 fresh run，含 live test） | §3–§9 inventory 仍為 0.4.0 基準；docker build 證據停在 0.3.x image |
> | `endpoints/**` | 2026-08-17 兩次校正（sub-agent 層＋`/v1/chat/completions`；退役端點移除） | 無已知落差 |
> | `modules/**` | 2026-08-17 校正；2026-08-20 依 §1 規則 4 縮減 | 無已知落差 |
>
> 下次落後時把該檔移回本表的落差欄，不整段警告。

## 1. 文件權威與邊界

本目錄是專案的**單一文件事實入口**，不同文件各有唯一職責：

1. [`prd.md`](./prd.md) 是**目標產品樣貌**；每條需求必須標 `已完成 / 部分完成 / 待建置 / 待決策`，不可把 status 省略。
2. Spec、QA、endpoint 與 module 頁只記錄**目前已實作行為與證據**，不把 PRD target 寫成現況。
3. 可執行程式碼、設定與測試是現況行為證據；若與 current-state reference 衝突，先校正文件。
4. module 頁只寫**落差、決策、陷阱**與跨模組 wiring 現實；「模組／檔案是什麼」的結構描述唯一擁有者是 `src/**` 的 `//!` doc comment（與程式同 diff，漂移即刻可見），module 頁不複述。endpoint 頁不適用本條——wire contract 沒有其他家，完整保留。
5. PRD 的 部分完成／待建置／待決策 差距必須由獨立的 [程式修改計劃](../../.agent/artifacts/plan/2026-06-29-runtime-correctness/implementation.md) 派生，計劃狀態不反向冒充完成狀態。
6. 移植期的歷史 PRD／Spec／TC 已自 worktree 移除，由 git 保存（取回方式見 [`docs/index.md`](../index.md#歷史git-保存不在-worktree)）。PRD／Spec／TC 三類文件的完整歸屬見 [`docs/index.md`](../index.md#prd--spec--tc-的歸屬)。

## 2. 一句話定位

`datacenter-agent` 是 Rust HTTP API 服務，透過 MCP 連接資料中心工具，使用 OpenRouter/OpenAI-compatible LLM tool-calling 回答自然語言查詢。它的**部分 config 驅動 runtime 已成為預設請求權威**；目前還不是「只換 config 就能換任意垂直應用」的完整可拔插平台。

| 項目 | 現況 |
|---|---|
| crate / 版本 | `datacenter-agent` `0.4.0` |
| HTTP / async | axum 0.8 · tokio 1 |
| MCP / LLM | rmcp 0.17 client · async-openai 0.40 · OpenRouter |
| 對外端點 | 5 條（4 條 standard + OpenAI 相容 `/v1/chat/completions`） |
| 主要編排 | [sub-agent pipeline](./modules/agent.md)（fetcher → analyst → charter/composer → finalizer/renderer） |
| prompt 入口 | `/agent/stream`、`/v1/chat/completions`，**兩者都經過 prelude**——不存在無 guardrail 入口 |
| runtime 現況 | partial；prelude（guardrails、intent、answer policy、memory、audit）已接線並覆蓋全部 prompt 入口。完整的 `run_agent_turn` 連同 `AgentPort` 仍 dormant，production 只走同步前段 `plan_stream_turn` |

## 3. 導覽

| 類型 | 文件 | 用途（含該主題的唯一 owner） |
|---|---|---|
| 目標產品樣貌 | [prd.md](./prd.md) | 完成後的需求與 AC；逐條標建置狀態；各 FR「現況」即已知缺口來源 |
| 現況技術規格 | [spec/spec.md](./spec/spec.md) | DTO、wire、資料流、狀態碼、雙路徑差異與 request flow |
| 現況測試證據 | [tests/qa-plan.md](./tests/qa-plan.md) | 實際 test inventory、來源與 coverage gaps |
| HTTP API | [endpoints/](./endpoints/index.md) | 路由、認證、limits、REST/SSE 契約 |
| 內部模組 | [modules/](./modules/index.md) | request-path wiring 與 runtime 成熟度 |
| Work items | [../work/](../work/index.md) | 變更中的 PRD/spec/QA/交付紀錄；不代表現況 contract |
| 待改程式 | [implementation.md](../../.agent/artifacts/plan/2026-06-29-runtime-correctness/implementation.md) | 未來工作；不代表目前已完成 |

> 雙路徑差異、request flow、端點契約、runtime 成熟度與已知缺口都由上述 owner 文件單獨維護；本頁只導覽、不複製，避免重複造成漂移。

## 4. 啟動與組裝（top-level 接線）

- [`src/main.rs`](../../src/main.rs)：讀 CLI/env/config、連 MCP、建立 AppState/Router、啟動 server。
- [`src/appstate.rs`](../../src/appstate.rs)：持有 MCP handle、tools、LLM defaults、`PromptBank`、
  auth token、greetings、`InsightGrants`、boot 載入的 `report_template`，與 optional `AppRuntime`。
- [`src/config.rs`](../../src/config.rs)：解析 top-level config 與相對檔案路徑；含
  `[insight.grants]` → `InsightGrants`（改 tool grant 不需重新編譯）。
- `AppState::new` 先解析 `RUNTIME_ENABLED`；明確 `false/0` 時跳過 runtime config/build，其他值（含未設）必須有 `[runtime]` 並組裝 `AppRuntime`，缺失則 fail-fast。

## 5. 維護規則

- 每個 reference 頁都要有 `Source`，且指向實際檔案或 symbol。
- Reference 頁只放可重用的 target/current contract；單一變更的過程、PRD/spec/QA 交接放 `docs/work/<slug>/`。
- 數值契約必須標明適用路徑，禁止只寫「prompt cap 2000」或「所有請求 120s」。
- 單元測試只證明局部行為；沒有 production call path 時必須標 `dormant`。
- 程式修改 PR 應同時更新受影響的 reference 與 QA coverage；計劃完成前不可先把未落地行為寫成現況。
