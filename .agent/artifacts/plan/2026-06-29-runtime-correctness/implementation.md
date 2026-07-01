# Plan: Runtime Correctness and Platform Completion

## Metadata
- Plan Name: Runtime Correctness and Platform Completion
- Plan Slug: 2026-06-29-runtime-correctness
- Status: active
- Created At: 2026-06-29 14:26 +0800
- Updated At: 2026-06-29 14:50 +0800

## Goal
把 `docs/reference/prd.md` 中所有 🟡／⬜ 項目落成可驗證的程式行為，使 runtime 具備可靠 streaming、正確 terminal semantics、真正 config wiring、安全 memory/audit、可信 CI gate，以及 Capability Gateway→Evidence Pack→tool-less Final LLM 的能力隔離，同時維持已宣告的 legacy compatibility。

## Success Criteria
- SC01: PRD AC-001～AC-013 每項轉為 ✅ 前，都有對應 production call path 與 contract test。
- SC02: slow consumer、disconnect、timeout、provider truncation、MCP semantic error 都不會被標成成功完成。
- SC03: intentional eval regression 使 binary 與 CI step nonzero。
- SC04: capability config 真正控制已宣告的 stages/guardrails/extractors/evaluators，或刪除不支援的宣告。
- SC05: log/audit/memory 不以 raw credential、tool args 或 client session id 單獨形成安全邊界。
- SC06: runtime flag off 時，無效 runtime config 不阻擋 legacy startup。
- SC07: Spec、QA 與 module/endpoint reference 在每次 code task 完成後同步更新，PRD status 只在驗證通過後升級。
- SC08: Final LLM port在type/dependency層無法取得tool schema、MCP/DB/RAG handles或credentials；published factual claims可回指validated Evidence Pack item/citation。

## Scope
- In: `src/server/**` 的 Router contract、REST/SSE validation、auth/CORS/probe 與 error mapping。
- In: `src/llm_connector/**`、`src/mcp_client.rs`、`src/runtime/**`、`src/appstate.rs` 的 correctness/security/wiring。
- In: runtime config schema/validation、eval binary/CI、unit/component/integration tests。
- In: Capability Registry/Skill Package、Capability Gateway/Tool Hub、Evidence Hub/Evidence Pack、Prompt Builder、tool-less Final LLM port與Output Validator contracts。
- In: 受影響的 `docs/reference/**`、README、runbook/QA evidence 同步。

## Non-goals
- Out: 本計劃不在文件整理階段修改任何 Rust、config、test、script 或 workflow。
- Out: 不建立任意第三方 native dynamic-plugin loader；pluggability 限 registry 已註冊元件。
- Out: 不在 identity contract 決定前宣稱 multi-tenant memory 完成。
- Out: 不在沒有真 evaluator 前宣稱 LLM judge、grounding 或 hallucination evaluation。
- Out: 不移除 legacy path；deprecation 需另立產品決策。
- Out: 不把Final LLM繼續直接tool-calling包裝成Evidence Pack架構；I09完成前兩者必須被視為不同模式。

## Assumptions
- A01: legacy prompt cap 2000 與既有 SSE events 在 migration window 保持相容。
- A02: runtime EV capability pack 預設 prompt cap 4000。
- A03: Rust toolchain 透過 `rustup which cargo` 可用；實作指令一律 `--locked`。
- A04: live LLM/MCP、staging 與 deployment tests 需要明確外部授權，不作一般 PR 必跑項。
- A05: external evidence與memory皆為untrusted data；Evidence Pack只提升traceability/integrity，不把內容自動變成可信instruction。

