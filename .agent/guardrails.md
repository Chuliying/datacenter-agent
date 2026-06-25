# datacenter-agent — Guardrails

## 這個專案是什麼

`datacenter-agent` 是一個 **Rust HTTP API 服務**，不是前端應用。它是一個 analytics agent，透過 MCP 協議連接資料中心的工具，再透過 LLM 回答自然語言查詢。

## 程式碼語言與生態系

- **只使用 Rust**。不要引入 JavaScript、TypeScript、Python 或其他語言的程式碼到核心邏輯
- **Cargo 管理依賴**，`Cargo.toml` 是唯一的依賴設定
- **async/await with tokio**，不要使用 blocking I/O

## 安全邊界

- **絕對不要**把 `GLOBAL_TOKEN`、`OPENROUTER_API_KEY` 或任何 secret 寫進程式碼
- 所有 secret 只能從環境變數載入（`dotenvy`）
- 不要 log 任何 secret 或 token 內容

## 架構邊界

- **AppState 是唯一的共享狀態**，所有 handler 都透過 `axum::extract::State<Arc<AppState>>` 存取
- 不要在 handler 之間用 global mutable state（static Mutex 等），除非有明確需要
- MCP client 在啟動時連線一次，`mcp_client.handle()` 回傳的 handle 可以 clone 跨 task 使用
- `PromptBank` 在啟動後是 immutable 的，不要設計 runtime 重載 prompt 的機制（除非明確需求）

## 錯誤處理

- 使用 `anyhow::Result` 處理啟動流程的錯誤
- handler 層使用 `server::error` 模組中定義的錯誤型別，並 impl `IntoResponse`
- 不要使用 `unwrap()` 在 production path，啟動時的 expect 可以接受（附說明）

## 設定與 Prompt 管理

- 所有設定透過 `config/config.toml` 管理，路徑相對於 config 檔案本身
- Prompt 是 Markdown 檔案，透過 config 對應到 id，不要 hardcode prompt 字串在程式碼裡
- 新增 prompt 時，先在 `config/config.toml` 加入對應，再建立 Markdown 檔案

## 測試

- 整合測試放在 `tests/` 目錄（Cargo 慣例）
- 單元測試放在對應模組的 `#[cfg(test)]` 區塊內
- 測試指令：`cargo test`（詳見 manifest Stack）

## 程式碼風格

- 使用 `rustfmt` 格式化（`cargo fmt`）
- 使用 `clippy` 做 lint（`cargo clippy -- -D warnings`）
- 每個公開的 struct / fn 都要有 doc comment（`///`）
- 模組層級用 `//!` 寫 module-level doc

## 不要做的事

- 不要把業務邏輯塞進 `main.rs`，只做啟動流程
- 不要在 `server/handler.rs` 裡直接呼叫 LLM 或 MCP，透過 `AppState` 拿到的 handle 操作
- 不要修改 `config/config.toml` 的版本號（`version = 1`）格式，這是向後相容的鍵
- 不要改變 probe 路由（`/health`, `/ready`）的回應格式，這些可能被 k8s 或監控系統依賴

## Container 相容性

- `config/` 目錄設計為可獨立掛載的 volume
- 使用 `--config` 參數指定 config 路徑，不要假設固定路徑
- 所有路徑都應該接受相對（相對於 config）或絕對路徑
