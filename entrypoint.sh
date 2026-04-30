#!/bin/bash
set -euo pipefail

export GIT_CONFIG_GLOBAL="${GIT_CONFIG_GLOBAL:-/tmp/alucard-gitconfig}"
export GH_CONFIG_DIR="${GH_CONFIG_DIR:-/tmp/gh}"

mkdir -p "$(dirname "$GIT_CONFIG_GLOBAL")" "$GH_CONFIG_DIR"

git config --global user.name "alucard-bot"
git config --global user.email "alucard-bot@users.noreply.github.com"
git config --global --add safe.directory /work
git config --global --add safe.directory '*'

printf '%s\n' "$GITHUB_TOKEN" | gh auth login --with-token
gh auth setup-git

exec "$@"