## Decisions
- D01: PRD 是 target-state source of truth；Spec/QA/endpoint/module pages 是 current-state evidence。
- D02: 程式工作以 test-first 小步執行；每個 task 先建立能重現現況缺陷的 failing test。
- D03: runtime SSE 使用 bounded channel、structured cancellation 與單一 terminal outcome；不保留 unbounded queue。
- D04: provider 沒有合法 terminal reason 時不得回 Done/Ok；MCP semantic error 必須保留 `ok=false`。
- D05: config 宣稱可選的元件必須真 dispatch；否則從 public config contract 移除。
- D06: auth 418 migration、probe policy、trusted actor source 是 decision gates，不能由實作者默認。
- D07: Final LLM只負責generation，不接收tools、MCP/DB/RAG handles或credentials；所有能力執行經code-controlled gateway，資料以validated Evidence Pack進入Prompt Builder。

## Implementation Sections

### I01 | reliable-sse-lifecycle
- Key: reliable-sse-lifecycle
- Title: Bounded and cancellable SSE lifecycle
- Status: active
- Summary: 讓 runtime SSE 具備 backpressure、disconnect cancellation、deadline、JoinError handling 與單一 terminal outcome。

#### Intent
消除 unbounded buffering 與 detached producer，使 client、handler、orchestrator、LLM/MCP 的生命週期一致；任何 disconnect、timeout、panic 或 abort 都有可觀察且不重複的 terminal outcome。

#### Logic
在 `agent_stream_runtime` 使用有容量的 `mpsc::channel`，讓 emit/send 能 backpressure 或明確失敗；以 cancellation token、abort-on-drop guard 或 owned task wrapper 把 SSE body drop 傳回 producer。把 120 秒 runtime turn deadline 放在 turn/task 層，而非只依賴 Router handler timeout。明確映射 `JoinError`、send failure、deadline 與 `AgentTurnOutcome::Aborted`；normal completion只能產生一次 Done。所有 exit path 都寫 terminal audit，且 client disconnect 不繼續回寫 answer memory。

#### Edge Cases
- EC01: client 在第一個 token 前斷線。
- EC02: slow consumer 填滿 channel。
- EC03: producer panic、task cancelled、audit sink fail-open/fail-closed。
- EC04: deadline 與正常 Done 同時競爭。
- EC05: refusal 在沒有上游 call 時仍能乾淨完成。

#### Impact Areas
- File/Artifact: `src/server/handler.rs`, `src/runtime/orchestrator.rs`, `src/runtime/audit.rs`
- File/Artifact: runtime SSE component/integration tests
- Documentation: `docs/reference/endpoints/agent-stream.md`, runtime orchestrator/audit pages

#### Validation
- V01: bounded-channel slow-consumer test proves no unbounded enqueue path。
- V02: dropping SSE receiver cancels producer/upstream and prevents post-disconnect memory append。
- V03: timeout/JoinError/send failure each produce exactly one terminal error/cancel audit outcome。
- V04: normal stream ordering remains IntentResolved → Token* → Done。

#### Open Questions
- Q01: cancellation 使用 `tokio_util::sync::CancellationToken` 或現有 dependency-free abort guard；實作前以最小依賴方案決定。

### I02 | http-contract-parity
- Key: http-contract-parity
- Title: REST/SSE validation, limits and status parity
- Status: active
- Summary: 固定 legacy/runtime prompt caps、pre-stream validation、body status 與 deadline 外部契約。

#### Intent
讓 client 能依 endpoint/path 預測 prompt/body/status，不再出現 runtime REST 400 但 runtime SSE 200 error frame的隱性差異。

#### Logic
在建立 runtime SSE response/task 前執行與 orchestrator 共用的 structural validation；避免複製規則，可抽出純 validation function或讓 prelude 在 response 前執行。保留 legacy 2000 與 runtime config 4000。調整 `JsonRejection` mapping以保留 body-limit 413與其他 extractor status。定義 REST deadline 504、SSE deadline terminal error + cancellation，並使 external error body只使用 stable code/message。

#### Edge Cases
- EC01: empty/whitespace prompt。
- EC02: legacy 2000/2001、runtime 4000/4001、runtime 2001。
- EC03: multibyte Unicode char count 與 64 KiB encoded body不是同一限制。
- EC04: malformed JSON、missing prompt、oversized body。

