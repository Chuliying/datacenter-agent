#!/usr/bin/env bash
set -euo pipefail

# Staging smoke test for the two prompt endpoints.
#
# `POST /agent`, `/insight`, `/insight/stream`, `/report` and `/report/stream` were retired
# (they return 404), so this script exercises what actually serves prompts today:
#   - non-streaming: POST /v1/chat/completions   (OpenAI-compatible, agentgateway Path C)
#   - streaming:     POST /agent/stream          (native SSE, intent-routed sub-agent pipeline)

: "${AGENT_BASE_URL:?set AGENT_BASE_URL to the staging datacenter-agent base URL}"
: "${AGENT_TOKEN:?set AGENT_TOKEN to the staging bearer token}"

base_url="${AGENT_BASE_URL%/}"
session_id="${SMOKE_SESSION_ID:-staging-smoke-session}"
agent_prompt="${SMOKE_AGENT_PROMPT:-Revenue overview for the last three months?}"
stream_prompt="${SMOKE_STREAM_PROMPT:-Compare station contribution next.}"

# ──── non-streaming: /v1/chat/completions ────
#
# The prompt must resolve to a known intent, or the answer policy refuses it with `off_scope`
# (still HTTP 200). The refusal is a valid response, so this only asserts the OpenAI envelope.

chat_payload=$(cat <<JSON
{
  "model": "datacenter-agent",
  "messages": [{"role": "user", "content": "$agent_prompt"}],
  "stream": false
}
JSON
)

chat_response=$(curl -fsS "$base_url/v1/chat/completions" \
  -H "authorization: Bearer $AGENT_TOKEN" \
  -H "content-type: application/json" \
  -d "$chat_payload")

case "$chat_response" in
  *'"object"'*'"chat.completion"'*'"choices"'*) ;;
  *)
    echo "agent smoke failed: /v1/chat/completions response is not a chat.completion envelope" >&2
    echo "$chat_response" >&2
    exit 1
    ;;
esac

# ──── streaming: /agent/stream ────

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

# The external frame set is the 9 `StreamFrame` variants in src/server/dto.rs. The sub-agent
# pipeline adds stage / tool_call / tool_args / usage on top of the original five.
known_events='intent.resolved|stage|token|tool_call|tool_args|usage|clear|done|error'

if grep '^data:' "$stream_output" \
  | grep -Ev "\"event\":\"($known_events)\"|\"event\": \"($known_events)\"" >/dev/null; then
  echo "agent smoke failed: /agent/stream emitted an unexpected external event" >&2
  cat "$stream_output" >&2
  exit 1
fi

# A clean turn must terminate; `done` is the only clean terminal frame.
if ! grep '^data:' "$stream_output" | grep -Eq '"event":\s*"done"'; then
  echo "agent smoke failed: /agent/stream did not terminate with a done frame" >&2
  cat "$stream_output" >&2
  exit 1
fi

echo "staging smoke passed: /v1/chat/completions envelope and /agent/stream event set are compatible"
