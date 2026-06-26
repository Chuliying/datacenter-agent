#!/usr/bin/env bash
set -euo pipefail

: "${AGENT_BASE_URL:?set AGENT_BASE_URL to the staging datacenter-agent base URL}"
: "${AGENT_TOKEN:?set AGENT_TOKEN to the staging bearer token}"

base_url="${AGENT_BASE_URL%/}"
session_id="${SMOKE_SESSION_ID:-staging-smoke-session}"
agent_prompt="${SMOKE_AGENT_PROMPT:-Revenue overview for the last three months?}"
stream_prompt="${SMOKE_STREAM_PROMPT:-Compare station contribution next.}"

agent_payload=$(cat <<JSON
{
  "prompt": "$agent_prompt",
  "history": [],
  "session_id": "$session_id",
  "option_id": "revenue.monthly"
}
JSON
)

agent_response=$(curl -fsS "$base_url/agent" \
  -H "authorization: Bearer $AGENT_TOKEN" \
  -H "content-type: application/json" \
  -d "$agent_payload")

case "$agent_response" in
  *'"user_prompt"'*'"model_response"'*) ;;
  *)
    echo "agent smoke failed: /agent response missing user_prompt/model_response" >&2
    echo "$agent_response" >&2
    exit 1
    ;;
esac

stream_payload=$(cat <<JSON
{
  "prompt": "$stream_prompt",
  "history": [{"user_prompt":"$agent_prompt","model_response":"smoke prior"}],
  "session_id": "$session_id",
  "option_id": "station.ranking"
}
JSON
)

stream_output="$(mktemp)"
trap 'rm -f "$stream_output"' EXIT

curl -fsS -N "$base_url/agent/stream" \
  -H "authorization: Bearer $AGENT_TOKEN" \
  -H "accept: text/event-stream" \
  -H "content-type: application/json" \
  -d "$stream_payload" > "$stream_output"

if ! grep -q '^data:' "$stream_output"; then
  echo "agent smoke failed: /agent/stream emitted no SSE data frames" >&2
  cat "$stream_output" >&2
  exit 1
fi

if grep '^data:' "$stream_output" \
  | grep -Ev '"event":"(intent.resolved|token|done|error|clear)"|"event": "(intent.resolved|token|done|error|clear)"' >/dev/null; then
  echo "agent smoke failed: /agent/stream emitted an unexpected external event" >&2
  cat "$stream_output" >&2
  exit 1
fi

echo "staging smoke passed: /agent response shape and /agent/stream event set are compatible"