#### Impact Areas
- File/Artifact: `src/server/handler.rs`, `src/server/error.rs`, `src/server/route.rs`
- File/Artifact: Router oneshot integration tests
- Documentation: endpoint/spec/QA limit tables

#### Validation
- V01: Router tests固定 legacy/runtime REST/SSE boundary status與body/frame。
- V02: >64 KiB JSON回413；malformed/missing JSON保留正確 extractor status。
- V03: no LLM/MCP call occurs for any structural rejection。
- V04: SSE deadline test與I01 cancellation test共同通過。

#### Open Questions
- Q01: stable public error code schema是否在既有 `{error:string}` 中擴充或另立version；需先做 compatibility check。

### I03 | llm-mcp-terminal-semantics
- Key: llm-mcp-terminal-semantics
- Title: Correct LLM stream and MCP tool outcomes
- Status: active
- Summary: 防止 provider partial EOF 與 MCP semantic error被誤記成功。

#### Intent
只有合法 terminal signal 才能產生 Done/Ok；transport、protocol與semantic failure在 connector、orchestrator、HTTP/audit之間保持一致。

#### Logic
在 `agent_stream` 每輪追蹤是否看到 finish reason、finish kind、tool-call completeness與terminal event。natural EOF without finish reason回 typed error/aborted，不 emit Done。`generate` 在stream結束但沒有 Done時回 Err。把 MCP call result改為包含 text + semantic status的typed outcome；`is_error=true`仍把text回灌模型，但 emit/audit `ok=false`。raw tool args只保留hash/size，upstream error對client映射stable code。

#### Edge Cases
- EC01: partial content後EOF。
- EC02: tool-call JSON arguments只收到一部分即EOF。
- EC03: finish_reason表示tool call但缺id/name。
- EC04: MCP transport成功但`is_error=true`。
- EC05: MCP error後模型下一輪成功自我修正。

#### Impact Areas
- File/Artifact: `src/llm_connector/agent.rs`, `src/mcp_client.rs`, runtime adapter/audit mapping
- File/Artifact: connector unit/component tests with deterministic fake streams/results
- Documentation: llm_connector、mcp_client、orchestrator、error contract pages

#### Validation
- V01: natural EOF without finish reason必定非Done，`generate`回Err。
- V02: explicit valid finish仍維持Token* → Done。
- V03: MCP `is_error=true`產生`ToolResult.ok=false`且模型可讀error text。
- V04: logs/HTTP/SSE不包含raw tool args或完整upstream chain。

#### Open Questions
- Q01: aborted與upstream error對外是否共用502 stable code，或新增不破壞wire的SSE code欄位。

### I04 | config-driven-runtime
- Key: config-driven-runtime
- Title: Real stage, guardrail, extractor and evaluator dispatch
- Status: active
- Summary: 把config宣稱的module IDs變成production wiring，並接上injection與config-driven policy thresholds。

#### Intent
兌現PRD的config組合能力：config能選registry已註冊機制，request path實際使用所選元件；沒有接線的public config不再只是metadata。

#### Logic
定義stage contract與ordered dispatcher，由registry把`input_stages`解析成可執行components，AppState持有build結果。將input guard/injection/intent/slots依order執行，確保typed warnings與normalized output contract。answer policy讀validated thresholds，不硬編0.5/0.7。對extractor/guardrail/evaluator同樣建立real builder與consumer；若本階段不支援某類，縮減config schema/文件而不是回noop。補numeric/order/conflict validation與兩個capability-pack contract fixtures。

#### Edge Cases
- EC01: missing/duplicate stage、injection在normalize之前、intent/slots缺少前置stage。
- EC02: confidence越界或gray > normal。
- EC03: enabled module ID已知但缺production builder。
- EC04: injection regex有效但warning/policy flow失敗。

#### Impact Areas
- File/Artifact: `src/runtime/input/**`, `guardrails/**`, `registry.rs`, `config.rs`, `eval/**`, `src/appstate.rs`
- File/Artifact: `config/runtime/*.toml` schema/fixtures and component tests
- Documentation: runtime config/registry/input/guardrails/eval pages

