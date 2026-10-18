#!/bin/bash
set -euo pipefail

SCRIPT_DIR="$(cd -P "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/lib.sh"
INSTALL="$SCRIPT_DIR/../install.sh"

TEST_TMPDIR=$(mktemp -d)
trap 'rm -rf "$TEST_TMPDIR"' EXIT

# A source clone whose default branch moves past its latest tag.
SRC="$TEST_TMPDIR/src"
git clone -q "$SCRIPT_DIR/.." "$SRC"
git -C "$SRC" config user.email test@example.com
git -C "$SRC" config user.name test
git -C "$SRC" tag -l | xargs -r git -C "$SRC" tag -d >/dev/null

RELEASE="$TEST_TMPDIR/release"
# Tags SRC's HEAD as $1 and publishes a stub binary that reports it.
release() {
  git -C "$SRC" tag "$1"
  mkdir -p "$RELEASE/$1"
  for target in x86_64-unknown-linux-musl aarch64-unknown-linux-musl; do
    printf '#!/bin/sh\necho "alucard %s"\n' "$1" > "$RELEASE/$1/alucard-$target"
  done
}
release v0.1.0
git -C "$SRC" commit -q --allow-empty -m "second release"
release v0.2.0
git -C "$SRC" commit -q --allow-empty -m "unreleased"

export ALUCARD_HOME="$TEST_TMPDIR/home"
export ALUCARD_BIN_DIR="$TEST_TMPDIR/bin"
export ALUCARD_REPO_URL="file://$SRC"
export ALUCARD_RELEASE_URL="file://$RELEASE"

run_install() { bash "$INSTALL" </dev/null 2>&1; }

run_install >/dev/null
assert_eq "first install links the latest binary" "alucard v0.2.0" "$("$ALUCARD_BIN_DIR/alucard" version)"
assert_eq "first install checks out the latest tag, not the branch head" \
  "$(git -C "$SRC" rev-parse v0.2.0)" "$(git -C "$ALUCARD_HOME" rev-parse HEAD)"

ALUCARD_VERSION=v0.1.0 run_install >/dev/null
assert_eq "ALUCARD_VERSION selects the binary" "alucard v0.1.0" "$("$ALUCARD_BIN_DIR/alucard" version)"
assert_eq "ALUCARD_VERSION selects the checkout" \
  "$(git -C "$SRC" rev-parse v0.1.0)" "$(git -C "$ALUCARD_HOME" rev-parse HEAD)"

head_before="$(git -C "$ALUCARD_HOME" rev-parse HEAD)"
sum_before="$(sha256sum < "$ALUCARD_HOME/bin/alucard")"

# Leaves the previous install untouched after a failed re-install.
assert_install_unchanged() {
  local label="$1" message="$2" out="$3" status="$4"
  assert_eq "$label exits non-zero" 1 "$status"
  assert_contains "$label explains why" "$message" "$out"
  assert_eq "$label keeps the clone" "$head_before" "$(git -C "$ALUCARD_HOME" rev-parse HEAD)"
  assert_eq "$label keeps the binary" "$sum_before" "$(sha256sum < "$ALUCARD_HOME/bin/alucard")"
  assert_eq "$label keeps alucard working" "alucard v0.1.0" "$("$ALUCARD_BIN_DIR/alucard" version)"
}

# The new tag has no published binary yet.
git -C "$SRC" tag v0.3.0
status=0
out="$(run_install)" || status=$?
assert_install_unchanged "failed download" "failed to download" "$out" "$status"

# A published binary whose tag cannot be fetched.
mkdir -p "$RELEASE/v9.9.9"
cp "$RELEASE/v0.2.0/"* "$RELEASE/v9.9.9/"
status=0
out="$(ALUCARD_VERSION=v9.9.9 run_install)" || status=$?
assert_install_unchanged "failed tag fetch" "failed to fetch v9.9.9" "$out" "$status"

STUB_BIN="$TEST_TMPDIR/stub-bin"
mkdir -p "$STUB_BIN"
cat > "$STUB_BIN/uname" <<EOF
#!/bin/sh
[ "\$1" = -m ] && { echo riscv64; exit; }
exec $(command -v uname) "\$@"
EOF
chmod +x "$STUB_BIN/uname"
status=0
out="$(PATH="$STUB_BIN:$PATH" run_install)" || status=$?
assert_install_unchanged "unsupported platform" "unsupported platform: Linux riscv64" "$out" "$status"

finish
