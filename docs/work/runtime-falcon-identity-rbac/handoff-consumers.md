# Consumer handoff — Falcon identity and RBAC

This handoff is for the downstream `falcon-client` and `agentgateway` consumers. The runtime
expects the service bearer and the delegated Falcon access token separately; it does not perform
Falcon login or refresh itself.

## Required request change

For both prompt routes, send:

```text
Authorization: Bearer <GLOBAL_TOKEN>
X-Falcon-Authorization: Bearer <FALCON_ACCESS_TOKEN>
```

The Falcon token is used only by the runtime for
`GET {FALCON_API_BASE_URL}/api/auth/me/permissions`. The guide documents that endpoint's `401` only
generically, but the live endpoint returns an `error_code` in the body, so the runtime branches on
it and hands you **two** distinct 401 codes. You never see the upstream code itself — branch on the
runtime's own `code`. Separately, the `external_auth.*` codes in the guide belong to
external-login/refresh and must not be used to branch permissions failures; they are a different
namespace from the permissions endpoint's `auth.*`.

## C1–C9 consumer checklist

| ID | Consumer location / concern | Required change | Verification |
|---|---|---|---|
| C1 | falcon-client API wrapper | Preserve the delegated access token and attach it as `X-Falcon-Authorization` when calling either prompt route. | Network inspection in a local/mock environment; never log the value. |
| C2 | stream route / request builder | Forward the header on `/agent/stream`; do not substitute the service `Authorization` token. | Request fixture contains both distinct headers. |
| C3 | `agentErrorInfo` or equivalent error parser | Read the runtime `code` field before choosing a retry or display path. | Fixture tests for every row in the matrix below. |
| C4 | token refresh handler | **Only `identity.token_refreshable`** may spend the existing silent-refresh budget, and it may retry the original request once. `identity.token_terminal` must **never** enter the refresh path — refreshing returns a token for the same deactivated, revoked, or unknown account, which is how an infinite refresh loop is built. | Second `401` after a refresh is terminal and asks for sign-in; a `identity.token_terminal` response triggers zero refresh attempts. |
| C5 | React/native SSE consumer | Parse `error.code` and `refusal.code`; handle `refusal` as a terminal policy answer, not transport failure. After `clear`, retain the following full `token` as authoritative. | SSE fixture covers refusal, report degradation, and final `done`. |
| C6 | agentgateway `/v1/chat/completions` caller | Send both headers and preserve the OpenAI nested error `code`. | 401/503 mock responses route to the intended client state. |
| C7 | generated schemas / DTOs | Add optional `code` to standard and OpenAI error types; add the native SSE `refusal` event. | Backward-compatible deserialization of old payloads remains green. |
| C8 | telemetry / redaction | Keep Falcon access tokens out of logs, traces, audit, cache keys, and error bodies. Actor keys are opaque and may be used for correlation. | Secret scan plus a redaction assertion in consumer telemetry tests. |
| C9 | rollout/config/mock | Configure `FALCON_API_BASE_URL` and verify local Falcon stub + service bearer before enabling traffic. | One authorized local request succeeds; invalid token and unavailable stub are distinct. |

## Failure handling matrix

| Runtime code | Status / shape | Consumer action |
|---|---|---|
| `auth.service_token_invalid` | Standard `418`; `/v1` `401` | Fix the service credential. Do not refresh the user token. |
| `identity.header_missing` | `401` | Fix request construction. Do not refresh. |
| `identity.token_refreshable` | `401` | Spend at most one existing silent-refresh attempt, resend once, then stop and require login. |
| `identity.token_terminal` | `401` | Terminal. **Do not refresh.** Require sign-in, or tell the user to contact an administrator — the account is deactivated, the token is revoked, or the user is unknown. |
| `identity.upstream_unavailable` | `503` | Back off and retry according to the consumer's transient-failure policy. Do not treat as a token refresh signal. |
| `authz.insufficient` | `200` refusal (`x_refusal_code` for OpenAI; `refusal` SSE event) | Render the terminal refusal. Do not retry automatically. |
| `rate_limit.global` | `429` + `Retry-After` | Wait for the advertised delay; avoid a tight retry loop. |
| `rate_limit.actor` | `429` + `Retry-After` | Wait for the advertised delay for that actor. |
| `request.invalid` | `400` | Fix the request; do not retry unchanged. |
| `upstream.error` / `server.unavailable` / `server.internal` | `5xx` | Apply the consumer's transient retry policy, with backoff. |

Partial report authorization is a successful response, not an error. The final report declares the
omitted topics; consumers should display that declaration and should not try to reconstruct omitted
data from an older transcript.

## Three-stage rollout

1. Deploy consumer changes that send `X-Falcon-Authorization`, parse `code`, and implement the
   one-refresh budget. Keep the current runtime binary serving traffic while header coverage is
   observed.
2. Point a local/staging runtime at the Falcon permissions endpoint or the checked-in local stub;
   verify success, generic permissions `401`, unavailable `503`, and the two rate-limit codes.
3. Enable the identity-enforcing runtime binary for production traffic only after C1–C8 are green.
   Keep the outer global limiter enabled and remove any consumer fallback that submits a request
   without the delegated header.

## Source of truth

The permissions transport and response shape are taken from the specified Falcon guide commit:

<https://github.com/HDRenewables/falcon-backend/blob/c5d0408b5e1847bf165e8c5f5859ef61da93e02d/docs/External_Delegated_API_Integration.md>

The runtime-side contract and code list are maintained in `prd.md`, `spec.md`, and the two endpoint
reference documents beside this file. No credential, token, or secret value belongs in this handoff.
