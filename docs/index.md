# datacenter-agent 文件入口

本專案文件分三區，**權威只有一處**：

| 區域 | 角色 | 說明 |
|---|---|---|
| [`reference/`](./reference/index.md) | 單一事實來源（canonical） | 現況 target/current contract：PRD、Spec、QA、endpoints、modules。所有讀寫從這裡開始。 |
| [`work/`](./work/index.md) | 工作項 artifact | 單一變更的 PRD、spec、QA、實作與驗收紀錄；使用 `meta.yml` 做分類與 stage 追蹤。 |
| [`archives/`](./archives/) | 歷史 | 原始 `to-prd` 移植產出的計畫／runbook／migration log。 |
| [`agent-runtime-rust-port/`](./agent-runtime-rust-port/prd.md) | 歷史 | 原始移植 PRD/Spec/QA/架構草案（v1.3.0，2026-06-25），已由 `reference/` 取代，僅供溯源。 |

> 規則：現況一律以 `reference/` 為準；變更過程放 `work/`；`archives/` 與 `agent-runtime-rust-port/` 是歷史紀錄，不得當成目前 contract。
> 待改程式工作見 [程式修改計劃](../.agent/artifacts/plan/2026-06-29-runtime-correctness/implementation.md)；計劃狀態不代表已完成。
