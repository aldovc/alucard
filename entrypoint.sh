#!/bin/bash
set -euo pipefail

# The checkout the agent is about to work in. An empty or missing one means
# the harness's worktree vanished between clone and mount — another run
# deleted it (#92) — and an agent started here would spend its budget on a
# repository that is not there, then report the absence as its finding. Exit
# with a code the harness maps to "get a fresh worktree" instead. The cwd is
# checked rather than a fixed path because preflight runs in a subdirectory
# of the checkout, and so the same test works on a host without /work. Keep
# the number in step with NO_WORKTREE_RC in the alucard script.
if ! git -c safe.directory='*' rev-parse --git-dir >/dev/null 2>&1; then
  echo "alucard: $PWD is not inside a git checkout — the worktree is missing; not starting the agent" >&2
  exit 78
fi

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
# Beside the git config rather than at a fixed /tmp path, so the entrypoint
# can run under a test with every path redirected.
GIT_CREDENTIALS="$(dirname "$GIT_CONFIG_GLOBAL")/git-credentials"
git config --global credential.helper "store --file $GIT_CREDENTIALS"
printf 'https://x-access-token:%s@github.com\n' "$GITHUB_TOKEN" > "$GIT_CREDENTIALS"

# Codex CLI's Responses websocket ignores OPENAI_API_KEY and only reads
# ~/.codex/auth.json. Pre-populate it via `codex login --with-api-key`.
if [ -n "${OPENAI_API_KEY:-}" ] && command -v codex >/dev/null 2>&1; then
  printenv OPENAI_API_KEY | codex login --with-api-key >/dev/null 2>&1 || true
fi

exec "$@"
