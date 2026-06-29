# datacenter-agent 現況參考（Documentation Source of Truth）

> **文件類型**：documentation source of truth。PRD 定義完成後的 target state 並逐項標建置狀態；Spec、QA、endpoint 與 module 頁描述目前 worktree。  
> **Source**：[`README.md`](../../README.md)、[`Cargo.toml`](../../Cargo.toml)、[`src/main.rs`](../../src/main.rs)、[`src/appstate.rs`](../../src/appstate.rs)、[`src/server/`](../../src/server/mod.rs)、[`src/runtime/`](../../src/runtime/mod.rs)  
> **對應版本**：PRD v1.3.0 · Spec v1.2.0 · QA v1.2.0（2026-06-29）

## 1. 文件權威與邊界

本目錄是專案的**單一文件事實入口**，不同文件各有唯一職責：

1. [`prd.md`](./prd.md) 是**目標產品樣貌**；每條需求必須標 `已完成 / 部分完成 / 待建置 / 待決策`，不可把 status 省略。
2. Spec、QA、endpoint 與 module 頁只記錄**目前已實作行為與證據**，不把 PRD target 寫成現況。
3. 可執行程式碼、設定與測試是現況行為證據；若與 current-state reference 衝突，先校正文件。
4. PRD 的 部分完成／待建置／待決策 差距必須由獨立的 [程式修改計劃](../../.agent/artifacts/plan/2026-06-29-runtime-correctness/implementation.md) 派生，計劃狀態不反向冒充完成狀態。
5. `docs/agent-runtime-rust-port/**` 與 `docs/archives/**` 是歷史移植需求、設計與計畫資料，不是目前 target/current contract。

## 2. 一句話定位

`datacenter-agent` 是 Rust HTTP API 服務，透過 MCP 連接資料中心工具，使用 OpenRouter/OpenAI-compatible LLM tool-calling 回答自然語言查詢。它同時包含一套**預設關閉、部分 config 驅動的 runtime seam**；目前還不是「只換 config 就能換任意垂直應用」的完整可拔插平台。

| 項目 | 現況 |
|---|---|
| crate / 版本 | `datacenter-agent` `0.1.1` |
| HTTP / async | axum 0.8.9 · tokio 1.52.3 |
| MCP / LLM | rmcp 0.17.0 client · async-openai 0.40.3 · OpenRouter |
| 預設請求路徑 | legacy `llm_connector`；`RUNTIME_ENABLED` 未設或非 `true`/`1` 時使用 |
| runtime 現況 | partial；orchestrator、memory、audit、answer policy 已接線，stage dispatch、injection、evaluator 等仍不完整 |

## 3. 導覽

| 類型 | 文件 | 用途（含該主題的唯一 owner） |
|---|---|---|
| 目標產品樣貌 | [prd.md](./prd.md) | 完成後的需求與 AC；逐條標建置狀態；各 FR「現況」即已知缺口來源 |
| 現況技術規格 | [spec/spec.md](./spec/spec.md) | DTO、wire、資料流、狀態碼、雙路徑差異與 request flow |
| 現況測試證據 | [tests/qa-plan.md](./tests/qa-plan.md) | 實際 test inventory、來源與 coverage gaps |
| HTTP API | [endpoints/](./endpoints/index.md) | 路由、認證、limits、REST/SSE 契約 |
| 內部模組 | [modules/](./modules/index.md) | request-path wiring 與 runtime 成熟度 |
| 待改程式 | [implementation.md](../../.agent/artifacts/plan/2026-06-29-runtime-correctness/implementation.md) | 未來工作；不代表目前已完成 |

> 雙路徑差異、request flow、端點契約、runtime 成熟度與已知缺口都由上述 owner 文件單獨維護；本頁只導覽、不複製，避免重複造成漂移。

## 4. 啟動與組裝（top-level 接線）

- [`src/main.rs`](../../src/main.rs)：讀 CLI/env/config、連 MCP、建立 AppState/Router、啟動 server。
- [`src/appstate.rs`](../../src/appstate.rs)：持有 MCP handle、tools、LLM defaults、prompts、auth token、greetings 與 optional `AppRuntime`。
- [`src/config.rs`](../../src/config.rs)：解析 top-level config 與相對檔案路徑。
- `AppState::new` 在 top-level config 有 runtime refs 時會先 `build_runtime`；`RUNTIME_ENABLED` 只決定建好的 runtime 是否在 handler 被選用。

## 5. 維護規則

- 每個 reference 頁都要有 `Source`，且指向實際檔案或 symbol。
- 數值契約必須標明適用路徑，禁止只寫「prompt cap 2000」或「所有請求 120s」。
- 單元測試只證明局部行為；沒有 production call path 時必須標 `dormant`。
- 程式修改 PR 應同時更新受影響的 reference 與 QA coverage；計劃完成前不可先把未落地行為寫成現況。
