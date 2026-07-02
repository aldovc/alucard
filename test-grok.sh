#!/usr/bin/env bash
# Test connectivity to the xAI Grok API.
# Usage: XAI_API_KEY=<key> ./test-grok.sh [model]
set -euo pipefail

API_KEY="${XAI_API_KEY:-}"
MODEL="${1:-grok-4.3}"
BASE_URL="https://api.x.ai/v1"

if [[ -z "$API_KEY" ]]; then
  echo "ERROR: XAI_API_KEY is not set." >&2
  echo "Usage: XAI_API_KEY=<key> $0 [model]" >&2
  exit 1
fi

echo "Testing Grok connectivity..."
echo "  Base URL : $BASE_URL"
echo "  Model    : $MODEL"
echo ""

PAYLOAD=$(cat <<EOF
{
  "model": "$MODEL",
  "messages": [{"role": "user", "content": "Say hello"}],
  "max_tokens": 10
}
EOF
)

RESPONSE=$(curl -sf \
  --max-time 15 \
  -X POST "$BASE_URL/chat/completions" \
  -H "Authorization: Bearer $API_KEY" \
  -H "Content-Type: application/json" \
  -d "$PAYLOAD") || {
  echo "ERROR: Request to Grok API failed (check your API key, model name, and network)." >&2
  exit 1
}

CONTENT=$(echo "$RESPONSE" | grep -o '"content":"[^"]*"' | head -1 | sed 's/"content":"//;s/"$//')
PROMPT_TOKENS=$(echo "$RESPONSE" | grep -o '"prompt_tokens":[0-9]*' | grep -o '[0-9]*')
COMPLETION_TOKENS=$(echo "$RESPONSE" | grep -o '"completion_tokens":[0-9]*' | grep -o '[0-9]*')

echo "Response   : $CONTENT"
echo "Tokens     : ${PROMPT_TOKENS} prompt / ${COMPLETION_TOKENS} completion"
echo ""
echo "OK — Grok API is reachable."
