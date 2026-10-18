#!/usr/bin/env bash
# Alucard installer
# Usage: curl -fsSL https://raw.githubusercontent.com/aldovc/alucard/main/install.sh | sh
set -euo pipefail

REPO_URL="${ALUCARD_REPO_URL:-https://github.com/aldovc/alucard.git}"
RELEASE_URL="${ALUCARD_RELEASE_URL:-https://github.com/aldovc/alucard/releases/latest/download}"
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
command -v curl >/dev/null 2>&1 || die "curl is required to install alucard"

# Fetch and check the binary before touching an existing install
os="$(uname -s)"
arch="$(uname -m)"
case "$os/$arch" in
  Linux/x86_64|Linux/amd64)  target="x86_64-unknown-linux-musl" ;;
  Linux/aarch64|Linux/arm64) target="aarch64-unknown-linux-musl" ;;
  *) die "unsupported platform: $os $arch (alucard ships Linux x86_64 and aarch64 binaries)";;
esac
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
binary_url="$RELEASE_URL/alucard-$target"
info "Downloading $binary_url"
curl -fsSL -o "$tmp/alucard" "$binary_url" || die "failed to download $binary_url; existing install left unchanged"
chmod +x "$tmp/alucard"
"$tmp/alucard" version >/dev/null 2>&1 || die "downloaded binary does not run on this host; existing install left unchanged"

# Clone or update
if [ -d "$ALUCARD_HOME/.git" ]; then
  info "Updating existing install at $ALUCARD_HOME"
  git -C "$ALUCARD_HOME" pull --ff-only
else
  info "Installing alucard into $ALUCARD_HOME"
  git clone --depth 1 "$REPO_URL" "$ALUCARD_HOME"
fi

chmod +x "$ALUCARD_HOME/alucard" "$ALUCARD_HOME/entrypoint.sh"

# The binary finds its tool files through the Cargo.toml in ALUCARD_HOME
mkdir -p "$ALUCARD_HOME/bin"
cp "$tmp/alucard" "$ALUCARD_HOME/bin/.alucard.new"
mv -f "$ALUCARD_HOME/bin/.alucard.new" "$ALUCARD_HOME/bin/alucard"

# Symlink CLIs into BIN_DIR
mkdir -p "$BIN_DIR"
ln -sf "$ALUCARD_HOME/bin/alucard" "$BIN_DIR/alucard"
ln -sf "$ALUCARD_HOME/alucard" "$BIN_DIR/alucard-bash"
ok "alucard linked at $BIN_DIR/alucard, Bash CLI at $BIN_DIR/alucard-bash"

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
  2. Pull the pre-built image:     docker pull ghcr.io/aldovc/alucard:latest
     (or build locally:            alucard build)
  3. Run against a repo:           alucard run /path/to/repo --iterations 1

Full docs: https://github.com/aldovc/alucard#readme
EOF
