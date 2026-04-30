#!/bin/bash
set -euo pipefail

export GIT_CONFIG_GLOBAL="${GIT_CONFIG_GLOBAL:-/tmp/alucard-gitconfig}"
export GH_CONFIG_DIR="${GH_CONFIG_DIR:-/tmp/gh}"

mkdir -p "$(dirname "$GIT_CONFIG_GLOBAL")" "$GH_CONFIG_DIR"

git config --global user.name "alucard-bot"
git config --global user.email "alucard-bot@users.noreply.github.com"
git config --global --add safe.directory /work
git config --global --add safe.directory '*'

# gh uses GITHUB_TOKEN from env automatically; wire the same token into git
# so `git push` works without a separate credential store.
git config --global credential.helper "store --file /tmp/git-credentials"
printf 'https://x-access-token:%s@github.com\n' "$GITHUB_TOKEN" > /tmp/git-credentials

exec "$@"
