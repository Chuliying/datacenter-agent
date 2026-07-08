# Tasks: Runtime Correctness and Platform Completion

## Metadata
- Plan Name: Runtime Correctness and Platform Completion
- Source File: implementation.md
- Generated At: 2026-06-29 14:28 +0800
- Updated At: 2026-06-29 14:50 +0800

## Task List

### T01 | reliable-sse-lifecycle
- Source IDs: [I01]
- Source Fingerprints: I01=8cdd691262a1
- Status: pending
- Priority: P0
- Owner: ai
- Summary: 讓 runtime SSE 具備 backpressure、disconnect cancellation、deadline、JoinError handling 與單一 terminal outcome。

#### Intent
消除 unbounded buffering 與 detached producer，使 client、handler、orchestrator、LLM/MCP 的生命週期一致；任何 disconnect、timeout、panic 或 abort 都有可觀察且不重複的 terminal outcome。

#### Expected Result
I01 的 lifecycle contract 完成並有 deterministic tests。

#### Execution Checklist
- [ ] 先寫 slow consumer、disconnect、JoinError、deadline failing tests
- [ ] 選定最小 bounded channel/cancellation design
- [ ] 實作 lifecycle 與 terminal audit
- [ ] 跑 targeted tests、完整 test/clippy，再同步 current-state docs

#### Impact Scope
- File/Artifact: `src/server/handler.rs`, `src/runtime/orchestrator.rs`, `src/runtime/audit.rs`
- File/Artifact: runtime SSE component/integration tests
- Documentation: `docs/reference/endpoints/agent-stream.md`, runtime orchestrator/audit pages

#### Definition of Done
- V01: bounded-channel slow-consumer test proves no unbounded enqueue path。
- V02: dropping SSE receiver cancels producer/upstream and prevents post-disconnect memory append。
- V03: timeout/JoinError/send failure each produce exactly one terminal error/cancel audit outcome。
- V04: normal stream ordering remains IntentResolved → Token* → Done。

#### Verification
- V01: bounded-channel slow-consumer test proves no unbounded enqueue path。
- V02: dropping SSE receiver cancels producer/upstream and prevents post-disconnect memory append。
- V03: timeout/JoinError/send failure each produce exactly one terminal error/cancel audit outcome。
- V04: normal stream ordering remains IntentResolved → Token* → Done。

#### Dependency
- Depends On: []
- Blocks: [T02, T05]

#### Execution Notes
- None

### T02 | http-contract-parity
- Source IDs: [I02]
- Source Fingerprints: I02=0ca7e2daf96d
- Status: pending
- Priority: P0
- Owner: ai
- Summary: 固定 legacy/runtime prompt caps、pre-stream validation、body status 與 deadline 外部契約。

#### Intent
讓 client 能依 endpoint/path 預測 prompt/body/status，不再出現 runtime REST 400 但 runtime SSE 200 error frame的隱性差異。

#### Expected Result
I02 的 Router contract 由 integration tests 固定。

#### Execution Checklist
- [ ] 建 Router test harness與prompt/body boundary failing tests
- [ ] 將runtime SSE structural validation移到response建立前
- [ ] 保留JsonRejection/body-limit正確status
- [ ] 與T01整合deadline/cancellation並同步docs

#### Impact Scope
- File/Artifact: `src/server/handler.rs`, `src/server/error.rs`, `src/server/route.rs`
- File/Artifact: Router oneshot integration tests
- Documentation: endpoint/spec/QA limit tables

#### Definition of Done
- V01: Router tests固定 legacy/runtime REST/SSE boundary status與body/frame。
- V02: >64 KiB JSON回413；malformed/missing JSON保留正確 extractor status。
- V03: no LLM/MCP call occurs for any structural rejection。
- V04: SSE deadline test與I01 cancellation test共同通過。

