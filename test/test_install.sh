#!/bin/bash
set -euo pipefail

SCRIPT_DIR="$(cd -P "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/lib.sh"
INSTALL="$SCRIPT_DIR/../install.sh"

TEST_TMPDIR=$(mktemp -d)
trap 'rm -rf "$TEST_TMPDIR"' EXIT

# A source clone that gains a commit after the first install, so a failed
# re-install that still pulled would move the installed HEAD.
SRC="$TEST_TMPDIR/src"
git clone -q "$SCRIPT_DIR/.." "$SRC"
git -C "$SRC" config user.email test@example.com
git -C "$SRC" config user.name test

RELEASE="$TEST_TMPDIR/release"
mkdir -p "$RELEASE"
for target in x86_64-unknown-linux-musl aarch64-unknown-linux-musl; do
  printf '#!/bin/sh\necho "alucard stub"\n' > "$RELEASE/alucard-$target"
done

export ALUCARD_HOME="$TEST_TMPDIR/home"
export ALUCARD_BIN_DIR="$TEST_TMPDIR/bin"
export ALUCARD_REPO_URL="file://$SRC"

run_install() { bash "$INSTALL" </dev/null 2>&1; }

ALUCARD_RELEASE_URL="file://$RELEASE" run_install >/dev/null
assert_eq "first install links the binary" "alucard stub" "$("$ALUCARD_BIN_DIR/alucard" version)"

git -C "$SRC" commit -q --allow-empty -m "newer upstream"
head_before="$(git -C "$ALUCARD_HOME" rev-parse HEAD)"
sum_before="$(sha256sum < "$ALUCARD_HOME/bin/alucard")"

# Leaves the previous install untouched after a failed re-install.
assert_install_unchanged() {
  local label="$1" message="$2" out="$3" status="$4"
  assert_eq "$label exits non-zero" 1 "$status"
  assert_contains "$label explains why" "$message" "$out"
  assert_eq "$label keeps the clone" "$head_before" "$(git -C "$ALUCARD_HOME" rev-parse HEAD)"
  assert_eq "$label keeps the binary" "$sum_before" "$(sha256sum < "$ALUCARD_HOME/bin/alucard")"
  assert_eq "$label keeps alucard working" "alucard stub" "$("$ALUCARD_BIN_DIR/alucard" version)"
}

status=0
out="$(ALUCARD_RELEASE_URL="file://$TEST_TMPDIR/missing" run_install)" || status=$?
assert_install_unchanged "failed download" "failed to download" "$out" "$status"

STUB_BIN="$TEST_TMPDIR/stub-bin"
mkdir -p "$STUB_BIN"
cat > "$STUB_BIN/uname" <<EOF
#!/bin/sh
[ "\$1" = -m ] && { echo riscv64; exit; }
exec $(command -v uname) "\$@"
EOF
chmod +x "$STUB_BIN/uname"
status=0
out="$(PATH="$STUB_BIN:$PATH" ALUCARD_RELEASE_URL="file://$RELEASE" run_install)" || status=$?
assert_install_unchanged "unsupported platform" "unsupported platform: Linux riscv64" "$out" "$status"

finish
