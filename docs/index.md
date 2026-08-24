# datacenter-agent 文件入口

本專案文件分下列區域，**現況權威只有一處**：

| 區域 | 角色 | 說明 |
|---|---|---|
| [`reference/`](./reference/index.md) | 單一事實來源（canonical） | 現況 target/current contract：PRD、Spec、QA、endpoints、modules。所有讀寫從這裡開始。 |
| [`work/`](./work/index.md) | 工作項 artifact | 單一變更的 PRD、spec、QA、實作與驗收紀錄；`meta.yml` 管 work / delivery status 與 stage，分類看 [`work/index.md`](./work/index.md) 的表格。 |
| [`../.spec/contract/`](../.spec/contract/sub_agent/Contract.md) | 型別契約（現行） | sub-agent、payload、tool 三份契約，被 [`src/agent/mod.rs`](../src/agent/mod.rs) 等模組的 doc comment 直接引用，屬**現行**約束而非歷史；`.spec/plan/` 是寫在契約落地前的計畫，僅供溯源。 |

> 規則：現況一律以 `reference/` 為準；變更過程放 `work/`。`.spec/contract/` 是例外——它是程式碼引用的現行型別契約，改它等於改契約。
> 待改程式工作見 [程式修改計劃](../.agent/artifacts/plan/2026-06-29-runtime-correctness/implementation.md)；計劃狀態不代表已完成。

## PRD / Spec / TC 的歸屬

同一類文件有兩種活的角色，各有唯一位置；歷史一律由 git 保存，不佔 worktree（見下節）。

| 類別 | 現況權威（canonical） | 單一變更的過程紀錄 |
|---|---|---|
| PRD | [`reference/prd.md`](./reference/prd.md) | `work/<slug>/prd.md` |
| Spec | [`reference/spec/spec.md`](./reference/spec/spec.md) | `work/<slug>/spec.md` |
| TC（QA plan／驗收報告） | [`reference/tests/qa-plan.md`](./reference/tests/qa-plan.md) | `work/<slug>/qa-plan.md`、`work/<slug>/qa-report.md` |
| 型別契約 | [`../.spec/contract/`](../.spec/contract/sub_agent/Contract.md) | — |
| 執行計劃 | — | `work/<slug>/plan/plan.md`（canonical-v2）；[`../.agent/artifacts/plan/2026-06-29-runtime-correctness/`](../.agent/artifacts/plan/2026-06-29-runtime-correctness/implementation.md) 是尚未遷移的 retired 三件式，狀態仍 active |

規則：

1. 新的 PRD／Spec／TC 一律開在 `work/<slug>/`，形狀依
   [`ARTIFACTS.md`](../.agent/skills/_shared/ARTIFACTS.md)；不再寫進 `.agent/artifacts/`。
2. work item 交付後，長期事實升級到 `reference/`，`work/<slug>/` 只留該次變更的過程與證據。
3. `reference/` 內部分工不變：`prd.md` 是 target state 並逐項標建置狀態，
   `spec/` 與 `tests/` 只寫已實作行為與現有證據。
4. 被取代的文件不留在 worktree 蓋「superseded」章——直接刪除，由 git 保存；
   本索引的「歷史」一節記下取回方式。連結完整性由
   [`scripts/check-doc-links.sh`](../scripts/check-doc-links.sh) 在 CI 把關（斷鏈與
   連到未版控路徑都 FAIL）。

## 歷史（git 保存，不在 worktree）

- **移植期 PRD／Spec／TC**（`docs/agent-runtime-rust-port/`，2026-06-25 ~ 06-30，
  含 falcon→Rust input-pipeline 遷移驗收與平台架構草案）：全部已被 `reference/`
  取代，自 worktree 移除；最後完整存在於 `817418c`。取回：
  `git show 817418c:docs/agent-runtime-rust-port/prd.md`（其餘檔案同理；
  `git ls-tree -r --name-only 817418c docs/agent-runtime-rust-port` 列全表）。
- **`docs/archives/`**（原始 to-prd 移植的計畫／runbook／migration log）：從未版控，
  只存在於原作者本機 worktree，git 取不回。已版控文件不得連結它（gate 會擋）。
