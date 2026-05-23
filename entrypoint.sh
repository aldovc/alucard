#!/bin/bash
set -euo pipefail

export GIT_CONFIG_GLOBAL="${GIT_CONFIG_GLOBAL:-/tmp/alucard-gitconfig}"
export GH_CONFIG_DIR="${GH_CONFIG_DIR:-/tmp/gh}"

mkdir -p "$(dirname "$GIT_CONFIG_GLOBAL")" "$GH_CONFIG_DIR"

# Block Bash tool calls with dangerouslyDisableSandbox=true via a PreToolUse hook.
# That flag deletes the shell CWD (/work) on use, wedging every subsequent tool
# call for the rest of the iteration. The container is already sandboxed by the
# host docker config; the flag enables nothing useful for any agent role.
mkdir -p "$HOME/.claude"
cat > "$HOME/.claude/settings.json" <<'JSON'
{
  "hooks": {
    "PreToolUse": [
      {
        "matcher": "Bash",
        "hooks": [
          {
            "type": "command",
            "command": "jq -e '.tool_input.dangerouslyDisableSandbox == true' >/dev/null 2>&1 && { echo 'alucard: dangerouslyDisableSandbox is forbidden inside the harness container — it deletes the shell CWD and wedges the iteration. Use a regular Bash call instead.' >&2; exit 2; } || exit 0"
          }
        ]
      }
    ]
  }
}
JSON

git config --global user.name "alucard-bot"
git config --global user.email "alucard-bot@users.noreply.github.com"
git config --global --add safe.directory /work
git config --global --add safe.directory '*'

# gh uses GITHUB_TOKEN from env automatically; wire the same token into git
# so `git push` works without a separate credential store.
git config --global credential.helper "store --file /tmp/git-credentials"
printf 'https://x-access-token:%s@github.com\n' "$GITHUB_TOKEN" > /tmp/git-credentials

# Codex CLI's Responses websocket ignores OPENAI_API_KEY and only reads
# ~/.codex/auth.json. Pre-populate it via `codex login --with-api-key`.
if [ -n "${OPENAI_API_KEY:-}" ] && command -v codex >/dev/null 2>&1; then
  printenv OPENAI_API_KEY | codex login --with-api-key >/dev/null 2>&1 || true
fi

exec "$@"
