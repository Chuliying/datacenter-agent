---
output: docs/work/agentgateway-openai-endpoint/qa-plan.md
stage: qa-plan
slug: agentgateway-openai-endpoint
---

# OpenAI 相容 `/v1/chat/completions` 端點 QA 方案

**PRD**: `docs/work/agentgateway-openai-endpoint/prd.md` v1
**Spec**: `docs/work/agentgateway-openai-endpoint/spec.md`
**Capabilities**: `has_ui=false` · `has_api=true` · `typed_contracts=true` · `has_e2e=false`

> 可測試性(Challenge Spec):`messages→AgentRequest` 映射、usage 累加、chunk 切塊皆為純函式(L2 易測);handler 走 prelude+pipeline 用受控替身測 L4。無不可測相依 → 不退回 spec。
> 測試策略研究:Skipped — repo 既有測試慣例(`tests/`、`src/runtime/turn.rs` 測試)+ OpenAI SSE 為標準格式,evidence 充分。

## 1. Traceability

| Source | Behavior / Risk | Test ID | Level | Evidence |
|--------|-----------------|---------|-------|----------|
| AC-1 | 非串流回 OpenAI `choices[].message.content` + `usage` | TC-I01 | L4 | `cargo test` 整合 + 受控替身 |
| AC-2 | 串流逐塊 `delta.content`,末塊 `finish_reason:"stop"`,收到 `[DONE]` | TC-I02 | L4 | `cargo test` 整合(SSE 解析) |
| AC-3 | `/agent/stream` 及既有端點行為不變(回歸) | TC-I03 | L4 | 既有 test suite 全綠 |
| AC-4 | 規格書 C-3 curl(串流/非串流)通過 | TC-I01 + TC-I02 | L4 | 手動 curl + 整合覆蓋 |
| AC-5 | governance 與 `/agent/stream` 一致(injection 被 refuse) | TC-I04 | L4 | `cargo test` 整合 |
| ERR1 | RUNTIME_ENABLED=false → 503 | TC-ERR01 | L4 | `cargo test` |
| ERR2 | messages 空/無 user/格式錯 → 400 | TC-U03/04/05 + TC-ERR02 | L2+L4 | `cargo test` |
| ERR3 | prompt > 4000 → 400 | TC-ERR03 | L4 | `cargo test` |
| ERR4 | bearer 失敗 → 418 | TC-ERR04 | L4 | `cargo test` |
| ERR5 | 上游 LLM/MCP capability 失敗 → 502 | TC-ERR05 | L4 | `cargo test` + fake 失敗替身 |
| spec: `messages→AgentRequest` | 映射正確性 | TC-U01/U02 | L2 | `cargo test` |
| spec: usage 累加 | 多筆 `AgentEvent::Usage` sum | TC-U06 | L2 | `cargo test` |
| spec: chunk 切塊(D1 偽串流) | 完整答案 → chunk 序列 | TC-U07 | L2 | `cargo test` |

## 2. Test Cases

### TC-U01: messages→AgentRequest 單輪映射
**Source**: spec Contracts / AC-1 · **Level**: L2 · **Applicability**: always
**Precondition**: `messages=[{role:"user",content:"Q"}]`
```text
Given 單一 user message
When map_request(messages)
Then AgentRequest{prompt:"Q", history:[]}
```
**Implementation notes**: `src/server/openai.rs` 純函式;`tests/` 或模組內 `#[test]`

### TC-U02: 多輪配對成 history
**Source**: spec 映射規則 · **Level**: L2
```text
Given [user:"A", assistant:"a", user:"B"]
When map_request
Then prompt:"B", history:[{user_prompt:"A", model_response:"a"}]
```

### TC-U03: 只有 system → 400
**Source**: ERR2 · **Level**: L2
```text
Given [{role:"system",content:"..."}]（無 user）
When map_request
Then Err(invalid_request)  // 對映 400
```

### TC-U04: 空 messages → 400
**Source**: ERR2 · **Level**: L2
```text
Given messages=[]
When map_request
Then Err(invalid_request)
```

### TC-U05: 相鄰同 role → 400
**Source**: ERR2 / spec 硬性規則 · **Level**: L2
```text
Given [user:"A", user:"B"]
When map_request
Then Err(invalid_request)
```

### TC-U06: usage 累加
**Source**: spec D3 · **Level**: L2
```text
Given [Usage{p:10,c:5,t:15}, Usage{p:8,c:4,t:12,reasoning:3}]
When accumulate_usage
Then {prompt:18, completion:9, total:27, reasoning:3}
```

### TC-U07: chunk 切塊格式（D1 偽串流）
**Source**: spec D1 / AC-2 · **Level**: L2
```text
Given 完整答案字串 "hello world"
When build_chunks(answer)
Then 首塊 delta.role="assistant";中間塊 delta.content 累加=原字串;
     末塊 delta={} 且 finish_reason="stop";序列後接 [DONE] sentinel
```

### TC-I01: 非串流端到端
**Source**: AC-1 / AC-4 · **Level**: L4 · **Precondition**: runtime 啟用 + 受控 LLM/MCP 替身 + 有效 bearer
```text
Given POST /v1/chat/completions {stream:false} 有效查詢
When 呼叫
Then 200 + choices[0].message.content 非空 + object="chat.completion" + usage 存在
```
**Implementation notes**: 受控替身回放固定 pipeline 結果;斷言 OpenAI schema

