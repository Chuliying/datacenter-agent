# datacenter-agent 文件入口

本專案文件分下列區域，**現況權威只有一處**：

| 區域 | 角色 | 說明 |
|---|---|---|
| [`reference/`](./reference/index.md) | 單一事實來源（canonical） | 現況 target/current contract：PRD、Spec、QA、endpoints、modules。所有讀寫從這裡開始。 |
| [`work/`](./work/index.md) | 工作項 artifact | 單一變更的 PRD、spec、QA、實作與驗收紀錄；`meta.yml` 管 work / delivery status 與 stage，分類看 [`work/index.md`](./work/index.md) 的表格。 |
| `archives/` | 歷史（未版控） | 原始 `to-prd` 移植產出的計畫／runbook／migration log。`.gitignore` 排除本目錄，只存在於本機 worktree，乾淨 clone 沒有它。 |
| [`agent-runtime-rust-port/`](./agent-runtime-rust-port/prd.md) | 歷史 | 原始移植 PRD/Spec/QA/架構草案（v1.3.0，2026-06-25），已由 `reference/` 取代，僅供溯源。 |
| [`../.spec/contract/`](../.spec/contract/sub_agent/Contract.md) | 型別契約（現行） | sub-agent、payload、tool 三份契約，被 [`src/agent/mod.rs`](../src/agent/mod.rs) 等模組的 doc comment 直接引用，屬**現行**約束而非歷史；`.spec/plan/` 是寫在契約落地前的計畫，僅供溯源。 |

> 規則：現況一律以 `reference/` 為準；變更過程放 `work/`；`archives/` 與 `agent-runtime-rust-port/` 是歷史紀錄，不得當成目前 contract。`.spec/contract/` 是例外——它是程式碼引用的現行型別契約，改它等於改契約。
> 待改程式工作見 [程式修改計劃](../.agent/artifacts/plan/2026-06-29-runtime-correctness/implementation.md)；計劃狀態不代表已完成。