#### Validation
- V01: config stage順序改變會改變實際call sequence，且unknown/invalid order fail startup。
- V02: injection從HTTP/runtime request產warning、refusal、不呼叫AgentPort並audit。
- V03: policy boundary使用config values，修改fixture thresholds不需改Rust。
- V04: 第二個capability pack只換config即可通過相同pipeline contract suite。
- V05: 沒有`NoopEvaluator`被文件或CI當作已實作quality judge。

#### Open Questions
- Q01: evaluator execution屬offline runner而非request path；registry contract需分清runtime components與eval components。

### I05 | identity-memory-audit
- Key: identity-memory-audit
- Title: Trusted memory scope and centralized redaction
- Status: active
- Summary: 建立可信actor/session邊界、正確memory資料模型與audit/log去敏。

#### Intent
避免不同使用者以相同/猜測session id互讀memory，並確保audit、startup log、tool log、HTTP/SSE error不洩漏secret或敏感payload。

#### Logic
先決定可信principal來源與single-token部署的tenant semantics；由auth middleware寫入typed principal extension，handler用它建立AuditCtx與SessionMemoryScope。把raw full text從summary欄位移除或重新命名並定義retention；context sanitizer/budget明確區分filter、drop與truncate。把redaction放在sink/log boundary，採結構化safe fields：URL只記scheme/host、tool args只記hash/size、error只對外stable code。補terminal/cancel events與memory failure policy。

#### Edge Cases
- EC01: 相同session id、不同actor/tenant。
- EC02: anonymous/single-service-token deployment沒有user identity。
- EC03: secret出現在URL query、bearer、tool args、nested error、session/option field。
- EC04: memory context超budget或包含instruction-like text。

#### Impact Areas
- File/Artifact: `src/server/auth.rs`, `handler.rs`, `runtime/memory/**`, `runtime/audit.rs`, `main.rs`, `mcp_client.rs`, `llm_connector/agent.rs`
- File/Artifact: security/memory/audit tests and data classification docs
- Documentation: PRD identity decision、memory/audit/server modules、operations guidance

#### Validation
- V01: 不同actor使用同session id無法讀寫彼此memory。
- V02: secret fixture不出現在stdout audit、tracing、HTTP或SSE output。
- V03: memory filter/drop/truncate與retention tests對應文件用詞。
- V04: every terminal outcome含request correlation與去敏actor/session表示。

#### Open Questions
- Q01: 可信actor來自JWT/mTLS/reverse-proxy signed header或新增API credential model；GLOBAL_TOKEN本身無法區分end users。
- Q02: session_id應hash、tokenize或保留server-generated opaque ID；需配合operational query需求。

### I06 | trustworthy-eval-gate
- Key: trustworthy-eval-gate
- Title: Nonzero regression gate and real evaluator scope
- Status: active
- Summary: 讓reported eval failure真的擋CI，並使evaluator名稱與實際能力一致。

#### Intent
消除false-green CI；任何regression都必須由process status可機器判定，quality claim只能對應已實作evaluator。

#### Logic
`src/bin/eval.rs`在`report.failed > 0`時exit nonzero；用process-level integration test執行intentional failing artifact，避免只測runner counts。pipeline-only改走I04 configured pipeline並增加guardrail/policy cases。response evaluator為每個config ID建立真實implementation或拒絕startup；未建LLM judge前移除相關claim。CI保留positive smoke並增加negative self-test（預期command fail的assert wrapper）。

#### Edge Cases
- EC01: report failed>0但runner本身Ok。
- EC02: empty fixture、pending baseline、duplicatecase。
- EC03: intentional-negative CI command的nonzero需被wrapper視為PASS。

#### Impact Areas
- File/Artifact: `src/bin/eval.rs`, `src/runtime/eval/**`, `src/runtime/registry.rs`, `.github/workflows/runtime.yml`
- File/Artifact: eval fixtures and process integration tests
- Documentation: runtime-eval、QA、CI/runbook