#### Verification
- V01: Router tests固定 legacy/runtime REST/SSE boundary status與body/frame。
- V02: >64 KiB JSON回413；malformed/missing JSON保留正確 extractor status。
- V03: no LLM/MCP call occurs for any structural rejection。
- V04: SSE deadline test與I01 cancellation test共同通過。

#### Dependency
- Depends On: [T01]
- Blocks: []

#### Execution Notes
- None

### T03 | llm-mcp-terminal-semantics
- Source IDs: [I03]
- Source Fingerprints: I03=01341a3a43b6
- Status: pending
- Priority: P0
- Owner: ai
- Summary: 防止 provider partial EOF 與 MCP semantic error被誤記成功。

#### Intent
只有合法 terminal signal 才能產生 Done/Ok；transport、protocol與semantic failure在 connector、orchestrator、HTTP/audit之間保持一致。

#### Expected Result
I03 的 provider/MCP semantic matrix有typed outcomes與tests。

#### Execution Checklist
- [ ] 建fake provider chunks與MCP semantic result fixtures
- [ ] 先重現natural EOF→Done與is_error→ok=true
- [ ] 實作typed terminal/tool outcome與safe logging
- [ ] 跑connector/orchestrator/HTTP regression suite並同步docs

#### Impact Scope
- File/Artifact: `src/llm_connector/agent.rs`, `src/mcp_client.rs`, runtime adapter/audit mapping
- File/Artifact: connector unit/component tests with deterministic fake streams/results
- Documentation: llm_connector、mcp_client、orchestrator、error contract pages

#### Definition of Done
- V01: natural EOF without finish reason必定非Done，`generate`回Err。
- V02: explicit valid finish仍維持Token* → Done。
- V03: MCP `is_error=true`產生`ToolResult.ok=false`且模型可讀error text。
- V04: logs/HTTP/SSE不包含raw tool args或完整upstream chain。

#### Verification
- V01: natural EOF without finish reason必定非Done，`generate`回Err。
- V02: explicit valid finish仍維持Token* → Done。
- V03: MCP `is_error=true`產生`ToolResult.ok=false`且模型可讀error text。
- V04: logs/HTTP/SSE不包含raw tool args或完整upstream chain。

#### Dependency
- Depends On: []
- Blocks: [T05]

#### Execution Notes
- None

### T04 | config-driven-runtime
- Source IDs: [I04]
- Source Fingerprints: I04=cce9ad26b8d7
- Status: pending
- Priority: P1
- Owner: ai
- Summary: 把config宣稱的module IDs變成production wiring，並接上injection與config-driven policy thresholds。

#### Intent
兌現PRD的config組合能力：config能選registry已註冊機制，request path實際使用所選元件；沒有接線的public config不再只是metadata。

#### Expected Result
I04 的public config每個宣稱module都有real builder/consumer與contract tests。

#### Execution Checklist
- [ ] 為stage order、injection E2E、policy threshold寫failing tests
- [ ] 定義stage/evaluator contracts與registry/AppState wiring
- [ ] 補numeric/order/conflict validation與第二capability pack
- [ ] 移除unsupported/noop claims並同步Spec/QA/PRD status

#### Impact Scope
- File/Artifact: `src/runtime/input/**`, `guardrails/**`, `registry.rs`, `config.rs`, `eval/**`, `src/appstate.rs`
- File/Artifact: `config/runtime/*.toml` schema/fixtures and component tests
- Documentation: runtime config/registry/input/guardrails/eval pages

#### Definition of Done
- V01: config stage順序改變會改變實際call sequence，且unknown/invalid order fail startup。
- V02: injection從HTTP/runtime request產warning、refusal、不呼叫AgentPort並audit。
- V03: policy boundary使用config values，修改fixture thresholds不需改Rust。
- V04: 第二個capability pack只換config即可通過相同pipeline contract suite。
- V05: 沒有`NoopEvaluator`被文件或CI當作已實作quality judge。

