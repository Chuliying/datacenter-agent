# Work Items

本目錄放單一變更的 durable workflow artifacts。每個 work item 使用同一個
`<slug>/` 目錄保存 PRD、spec、QA、實作報告與驗收報告；分類透過
`meta.yml` 的 `type`、`surface`、`area` 欄位提供，不把同一個 work item
拆散到 `prd/`、`spec/`、`qa/` 等不同目錄。

## Active

| Work item | Type | Surface | Area | Current stage | Source |
|---|---|---|---|---|---|
| [eval-evaluator-registry-fix](./eval-evaluator-registry-fix/prd.md) | feature | runtime | eval | PRD ready | `docs/work/eval-evaluator-registry-fix/meta.yml` |
| [evidence-pack-skillpackage-finalllmport](./evidence-pack-skillpackage-finalllmport/prd.md) | feature | runtime | evidence | PRD ready | `docs/work/evidence-pack-skillpackage-finalllmport/meta.yml` |

## Maintenance Rules

- `docs/work/<slug>/meta.yml` is the owner for status and stage state.
- Promote long-lived facts to `docs/reference/`; keep work-specific history here.
- New work items follow `.agent/skills/_shared/ARTIFACTS.md` v2.
