# `POST /agent` — 現況契約

> ← [Endpoints](./index.md)  
> **Source**：[`src/server/handler.rs`](../../../src/server/handler.rs) `agent` / `agent_inner` / `agent_inner_runtime`；[`src/server/dto.rs`](../../../src/server/dto.rs) `AgentRequest` / `AgentResponse`

## Request

```json
{
  "prompt": "本月充電量？",
  "history": [],
  "session_id": "abc",
  "option_id": "charging.monthly"
}
```

| Field | Required | Current behavior |
|---|---|---|
| `prompt` | yes | Unicode char count；cap 依 path 不同 |
| `history` | no | serde default `[]` |
| `session_id` | no | runtime memory key；legacy 不使用 |
| `option_id` | no | runtime intent classification signal；legacy 不使用 |

## Response

成功回 `200`：

```json
{
  "user_prompt": "本月充電量？",
  "model_response": "...",
  "intent": "charging"
}
```

## Legacy / runtime 差異

| Item | Legacy（default） | Runtime（`RUNTIME_ENABLED=true/1`） |
|---|---|---|
| prompt cap | 2000 | `thresholds.input.max_prompt_chars`，目前 4000 |
| validation | handler `validate_prompt` | orchestrator `validate_prompt` |
| orchestration | `llm_connector::generate` | `run_agent_turn` with no-op emit |
| intent | always `unknown` | Final resolved；Refused/Aborted unknown |
| memory/audit/policy | no | enabled according to runtime config/wiring |

### Runtime outcome mapping

| Outcome | HTTP | Body |
|---|---|---|
| `Final` | 200 | answer + resolved intent |
| `Refused` | 200 | refusal copy + unknown intent |
| `Aborted` | 200 | partial response + unknown intent |
| `Error{status:400}` | 400 | error code envelope |
| `Error{status:502}` | 502 | error code envelope |
| runtime internal `Err` | 400/502/503 via `runtime_error_to_app_error` | raw error string envelope |

## Error notes

- malformed/missing JSON 由 `JsonRejection` 統一包成 400。
- legacy upstream error 的完整 error chain 目前會放進 502 body。
- runtime structural error 是 stable-ish code；其他 runtime error 可能回 raw error text。
- provider stream natural EOF 可能被 connector 誤判為成功，詳見 [llm_connector](../modules/llm-connector.md)。

## Example

```bash
curl -s http://localhost:8080/agent \
  -H "Authorization: Bearer $GLOBAL_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"prompt":"本月充電量？"}'
```

## Target state

PRD 要求保留 legacy 2000、runtime config 4000 的明確差異，同時讓同一路徑 REST/SSE 的 validation timing 與 status 一致；見 [PRD FR-001/FR-002](../prd.md) 與 [code change plan](../../../.agent/artifacts/plan/2026-06-29-runtime-correctness/implementation.md)。