#### Verification
- V01: config stage順序改變會改變實際call sequence，且unknown/invalid order fail startup。
- V02: injection從HTTP/runtime request產warning、refusal、不呼叫AgentPort並audit。
- V03: policy boundary使用config values，修改fixture thresholds不需改Rust。
- V04: 第二個capability pack只換config即可通過相同pipeline contract suite。
- V05: 沒有`NoopEvaluator`被文件或CI當作已實作quality judge。

#### Dependency
- Depends On: [T08]
- Blocks: []

#### Execution Notes
- None

### T05 | identity-memory-audit
- Source IDs: [I05]
- Source Fingerprints: I05=daf4db894b00
- Status: pending
- Priority: P1
- Owner: ai
- Summary: 建立可信actor/session邊界、正確memory資料模型與audit/log去敏。

#### Intent
避免不同使用者以相同/猜測session id互讀memory，並確保audit、startup log、tool log、HTTP/SSE error不洩漏secret或敏感payload。

#### Expected Result
I05 的identity/data-classification決策落地，memory與所有output boundaries通過security tests。

#### Execution Checklist
- [ ] 先完成trusted actor/session與data classification decision
- [ ] 寫cross-actor memory與secret-fixture failing tests
- [ ] 接principal、central redaction、memory model與terminal audit
- [ ] 跑security/memory/audit/HTTP tests並同步docs

#### Impact Scope
- File/Artifact: `src/server/auth.rs`, `handler.rs`, `runtime/memory/**`, `runtime/audit.rs`, `main.rs`, `mcp_client.rs`, `llm_connector/agent.rs`
- File/Artifact: security/memory/audit tests and data classification docs
- Documentation: PRD identity decision、memory/audit/server modules、operations guidance

#### Definition of Done
- V01: 不同actor使用同session id無法讀寫彼此memory。
- V02: secret fixture不出現在stdout audit、tracing、HTTP或SSE output。
- V03: memory filter/drop/truncate與retention tests對應文件用詞。
- V04: every terminal outcome含request correlation與去敏actor/session表示。

#### Verification
- V01: 不同actor使用同session id無法讀寫彼此memory。
- V02: secret fixture不出現在stdout audit、tracing、HTTP或SSE output。
- V03: memory filter/drop/truncate與retention tests對應文件用詞。
- V04: every terminal outcome含request correlation與去敏actor/session表示。

#### Dependency
- Depends On: [T01, T03, T07]
- Blocks: []

#### Execution Notes
- Q01/Q02 decisions must be recorded before code changes.

### T06 | trustworthy-eval-gate
- Source IDs: [I06]
- Source Fingerprints: I06=e4ee233df577
- Status: pending
- Priority: P0
- Owner: ai
- Summary: 讓reported eval failure真的擋CI，並使evaluator名稱與實際能力一致。

#### Intent
消除false-green CI；任何regression都必須由process status可機器判定，quality claim只能對應已實作evaluator。

#### Expected Result
I06 的positive/negativeprocess tests與CI gate都能可靠區分pass/fail。

#### Execution Checklist
- [ ] 建binary process test重現failed=1 exit0
- [ ] 修正report failure exit並驗positive/replay
- [ ] 增加CI negative self-test
- [ ] 與T04對齊evaluator scope並同步docs

#### Impact Scope
- File/Artifact: `src/bin/eval.rs`, `src/runtime/eval/**`, `src/runtime/registry.rs`, `.github/workflows/runtime.yml`
- File/Artifact: eval fixtures and process integration tests
- Documentation: runtime-eval、QA、CI/runbook

#### Definition of Done
- V01: synthetic failed=1 replay process exit nonzero。
- V02: valid pipeline/replay仍exit0。
- V03: CI negative self-test在binary錯誤回0時會失敗。
- V04: evaluator config IDs都有non-noop behavior或明確unsupported error。

