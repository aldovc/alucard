#!/usr/bin/env bash
# Test connectivity to the Anthropic Claude API.
# Usage: ANTHROPIC_API_KEY=<key> ./test-claude.sh [model]
set -euo pipefail

API_KEY="${ANTHROPIC_API_KEY:-}"
MODEL="${1:-claude-sonnet-5}"
BASE_URL="https://api.anthropic.com/v1"
ANTHROPIC_VERSION="${ANTHROPIC_VERSION:-2023-06-01}"

if [[ -z "$API_KEY" ]]; then
  echo "ERROR: ANTHROPIC_API_KEY is not set." >&2
  echo "Usage: ANTHROPIC_API_KEY=<key> $0 [model]" >&2
  exit 1
fi

echo "Testing Claude connectivity..."
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
  -X POST "$BASE_URL/messages" \
  -H "x-api-key: $API_KEY" \
  -H "anthropic-version: $ANTHROPIC_VERSION" \
  -H "Content-Type: application/json" \
  -d "$PAYLOAD") || {
  echo "ERROR: Request to Claude API failed (check your API key, model name, and network)." >&2
  exit 1
}

COMPACT_RESPONSE=$(printf '%s' "$RESPONSE" | tr -d '\n')
CONTENT=$(printf '%s' "$COMPACT_RESPONSE" | sed -n 's/.*"text"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p')
INPUT_TOKENS=$(printf '%s' "$COMPACT_RESPONSE" | sed -n 's/.*"input_tokens"[[:space:]]*:[[:space:]]*\([0-9][0-9]*\).*/\1/p')
OUTPUT_TOKENS=$(printf '%s' "$COMPACT_RESPONSE" | sed -n 's/.*"output_tokens"[[:space:]]*:[[:space:]]*\([0-9][0-9]*\).*/\1/p')

echo "Response   : ${CONTENT:-<no text content parsed>}"
echo "Tokens     : ${INPUT_TOKENS:-?} input / ${OUTPUT_TOKENS:-?} output"
echo ""
echo "OK - Claude API is reachable."