#### Validation
- V01: synthetic failed=1 replay process exit nonzero。
- V02: valid pipeline/replay仍exit0。
- V03: CI negative self-test在binary錯誤回0時會失敗。
- V04: evaluator config IDs都有non-noop behavior或明確unsupported error。

#### Open Questions
- Q01: regression exit code使用1或獨立code；先確認現有CI/consumer是否只判zero/nonzero。

### I07 | auth-cors-probes
- Key: auth-cors-probes
- Title: Versioned authentication, least-privilege CORS and probe policy
- Status: active
- Summary: 在不默認破壞client的前提下決定並實作標準auth/CORS/probe deployment contract。

#### Intent
讓browser/API/probe exposure有明確安全邊界，移除very-permissive預設與未測試的418/probe假設。

#### Logic
先做decision record：418是否有client依賴、是否versioned migration到401 + WWW-Authenticate、health/ready是否保持auth或由private listener/network policy保護。CORS改為config allowlist或非browser時disabled，禁止credentials + arbitrary origin。為選定probe profile提供deployment example/smoke test，ready外部HEAD加入safe URL logging與可接受的cache/rate policy。

#### Edge Cases
- EC01: missing header、wrong scheme、wrong token、case-insensitiveBearer。
- EC02: browser credentialed request fromunknown origin。
- EC03: kube probe withcustom header與without header profiles。
- EC04: ready base URLslow/down/credential-bearing URL。

#### Impact Areas
- File/Artifact: `src/server/auth.rs`, `route.rs`, `handler.rs`, app config/env schema
- File/Artifact: Router auth/CORS tests and deployment/runbook docs
- Documentation: endpoints、README、PRD decision status

#### Validation
- V01: chosen authstatus/header/body有characterization + migration tests。
- V02: disallowed origin無CORS grant；allowed origin/method/header按config通過。
- V03: documented probe profile可在representative deployment smoke通過。
- V04: ready failure log不含raw credential URL。

#### Open Questions
- Q01: 是否保留418作legacy version並在新version改401。
- Q02: probes使用auth header、private listener或path exemption。
- Q03: production allowed origins清單與browser credentials需求。

### I08 | startup-rollback-and-evidence
- Key: startup-rollback-and-evidence
- Title: Real rollback, complete config validation and evidence synchronization
- Status: active
- Summary: 讓flag off真正隔離runtime startup failure，補齊validation/contract gates並維持文件單一事實。

#### Intent
使legacy rollback在runtime config損壞時仍可用，並建立code→test→Spec/QA→PRD status的完成閘門。

#### Logic
在AppState讀取runtime flag後才決定是否load/build runtime；flag off直接None或明確disabled lightweight state，不讀runtime refs。flag on執行I04完整validation。補startup matrix tests。修復project manifest command parser對Markdown backticks的處理，讓canonical verification wrapper可信。每個task完成時先更新current Spec/QA，再只有在contract verification通過後把PRD requirement升為✅；historical migration docs不覆寫canonical status。

#### Edge Cases
- EC01: runtime refs缺檔/invalid regex/unknown module + flag off。
- EC02: flag true/1/false/missing/other case combinations。
- EC03: runtime section完全不存在。
- EC04: verification wrapper command有stdout與Markdown backticks。

#### Impact Areas
- File/Artifact: `src/appstate.rs`, runtime config validation, startup tests
- File/Artifact: `.agent/skills/_shared/scripts/manifest-stack.sh` and its tests
- Documentation: `docs/reference/**`, README, `.agent/knowledge/system-context.md`

#### Validation
- V01: invalid runtime config + flag off仍可build AppState/serve legacy；flag on fail-fast。
- V02: every runtime numeric/order invariant hasnegative config test。
- V03: canonical verification wrapper正確執行cargo command並傳遞exit status。
- V04: doc link/fragment/source-anchor checker通過，PRD status與QA evidence無未解釋衝突。
- V05: final worktree verification證明每個implementation task只改declared impact scope。

#### Open Questions
- Q01: disabled runtime是否完全不建`AppRuntime`，或保留不讀外部config的diagnostic shell；以最小rollback surface優先。