#### Verification
- V01: synthetic failed=1 replay process exit nonzero。
- V02: valid pipeline/replay仍exit0。
- V03: CI negative self-test在binary錯誤回0時會失敗。
- V04: evaluator config IDs都有non-noop behavior或明確unsupported error。

#### Dependency
- Depends On: []
- Blocks: []

#### Execution Notes
- None

### T07 | auth-cors-probes
- Source IDs: [I07]
- Source Fingerprints: I07=194e156d8b2c
- Status: pending
- Priority: P1
- Owner: ai
- Summary: 在不默認破壞client的前提下決定並實作標準auth/CORS/probe deployment contract。

#### Intent
讓browser/API/probe exposure有明確安全邊界，移除very-permissive預設與未測試的418/probe假設。

#### Expected Result
I07 的auth migration、CORS allowlist與probe profile都有決策、implementation與tests。

#### Execution Checklist
- [ ] 蒐集418/client、browser origin與deployment probe constraints
- [ ] 記錄auth/probe/CORS decision
- [ ] 先寫Router/deployment contract tests再實作
- [ ] 更新README/endpoints/runbook與migration note

#### Impact Scope
- File/Artifact: `src/server/auth.rs`, `route.rs`, `handler.rs`, app config/env schema
- File/Artifact: Router auth/CORS tests and deployment/runbook docs
- Documentation: endpoints、README、PRD decision status

#### Definition of Done
- V01: chosen authstatus/header/body有characterization + migration tests。
- V02: disallowed origin無CORS grant；allowed origin/method/header按config通過。
- V03: documented probe profile可在representative deployment smoke通過。
- V04: ready failure log不含raw credential URL。

#### Verification
- V01: chosen authstatus/header/body有characterization + migration tests。
- V02: disallowed origin無CORS grant；allowed origin/method/header按config通過。
- V03: documented probe profile可在representative deployment smoke通過。
- V04: ready failure log不含raw credential URL。

#### Dependency
- Depends On: []
- Blocks: [T05]

#### Execution Notes
- Q01/Q02/Q03 are explicit decision gates.

### T08 | startup-rollback-and-evidence
- Source IDs: [I08]
- Source Fingerprints: I08=49d086aa161a
- Status: pending
- Priority: P1
- Owner: ai
- Summary: 讓flag off真正隔離runtime startup failure，補齊validation/contract gates並維持文件單一事實。

#### Intent
使legacy rollback在runtime config損壞時仍可用，並建立code→test→Spec/QA→PRD status的完成閘門。

#### Expected Result
I08 的startup matrix、verification wrapper與documentation status gate可重現通過。

#### Execution Checklist
- [ ] 建flag/config startup matrix failing tests
- [ ] 在load runtime config前決定enabled path
- [ ] 修正manifest command parser並加stdout regression test
- [ ] 建PRD status→Spec/QA evidence更新gate與link/source checks

#### Impact Scope
- File/Artifact: `src/appstate.rs`, runtime config validation, startup tests
- File/Artifact: `.agent/skills/_shared/scripts/manifest-stack.sh` and its tests
- Documentation: `docs/reference/**`, README, `.agent/knowledge/system-context.md`

#### Definition of Done
- V01: invalid runtime config + flag off仍可build AppState/serve legacy；flag on fail-fast。
- V02: every runtime numeric/order invariant hasnegative config test。
- V03: canonical verification wrapper正確執行cargo command並傳遞exit status。
- V04: doc link/fragment/source-anchor checker通過，PRD status與QA evidence無未解釋衝突。
- V05: final worktree verification證明每個implementation task只改declared impact scope。

#### Verification
- V01: invalid runtime config + flag off仍可build AppState/serve legacy；flag on fail-fast。
- V02: every runtime numeric/order invariant hasnegative config test。
- V03: canonical verification wrapper正確執行cargo command並傳遞exit status。
- V04: doc link/fragment/source-anchor checker通過，PRD status與QA evidence無未解釋衝突。
- V05: final worktree verification證明每個implementation task只改declared impact scope。

