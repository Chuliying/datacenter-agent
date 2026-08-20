# datacenter-agent 文件入口

本專案文件分下列區域，**現況權威只有一處**：

| 區域 | 角色 | 說明 |
|---|---|---|
| [`reference/`](./reference/index.md) | 單一事實來源（canonical） | 現況 target/current contract：PRD、Spec、QA、endpoints、modules。所有讀寫從這裡開始。 |
| [`work/`](./work/index.md) | 工作項 artifact | 單一變更的 PRD、spec、QA、實作與驗收紀錄；`meta.yml` 管 work / delivery status 與 stage，分類看 [`work/index.md`](./work/index.md) 的表格。 |
| `archives/` | 歷史（未版控） | 原始 `to-prd` 移植產出的計畫／runbook／migration log。`.gitignore` 排除本目錄，只存在於本機 worktree，乾淨 clone 沒有它。 |
| [`agent-runtime-rust-port/`](./agent-runtime-rust-port/prd.md) | 歷史 | 原始移植的 PRD／Spec／TC／架構草案（2026-06-25 ~ 06-30），已由 `reference/` 取代。每一份檔頭都有狀態標語指向對應的現況檔；僅供溯源。 |
| [`../.spec/contract/`](../.spec/contract/sub_agent/Contract.md) | 型別契約（現行） | sub-agent、payload、tool 三份契約，被 [`src/agent/mod.rs`](../src/agent/mod.rs) 等模組的 doc comment 直接引用，屬**現行**約束而非歷史；`.spec/plan/` 是寫在契約落地前的計畫，僅供溯源。 |

> 規則：現況一律以 `reference/` 為準；變更過程放 `work/`；`archives/` 與 `agent-runtime-rust-port/` 是歷史紀錄，不得當成目前 contract。`.spec/contract/` 是例外——它是程式碼引用的現行型別契約，改它等於改契約。
> 待改程式工作見 [程式修改計劃](../.agent/artifacts/plan/2026-06-29-runtime-correctness/implementation.md)；計劃狀態不代表已完成。

## PRD / Spec / TC 的歸屬

同一類文件有三種角色，各有唯一位置。找文件先看這張表，不要憑檔名猜。

| 類別 | 現況權威（canonical） | 單一變更的過程紀錄 | 歷史（僅供溯源） |
|---|---|---|---|
| PRD | [`reference/prd.md`](./reference/prd.md) | `work/<slug>/prd.md` | [`agent-runtime-rust-port/prd.md`](./agent-runtime-rust-port/prd.md) |
| Spec | [`reference/spec/spec.md`](./reference/spec/spec.md) | `work/<slug>/spec.md` | [`agent-runtime-rust-port/spec/`](./agent-runtime-rust-port/spec/spec-overview.md)（`spec-overview` + `spec-01`~`spec-06`）、[`runtime-architecture-spec.md`](./agent-runtime-rust-port/runtime-architecture-spec.md)、[`file-structure.md`](./agent-runtime-rust-port/file-structure.md) |
| TC（QA plan／驗收報告） | [`reference/tests/qa-plan.md`](./reference/tests/qa-plan.md) | `work/<slug>/qa-plan.md`、`work/<slug>/qa-report.md` | [`agent-runtime-rust-port/qa/`](./agent-runtime-rust-port/qa/qa-plan.md)（含 [`input-pipeline-migration-2026-06-30/`](./agent-runtime-rust-port/qa/input-pipeline-migration-2026-06-30/qa-report.md)） |
| 型別契約 | [`../.spec/contract/`](../.spec/contract/sub_agent/Contract.md) | — | [`../.spec/plan/sub_agent.md`](../.spec/plan/sub_agent.md) |
| 執行計劃 | — | `work/<slug>/plan/plan.md`（canonical-v2） | [`../.agent/artifacts/plan/2026-06-29-runtime-correctness/`](../.agent/artifacts/plan/2026-06-29-runtime-correctness/implementation.md)（retired 三件式，狀態仍 active） |

規則：

1. 新的 PRD／Spec／TC 一律開在 `work/<slug>/`，形狀依
   [`ARTIFACTS.md`](../.agent/skills/_shared/ARTIFACTS.md)；不再寫進 `.agent/artifacts/`。
2. 每份歷史檔的檔頭都要有狀態標語，指向對應的現況檔。沒有標語的歷史檔就是缺漏，補上再用。
3. work item 交付後，長期事實升級到 `reference/`，`work/<slug>/` 只留該次變更的過程與證據。
4. `reference/` 內部分工不變：`prd.md` 是 target state 並逐項標建置狀態，
   `spec/` 與 `tests/` 只寫已實作行為與現有證據。
