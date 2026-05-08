#!/usr/bin/env bash
# Alucard installer
# Usage: curl -fsSL https://raw.githubusercontent.com/aldovc/alucard/main/install.sh | sh
set -euo pipefail

REPO_URL="https://github.com/aldovc/alucard.git"
ALUCARD_HOME="${ALUCARD_HOME:-$HOME/.local/share/alucard}"
BIN_DIR="${ALUCARD_BIN_DIR:-$HOME/.local/bin}"

info()  { printf '\033[1;34m==> \033[0m%s\n' "$*"; }
ok()    { printf '\033[1;32m ok \033[0m%s\n' "$*"; }
warn()  { printf '\033[1;33mwarn\033[0m %s\n' "$*" >&2; }
die()   { printf '\033[1;31merr \033[0m%s\n' "$*" >&2; exit 1; }

# Prerequisites
for cmd in git docker gh jq; do
  command -v "$cmd" >/dev/null 2>&1 || warn "missing: $cmd (required before running alucard)"
done
command -v git >/dev/null 2>&1 || die "git is required to install alucard"

# Clone or update
if [ -d "$ALUCARD_HOME/.git" ]; then
  info "Updating existing install at $ALUCARD_HOME"
  git -C "$ALUCARD_HOME" pull --ff-only
else
  info "Installing alucard into $ALUCARD_HOME"
  git clone --depth 1 "$REPO_URL" "$ALUCARD_HOME"
fi

chmod +x "$ALUCARD_HOME/alucard" "$ALUCARD_HOME/entrypoint.sh"

# Symlink CLI into BIN_DIR
mkdir -p "$BIN_DIR"
ln -sf "$ALUCARD_HOME/alucard" "$BIN_DIR/alucard"
ok "alucard linked at $BIN_DIR/alucard"

# Create env file from example if not already present
if [ ! -f "$ALUCARD_HOME/alucard.env" ]; then
  cp "$ALUCARD_HOME/alucard.env.example" "$ALUCARD_HOME/alucard.env"
  ok "credential template written to $ALUCARD_HOME/alucard.env"
fi

# PATH hint
case ":$PATH:" in
  *":$BIN_DIR:"*) ;;
  *) warn "$BIN_DIR is not on your PATH — add it to your shell rc:"
     warn "  export PATH=\"$BIN_DIR:\$PATH\"";;
esac

cat <<EOF

Done. Next steps:
  1. Edit $ALUCARD_HOME/alucard.env — add GITHUB_TOKEN and ANTHROPIC_API_KEY
  2. Build the Docker image:       alucard build
  3. Run against a repo:           alucard run /path/to/repo --iterations 1

Full docs: https://github.com/aldovc/alucard#readme
EOF