### I09 | capability-evidence-boundary
- Key: capability-evidence-boundary
- Title: Capability Gateway, Evidence Pack and tool-less Final LLM
- Status: active
- Summary: 分離能力執行、證據封裝、prompt組裝與最終生成，使Final LLM無MCP/DB/RAG access。

#### Intent
建立可審計的能力引用與證據信任邊界：所有資料/工具存取由受控gateway完成，Evidence Hub把結果封裝成可追溯Evidence Pack，Final LLM只消費compiled prompt並生成候選答案。

#### Logic
新增versioned `SkillPackage` contract，包含capability id/version、instructions refs、allowed evidence sources/tools、required scopes、output schema、budgets與policy refs。`CapabilityGateway`獨占MCP/DB/RAG clients與credentials，驗tool allowlist、scopes、argument schema、rate/cost/timeout及actor/tenant policy後才執行。`EvidenceHub`規劃retrieval並只透過gateway取得結果，正規化為immutable、request-scoped `EvidencePack`：pack/request/capability identity、items的source/provenance/timestamps/freshness/classification/trust/digest、citations、policy decisions、warnings、partial state與size/token budget。`PromptBuilder` deterministic組合Skill Package、validated Evidence Pack、untrusted memory、output schema與request context。建立不接受tool schemas/MCP handle/DB/RAG client/credentials的`FinalLlmPort`；`OutputValidator`驗schema、citation existence/coverage、policy violations並限制repair次數。每階段使用同request id audit；credential與raw policy secret不得進pack/prompt。

#### Edge Cases
- EC01: capability不存在、version不相容、tool/source不在allowlist或scope不足。
- EC02: Evidence Pack空、partial、stale/expired、oversized、conflicting、classified或digest不符。
- EC03: retrieved evidence含indirect prompt injection或偽造instruction/citation。
- EC04: gateway部分tool成功、部分timeout/error；不得把partial pack標complete。
- EC05: Final LLM輸出引用不存在/被redact的evidence id，或claim沒有citation。
- EC06: Output Validator repair超budget；不得在repair階段取得tools/data access。

#### Impact Areas
- File/Artifact: new `src/runtime/capability/**`, `evidence/**`, `prompt_builder/**`, `output_validate/**`, `tool_gateway/**` modules or equivalent seams
- File/Artifact: refactor `src/runtime/orchestrator.rs`, `src/llm_connector/**`, `src/mcp_client.rs`, `src/appstate.rs`, runtime config/schema/audit
- File/Artifact: unit/component/integration/security fixtures for SkillPackage/EvidencePack/Gateway/PromptBuilder/FinalLlmPort/OutputValidator
- Documentation: PRD FR-013/AC-013、Spec capability gap、QA evidence tests、architecture/module pages

#### Validation
- V01: compile/API dependency test證明`FinalLlmPort`不接受`ChatCompletionTool`、`McpHandle`、DB/RAG client或credentials。
- V02: gateway allowed/denied tool與scope/argument/cost/timeout tests證明complete mediation；denied request執行次數為0。
- V03: Evidence Pack schema、version、digest、freshness、classification、budget、partial/conflict states皆有positive/negative tests。
- V04: Prompt Builder golden tests證明相同inputs產相同compiled prompt，external evidence/memory明確標untrusted，secret fixture不進prompt。
- V05: Output Validator拒絕unknown/missing citation與invalid schema，repair超budget後typed failure。
- V06: end-to-end fake flow只讓Final LLM看到compiled prompt；published claims/citations可追溯到Evidence Pack item與gateway audit。

#### Open Questions
- Q01: Evidence item保存inline typed content或content-addressed reference；需依payload size、privacy與audit retention決定。
- Q02: retrieval plan由deterministic rules、受限planner LLM或兩者混合；無論選擇，planner只能輸出gateway request plan且不持有credentials。
- Q03: citation coverage是所有factual claims強制，或只針對defined claim types；需定義Output Validator可測規則。
