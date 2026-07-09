# 模組功能（Modules）總覽

> ← 回 [專案主體](../index.md)
>
> **Source**：[`src/lib.rs`](../../../src/lib.rs)（crate 根）、[`src/runtime/mod.rs`](../../../src/runtime/mod.rs)（runtime 樹）

## Crate 結構

```
src/
├── main.rs            # 進入點（見 專案主體）
├── lib.rs             # crate 根
├── appstate.rs        # 程序級共享狀態（見 專案主體）
├── config.rs          # 頂層 config.toml 載入（見 專案主體）
├── model.rs           # 根層資料模型
├── bin/
│   └── eval.rs        # 第二個 binary target：eval CLI（見 eval）
├── server/            # → server 模組
├── runtime/           # → runtime 各子模組（領域無關核心）
├── llm_connector/     # → llm_connector 模組
└── mcp_client.rs      # → mcp_client 模組
```

## 模組索引（每模組一頁）

### HTTP 層
| 模組 | 職責 | 子頁 |
|---|---|---|
| `server` | axum 路由、middleware、auth、handler、DTO、錯誤、greeting 任務 | [server](./server.md) |

### Runtime 核心（[`src/runtime/`](../../../src/runtime/mod.rs)，與 HTTP 解耦）
| 模組 | 職責 | 子頁 |
|---|---|---|
| `turn` | `run_agent_turn`：一個 turn 的編排骨幹 | [turn](./runtime-turn.md) |
| `input` | 決定性輸入 pipeline（normalize/intent/slots） | [input](./runtime-input.md) |
| `llm_normalizer` | 可選 LLM-backed 輸入正規化 seam | [llm_normalizer](./runtime-llm-normalizer.md) |
| `guardrails` | input guard + injection detector + config-driven answer policy | [guardrails](./runtime-guardrails.md) |
| `memory` | partial：in-memory session store + context；actor 未接線 | [memory](./runtime-memory.md) |
| `audit` | partial：事件/sink；production redaction/actor 未接線 | [audit](./runtime-audit.md) |
| `registry` | partial：部分 config ID → trait object；多組 ID 只驗證 | [registry](./runtime-registry.md) |
| `config` | 能力包 config 載入 + validate | [config](./runtime-config.md) |
| `schema` | 共享 runtime 型別 | [schema](./runtime-schema.md) |
| `error` | runtime 錯誤模型 | [error](./runtime-error.md) |
| `eval` | partial：pipeline/replay/live runner；process gate 已接線，evaluator semantics 仍有缺口 | [eval](./runtime-eval.md) |

### 外部連接
| 模組 | 職責 | 子頁 |
|---|---|---|
| `llm_connector` | OpenRouter LLM + MCP tool-calling 迴圈 | [llm_connector](./llm-connector.md) |
| `mcp_client` | datacenter MCP server 的 rmcp client | [mcp_client](./mcp-client.md) |

## 依賴流向（高層）

```
server ──呼叫──▶ runtime::turn ◀── AppState 組裝部分 registry components ◀── config
   │                     │
   │                     ├─▶ input ─▶ llm_normalizer(可選)
   │                     ├─▶ guardrails
   │                     ├─▶ memory
   │                     ├─▶ audit
   │                     └─▶ llm_connector ─▶ mcp_client
   └──────────────────────────────────────────▶ llm_connector（runtime 關閉時直呼）
```

> runtime 核心**不依賴** axum / server DTO。trait seams 已存在，但「有 trait」不代表所有 config module 已可拔插；成熟度以各 module page 的 production wiring 為準。
