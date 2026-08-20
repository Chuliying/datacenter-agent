# Work Items

本目錄放單一變更的 durable workflow artifacts。每個 work item 使用同一個
`<slug>/` 目錄保存 PRD、spec、QA、實作報告與驗收報告，不把同一個 work item
拆散到 `prd/`、`spec/`、`qa/` 等不同目錄。

`meta.yml` 自 `work-item/v3` 起只收 schema 定義的欄位，`type` / `surface` / `area`
這類分類欄位已不再由 `meta.yml` 承載，改由本索引的表格提供。

skill-commons v0.10.0 起，`work_status: completed` 的 v3 work item 必須宣告
`documentation_impact`（`doc:<專案文件路徑>` 或 `n/a:<200 字內理由>`），且 `doc:` 不得
指向 `docs/work/` 底下任何檔案。本目錄的 active item 已預先宣告目標文件，收尾時不必回頭補。

## Active

| Work item | Type | Surface | Area | Execution mode | Current stage | Delivery |
|---|---|---|---|---|---|---|
| [eval-evaluator-registry-fix](./eval-evaluator-registry-fix/prd.md) | feature | runtime | eval | team-feature | PRD ready | — |
| [evidence-pack-skillpackage-finalllmport](./evidence-pack-skillpackage-finalllmport/prd.md) | feature | runtime | evidence | team-feature | PRD ready | — |
| [reference-docs-040-sync](./reference-docs-040-sync/prd.md) | docs | reference | docs | refactor | PRD ready | — |

## Completed（保留在原地，尚未 archive）

| Work item | Type | Surface | Area | Delivery | 遺留 gate |
|---|---|---|---|---|---|
| [retire-superseded-agent-endpoints](./retire-superseded-agent-endpoints/prd.md) | refactor | server | endpoints | [PR #11](https://github.com/h-alice/datacenter-agent/pull/11) merged `7aa2af1` | AC-001/AC-002 缺 route-level 404 斷言（見 prd.md `## Delivery`） |

## 尚未進 main

以下 work item 只存在於分支，`main` 上沒有它的檔案，本表只記狀態。

| Work item | 位置 | 狀態 | 進 main 前的已知 gate |
|---|---|---|---|
| runtime-user-session-rate-limit | `codex/runtime-user-session-rate-limit`（[PR #10](https://github.com/h-alice/datacenter-agent/pull/10)） | PRD approved v0.5.0、spec awaiting-approval、qa-plan ready | `meta.yml` 仍是 legacy 形狀且 `created_at` 為 2026-08-13（晚於 v3 採用日 2026-07-25），v0.10.0 的 `work-items.sh check` 會擋：缺 `schema_version`、`handoff` 與 `decisions` 是未知欄位。合併前需遷移到 `work-item/v3`。 |

## Archive

`_archive/` 保存已交付、不再變動的 work item（整份搬入，不拆檔）。裡面的 `meta.yml`
維持 legacy schema 且**刻意不遷移**：v3 遷移是刻意工作而非改檔的副作用，而它的
`release` stage 指向 `finishing-a-development-branch`——該 owner 已不在 skill-commons
的技能樹上，v0.10.0 的 stage-skill 檢查會擋下遷移後的結果。

| Work item | Type | Surface | Area | Delivery |
|---|---|---|---|---|
| [agentgateway-openai-endpoint](./_archive/agentgateway-openai-endpoint/prd.md) | feature | server | endpoints | [PR #9](https://github.com/h-alice/datacenter-agent/pull/9) merged（v0.3.0） |

## Maintenance Rules

- `docs/work/<slug>/meta.yml` is the owner for work status, delivery status and stage state.
- Validate with `bash .agent/skills/_shared/scripts/work-items.sh check`.
- Promote long-lived facts to `docs/reference/`; keep work-specific history here.
- New work items follow `.agent/skills/_shared/ARTIFACTS.md` and declare `schema_version: work-item/v3`.
