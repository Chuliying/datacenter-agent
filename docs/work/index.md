# Work Items

本目錄放單一變更的 durable workflow artifacts。每個 work item 使用同一個
`<slug>/` 目錄保存 PRD、spec、QA、實作報告與驗收報告，不把同一個 work item
拆散到 `prd/`、`spec/`、`qa/` 等不同目錄。

`meta.yml` 自 `work-item/v3` 起只收 schema 定義的欄位，`type` / `surface` / `area`
這類分類欄位已不再由 `meta.yml` 承載，改由本索引的表格提供。

## Active

| Work item | Type | Surface | Area | Execution mode | Current stage | Delivery |
|---|---|---|---|---|---|---|
| [eval-evaluator-registry-fix](./eval-evaluator-registry-fix/prd.md) | feature | runtime | eval | team-feature | PRD ready | — |
| [evidence-pack-skillpackage-finalllmport](./evidence-pack-skillpackage-finalllmport/prd.md) | feature | runtime | evidence | team-feature | PRD ready | — |
| [runtime-user-session-rate-limit](./runtime-user-session-rate-limit/prd.md) | feature | runtime | persistence | team-feature | PRD approved; spec + qa-plan in PR review | [PR #10](https://github.com/h-alice/datacenter-agent/pull/10) open |

## Completed（保留在原地，尚未 archive）

| Work item | Type | Surface | Area | Delivery | 遺留 gate |
|---|---|---|---|---|---|
| [retire-superseded-agent-endpoints](./retire-superseded-agent-endpoints/prd.md) | refactor | server | endpoints | [PR #11](https://github.com/h-alice/datacenter-agent/pull/11) merged `7aa2af1` | AC-001/AC-002 缺 route-level 404 斷言（見 prd.md `## Delivery`） |

## Archive

`_archive/` 保存已交付、不再變動的 work item（整份搬入，不拆檔）。

| Work item | Type | Surface | Area | Delivery |
|---|---|---|---|---|
| [agentgateway-openai-endpoint](./_archive/agentgateway-openai-endpoint/prd.md) | feature | server | endpoints | [PR #9](https://github.com/h-alice/datacenter-agent/pull/9) merged（v0.3.0） |

## Maintenance Rules

- `docs/work/<slug>/meta.yml` is the owner for work status, delivery status and stage state.
- Validate with `bash .agent/skills/_shared/scripts/work-items.sh check`.
- Promote long-lived facts to `docs/reference/`; keep work-specific history here.
- New work items follow `.agent/skills/_shared/ARTIFACTS.md` and declare `schema_version: work-item/v3`.
