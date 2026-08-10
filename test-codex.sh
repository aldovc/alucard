#!/usr/bin/env bash
# Test connectivity to the OpenAI API used by the Codex provider.
# Usage: OPENAI_API_KEY=<key> ./test-codex.sh [model]
set -euo pipefail

API_KEY="${OPENAI_API_KEY:-}"
MODEL="${1:-${ALUCARD_CODEX_MODEL:-gpt-5.6-terra}}"
BASE_URL="https://api.openai.com/v1"

if [[ -z "$API_KEY" ]]; then
  echo "ERROR: OPENAI_API_KEY is not set." >&2
  echo "Usage: OPENAI_API_KEY=<key> $0 [model]" >&2
  exit 1
fi

echo "Testing Codex/OpenAI connectivity..."
echo "  Base URL : $BASE_URL"
echo "  Model    : $MODEL"
echo ""

PAYLOAD=$(cat <<EOF
{
  "model": "$MODEL",
  "input": "Say hello",
  "max_output_tokens": 16
}
EOF
)

RESPONSE=$(curl -sf \
  --max-time 15 \
  -X POST "$BASE_URL/responses" \
  -H "Authorization: Bearer $API_KEY" \
  -H "Content-Type: application/json" \
  -d "$PAYLOAD") || {
  echo "ERROR: Request to OpenAI API failed (check your API key, model name, and network)." >&2
  exit 1
}

COMPACT_RESPONSE=$(printf '%s' "$RESPONSE" | tr -d '\n')
CONTENT=$(printf '%s' "$COMPACT_RESPONSE" | sed -n 's/.*"text"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p')
INPUT_TOKENS=$(printf '%s' "$COMPACT_RESPONSE" | sed -n 's/.*"input_tokens"[[:space:]]*:[[:space:]]*\([0-9][0-9]*\).*/\1/p')
OUTPUT_TOKENS=$(printf '%s' "$COMPACT_RESPONSE" | sed -n 's/.*"output_tokens"[[:space:]]*:[[:space:]]*\([0-9][0-9]*\).*/\1/p')

echo "Response   : ${CONTENT:-<no text content parsed>}"
echo "Tokens     : ${INPUT_TOKENS:-?} input / ${OUTPUT_TOKENS:-?} output"
echo ""
echo "OK - Codex/OpenAI API is reachable."
