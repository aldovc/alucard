#!/usr/bin/env bash
# Alucard installer
# Usage: curl -fsSL https://raw.githubusercontent.com/aldovc/alucard/main/install.sh | sh
set -euo pipefail

REPO_URL="${ALUCARD_REPO_URL:-https://github.com/aldovc/alucard.git}"
RELEASE_URL="${ALUCARD_RELEASE_URL:-https://github.com/aldovc/alucard/releases/download}"
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
# Binary and tool files come from the same release tag
version="${ALUCARD_VERSION:-}"
if [ -z "$version" ]; then
  version="$(git ls-remote --tags --refs "$REPO_URL" 'v*' | sed 's#.*refs/tags/##' \
    | grep -E '^v[0-9]+\.[0-9]+\.[0-9]+$' | sort -V | tail -n 1)" || true
  [ -n "$version" ] || die "could not resolve the latest release tag from $REPO_URL; existing install left unchanged"
fi
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
binary_url="$RELEASE_URL/$version/alucard-$target"
info "Downloading $binary_url"
curl -fsSL -o "$tmp/alucard" "$binary_url" || die "failed to download $binary_url; existing install left unchanged"
chmod +x "$tmp/alucard"
"$tmp/alucard" version >/dev/null 2>&1 || die "downloaded binary does not run on this host; existing install left unchanged"

# Clone or update
if [ -d "$ALUCARD_HOME/.git" ]; then
  info "Updating existing install at $ALUCARD_HOME to $version"
  git -C "$ALUCARD_HOME" fetch -q --depth 1 origin tag "$version" \
    || die "failed to fetch $version; existing install left unchanged"
  git -C "$ALUCARD_HOME" -c advice.detachedHead=false checkout -q "$version"
else
  info "Installing alucard $version into $ALUCARD_HOME"
  git -c advice.detachedHead=false clone -q --depth 1 --branch "$version" "$REPO_URL" "$ALUCARD_HOME"
fi

chmod +x "$ALUCARD_HOME/entrypoint.sh"

# The binary finds its tool files through the Cargo.toml in ALUCARD_HOME
mkdir -p "$ALUCARD_HOME/bin"
cp "$tmp/alucard" "$ALUCARD_HOME/bin/.alucard.new"
mv -f "$ALUCARD_HOME/bin/.alucard.new" "$ALUCARD_HOME/bin/alucard"

# Symlink the CLI into BIN_DIR
mkdir -p "$BIN_DIR"
ln -sf "$ALUCARD_HOME/bin/alucard" "$BIN_DIR/alucard"
# Releases up to v0.7.1 also linked the Bash CLI, which no longer exists
if [ "$(readlink "$BIN_DIR/alucard-bash" 2>/dev/null)" = "$ALUCARD_HOME/alucard" ]; then
  rm -f "$BIN_DIR/alucard-bash"
fi
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
  2. Pull the pre-built image:     docker pull ghcr.io/aldovc/alucard:latest
     (or build locally:            alucard build)
  3. Run against a repo:           alucard run /path/to/repo --iterations 1

Full docs: https://github.com/aldovc/alucard#readme
EOF