### TC-I02: 串流端到端
**Source**: AC-2 / AC-4 · **Level**: L4
```text
Given POST {stream:true}
When 呼叫
Then Content-Type text/event-stream;逐塊 data:{chunk,delta.content};
     累加=完整答案;末塊 finish_reason="stop";最後 data: [DONE]
```

### TC-I03: 既有端點回歸
**Source**: AC-3 · **Level**: L4
```text
Given 既有 test suite（/agent/stream、/insight*、/report* 等）
When cargo test
Then 全數通過（新端點未改動既有行為）
```

### TC-I04: governance 一致（injection refuse）
**Source**: AC-5 · **Level**: L4
```text
Given injection 輸入（走 plan_stream_turn prelude）
When POST /v1/chat/completions
Then 與 /agent/stream 一致地 refuse（回覆 refusal copy，非正常答案）
```

### TC-ERR01: runtime 未啟用
**Source**: ERR1 · **Level**: L4
```text
Given RUNTIME_ENABLED=false
When POST /v1/chat/completions
Then 503 + OpenAI 風格 error body
```

### TC-ERR02: 非法 JSON / 缺欄位
**Source**: ERR2 · **Level**: L4
```text
Given 非法 body 或缺 messages
When POST
Then 400 + OpenAI error（invalid_request_error）
```

### TC-ERR03: prompt 超長
**Source**: ERR3 · **Level**: L4
```text
Given prompt 長度 > 4000
When POST
Then 400
```

### TC-ERR04: auth 失敗
**Source**: ERR4 · **Level**: L4
```text
Given 缺少或錯誤 bearer
When POST
Then 418（沿用 require_bearer，D6）
```

### TC-ERR05: 上游能力失敗
**Source**: ERR5 · **Level**: L4
```text
Given LLM/MCP 替身回傳 Capability 失敗
When POST
Then 502 + OpenAI error
```

## 3. Boundary Tests

| Test ID | Boundary | Expected behavior | Source |
|---------|----------|-------------------|--------|
| TC-B01 | prompt 長度 = 4000 | 通過（邊界內） | FR1 / D4 |
| TC-B02 | prompt 長度 = 4001 | 400 | ERR3 / D4 |
| TC-B03 | 單一 user message（無歷史） | 正常，history=[] | AC-1 |
| TC-B04 | system message 存在 | 被忽略，不影響結果（D5） | FR2 |
| TC-B05 | pipeline 回空答案 | chunk 序列仍含 finish_reason + [DONE]，content 為空 | AC-2 |

## 4. Capability-specific Tests

| Capability | Required tests | Evidence |
|------------|----------------|----------|
| `has_ui=false` | — | N/A |
| `has_api=true` | L4 整合契約(受控替身):TC-I01..04、TC-ERR01..05 | `cargo test`（整合測試） |
| `typed_contracts=true` | L1 typecheck | `cargo check`;`cargo clippy -- -D warnings` |
| `has_e2e=false` | — | N/A |

## 5. Test Matrix

| Level | Scope | Count | Command / Evidence |
|-------|-------|------:|--------------------|
| L1 Static | OpenAI DTO + 映射契約編譯 | — | `cargo check` / `cargo clippy -- -D warnings` |
| L2 Unit | 映射 / usage 累加 / chunk 切塊 / 邊界 | 7 (+TC-B) | `cargo test` |
| L3 Component | N/A | 0 | N/A（has_ui=false） |
| L4 Integration | 端到端 handler（受控替身）+ 錯誤場景 | 9 | `cargo test` |
| L5 E2E | N/A | 0 | N/A（has_e2e=false） |

## 6. Mock / Fixture Plan

| Dependency | Strategy | Contract source |
|------------|----------|-----------------|
| LLM(OpenRouter/streaming+buffered) | 受控替身(fake LLM client,回放固定 delta/usage frames) | spec Contracts / `src/agent/llm.rs` |
| MCP tools | 受控替身(fake tool 結果) | api-reference / `src/mcp_client.rs` |
| runtime prelude | 真實 `plan_stream_turn`(注入 fake LLM/MCP);injection/長度用真實 guardrails | spec Data Flow / `src/runtime/turn.rs` |

> Mock 隔離:整合測試不得連真實 OpenRouter/MCP;替身資料須符合 spec 契約。真實整合(Gate L4 對真服務)由平台端 gateway 對接時以規格書 C-3 curl 驗證。

## Machine traceability gate

- [x] 每個 PRD AC(AC-1–AC-5)對應至少一個 TC。
- [x] 每個 PRD ERR(ERR1–ERR5)對應至少一個錯誤/回復測試。
- [x] 每個 TC 有 source ID 與 level。
- [x] Capability-disabled 章節標 N/A(L3/L5),未填假資產。
- [x] Mock/fixture 資料遵循 spec 契約。
- [x] 代表性測試有可執行命令(`cargo test` / `cargo check`)。

## Related Documents

| Document | Link |
|----------|------|
| PRD | `docs/work/agentgateway-openai-endpoint/prd.md` |
| Spec | `docs/work/agentgateway-openai-endpoint/spec.md` |
| Brainstorm | `docs/work/agentgateway-openai-endpoint/brainstorm.md` |
