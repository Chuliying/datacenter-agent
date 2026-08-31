# Local Falcon permissions stub

This document provides a local-only permissions endpoint for exercising the
identity boundary. It returns one ordinary permission item and follows the
permissions contract in the specified Falcon guide. The
[Falcon guide](https://github.com/HDRenewables/falcon-backend/blob/c5d0408b5e1847bf165e8c5f5859ef61da93e02d/docs/External_Delegated_API_Integration.md)
documents `external_auth.*` tables for token acquisition and refresh; those
codes do not apply to the permissions endpoint, whose invalid/expired/revoked
access token response is generic HTTP 401.

## What the stub serves

The stub listens on `127.0.0.1:8787` by default and serves:

```text
GET /api/auth/me/permissions
Authorization: Bearer dev-user-token
```

The valid local token receives HTTP 200 with the same top-level shape expected
by the permissions contract:

```json
{
  "user_id": 123,
  "roles": [
    {
      "id": 10,
      "code": "viewer",
      "name": "檢視者",
      "default_pages": ["/dashboard/"]
    }
  ],
  "permissions": [
    {
      "code": "starcharger.finance",
      "name": "Finance",
      "category": "starcharger",
      "page_path": "/finance",
      "can_read": true,
      "can_write": false
    }
  ]
}
```

Any other bearer value receives HTTP 401. The stub logs only the request
method and path; it never logs the bearer token.

## Start it

Prerequisite: Python 3. No third-party package is required.

```bash
export FALCON_STUB_HOST=127.0.0.1
export FALCON_STUB_PORT=8787
python3 - <<'PY'
import json
import os
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

HOST = os.environ.get("FALCON_STUB_HOST", "127.0.0.1")
PORT = int(os.environ.get("FALCON_STUB_PORT", "8787"))


class Handler(BaseHTTPRequestHandler):
    def _send(self, status, body):
        payload = json.dumps(body).encode("utf-8")
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(payload)))
        self.end_headers()
        self.wfile.write(payload)

    def do_GET(self):  # noqa: N802 - required by BaseHTTPRequestHandler
        if self.path != "/api/auth/me/permissions":
            self._send(404, {"detail": "not found"})
            return
        if self.headers.get("Authorization") != "Bearer dev-user-token":
            self._send(401, {"detail": "unauthorized"})
            return
        self._send(
            200,
            {
                "user_id": 123,
                "roles": [
                    {
                        "id": 10,
                        "code": "viewer",
                        "name": "檢視者",
                        "default_pages": ["/dashboard/"],
                    }
                ],
                "permissions": [
                    {
                        "code": "starcharger.finance",
                        "name": "Finance",
                        "category": "starcharger",
                        "page_path": "/finance",
                        "can_read": True,
                        "can_write": False,
                    }
                ],
            },
        )

    def log_message(self, format, *args):  # noqa: A002 - stdlib hook signature
        print("falcon-stub", self.command, self.path)


ThreadingHTTPServer((HOST, PORT), Handler).serve_forever()
PY
```

Leave that terminal running. In a second terminal, verify the one authorized
request:

```bash
curl --fail-with-body -sS \
  -H 'Authorization: Bearer dev-user-token' \
  http://127.0.0.1:8787/api/auth/me/permissions
```

The response must contain `user_id`, `roles`, and `permissions`. Stop the
stub with `Ctrl-C` when finished.

## Point local configuration at it

Copy `.env.example` to an untracked `.env`, then set a local pepper of at
least 32 bytes:

```bash
cp .env.example .env
# Generate the local pepper rather than copying a literal from this file. A pasted value is
# copy-pasteable into a real deployment, and a known pepper makes every `actor_key` computable
# from its `user_id` — which is the one property the pseudonymisation exists to provide. The
# rotation section below gives the same instruction; this command follows it.
export ACTOR_KEY_PEPPER="$(openssl rand -hex 32)"
export FALCON_API_BASE_URL='http://127.0.0.1:8787'
```

The checkout reads the identity URL from `[identity].base_url` in
`config/config.toml`, with `FALCON_API_BASE_URL` taking precedence when set.
The runtime always appends `/api/auth/me/permissions` to the resolved base URL.

The whole service also needs its existing local prerequisites, including
`GLOBAL_TOKEN`, `DATACENTER_MCP_URL`, and the OpenRouter settings. The outer
global limiter is enabled in the shipped config; do not disable it when
testing the identity layer.

The stub endpoint smoke above is executable now. To exercise the runtime
identity boundary, start the service with the exported variables above and
send both bearer headers: the service `Authorization` header and
`X-Falcon-Authorization: Bearer dev-user-token`.

## Pepper rotation runbook

1. Generate a replacement locally, for example with `openssl rand -hex 32`.
   Do not paste the output into the repository, shell history shared with
   others, logs, or this document.
2. Store the replacement in the deployment secret manager under
   `ACTOR_KEY_PEPPER`. Keep the previous value available for the rollback
   window.
3. Roll the service instances with the new value and verify each instance
   passes boot validation. `ACTOR_KEY_PEPPER` must be present, non-empty, and
   at least 32 bytes.
4. Confirm logs contain neither pepper value. Actor keys keep the `v1:`
   format but change for every user after the pepper changes; never log the
   full key to prove this.
5. After every instance has restarted and the smoke check is green, revoke
   the old secret. If a rollback is needed, restore the old secret and roll
   all instances consistently.

Rotation invalidates derived actor keys across the process boundary. Existing
naively stored user identifiers, audit records, and the actor-key algorithm
version are not rewritten by this runbook; coordinate any persistent cache or
session migration separately before rotating in production.

## Safety notes

The stub is for loopback development only. It contains a dummy token and a
dummy permission; do not expose its port or use either value outside local
testing. Do not add the local `.env` to git.