#### Dependency
- Depends On: []
- Blocks: [T04]

#### Execution Notes
- None

### T09 | capability-evidence-boundary
- Source IDs: [I09]
- Source Fingerprints: I09=b037dad77d12
- Status: pending
- Priority: P1
- Owner: ai
- Summary: 分離能力執行、證據封裝、prompt組裝與最終生成，使Final LLM無MCP/DB/RAG access。

#### Intent
建立可審計的能力引用與證據信任邊界：所有資料/工具存取由受控gateway完成，Evidence Hub把結果封裝成可追溯Evidence Pack，Final LLM只消費compiled prompt並生成候選答案。

#### Expected Result
I09 的 Skill Package、Gateway、Evidence Pack、Prompt Builder、tool-less Final LLM與Output Validator contracts完成並有跨boundary tests。

#### Execution Checklist
- [ ] 先定義SkillPackage/EvidencePack schema、trust/budget/citation invariants與FinalLlmPort forbidden dependencies
- [ ] 建gateway allow/deny、pack tamper/stale、Prompt Builder golden、citation validation failing tests
- [ ] 實作Gateway/Evidence Hub/Prompt Builder/Final LLM/Output Validator seams並重構orchestrator flow
- [ ] 跑unit/component/integration/security tests，確認secret/credential/tool handle不越界
- [ ] 同步PRD status、current Spec/QA、module/architecture docs與audit events

#### Impact Scope
- File/Artifact: new `src/runtime/capability/**`, `evidence/**`, `prompt_builder/**`, `output_validate/**`, `tool_gateway/**` modules or equivalent seams
- File/Artifact: refactor `src/runtime/orchestrator.rs`, `src/llm_connector/**`, `src/mcp_client.rs`, `src/appstate.rs`, runtime config/schema/audit
- File/Artifact: unit/component/integration/security fixtures for SkillPackage/EvidencePack/Gateway/PromptBuilder/FinalLlmPort/OutputValidator
- Documentation: PRD FR-013/AC-013、Spec capability gap、QA evidence tests、architecture/module pages

#### Definition of Done
- V01: compile/API dependency test證明`FinalLlmPort`不接受`ChatCompletionTool`、`McpHandle`、DB/RAG client或credentials。
- V02: gateway allowed/denied tool與scope/argument/cost/timeout tests證明complete mediation；denied request執行次數為0。
- V03: Evidence Pack schema、version、digest、freshness、classification、budget、partial/conflict states皆有positive/negative tests。
- V04: Prompt Builder golden tests證明相同inputs產相同compiled prompt，external evidence/memory明確標untrusted，secret fixture不進prompt。
- V05: Output Validator拒絕unknown/missing citation與invalid schema，repair超budget後typed failure。
- V06: end-to-end fake flow只讓Final LLM看到compiled prompt；published claims/citations可追溯到Evidence Pack item與gateway audit。

#### Verification
- V01: compile/API dependency test證明`FinalLlmPort`不接受`ChatCompletionTool`、`McpHandle`、DB/RAG client或credentials。
- V02: gateway allowed/denied tool與scope/argument/cost/timeout tests證明complete mediation；denied request執行次數為0。
- V03: Evidence Pack schema、version、digest、freshness、classification、budget、partial/conflict states皆有positive/negative tests。
- V04: Prompt Builder golden tests證明相同inputs產相同compiled prompt，external evidence/memory明確標untrusted，secret fixture不進prompt。
- V05: Output Validator拒絕unknown/missing citation與invalid schema，repair超budget後typed failure。
- V06: end-to-end fake flow只讓Final LLM看到compiled prompt；published claims/citations可追溯到Evidence Pack item與gateway audit。

#### Dependency
- Depends On: [T01, T03, T04, T05]
- Blocks: []

#### Execution Notes
- Q01/Q02/Q03 must be resolved during design; do not fall back to direct Final LLM tool access.
