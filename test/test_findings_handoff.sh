#!/bin/bash
# The reviewer writes sections meant for a human — out-of-scope follow-ups, and
# fixes too large for one pass. The feedback agent is handed the review body
# verbatim, so those sections must be stripped before they reach it or it will
# act on them, which is the damage they exist to prevent.
set -euo pipefail

SCRIPT_DIR="$(cd -P "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ALUCARD="$SCRIPT_DIR/../alucard"

PASS=0
FAIL=0

pass() { echo "PASS: $1"; PASS=$((PASS + 1)); }
fail() { echo "FAIL: $1" >&2; FAIL=$((FAIL + 1)); }

assert_eq() {
  local label="$1" expected="$2" actual="$3"
  if [ "$expected" = "$actual" ]; then
    pass "$label"
  else
    fail "$label (expected '$expected', got '$actual')"
  fi
}

assert_contains() {
  local label="$1" needle="$2" haystack="$3"
  if grep -qF -- "$needle" <<<"$haystack"; then
    pass "$label"
  else
    fail "$label (missing '$needle')"
  fi
}

assert_not_contains() {
  local label="$1" needle="$2" haystack="$3"
  if grep -qF -- "$needle" <<<"$haystack"; then
    fail "$label (unexpectedly contains '$needle')"
  else
    pass "$label"
  fi
}

# shellcheck disable=SC1090
source "$ALUCARD"

TMP_ROOT=$(mktemp -d /tmp/alucard_test_handoff.XXXXXX)
trap 'rm -rf "$TMP_ROOT"' EXIT

# ── strip_nonactionable_sections ─────────────────────────────────────────────
echo "── strip_nonactionable_sections ──"

BODY='## Findings

- **Severity**: High
- **Problem**: KEEP-FINDING-ONE

## Too large for this loop

DROP-TOO-LARGE — a cross-cutting refactor.

## Out of scope (follow-up)

DROP-OUT-OF-SCOPE — predates the branch.'

out=$(printf '%s' "$BODY" | strip_nonactionable_sections)

assert_contains "the findings survive" "KEEP-FINDING-ONE" "$out"
assert_not_contains "the too-large section is dropped" "DROP-TOO-LARGE" "$out"
assert_not_contains "its heading goes too" "Too large for this loop" "$out"
assert_not_contains "the out-of-scope section is dropped" "DROP-OUT-OF-SCOPE" "$out"
assert_not_contains "its heading goes too" "Out of scope (follow-up)" "$out"

# A section that FOLLOWS a stripped one must come back — skipping must end at
# the next heading, not run to the end of the body.
SANDWICH='## Findings

KEEP-BEFORE

## Too large for this loop

DROP-MIDDLE

## Expected fix

KEEP-AFTER'
out=$(printf '%s' "$SANDWICH" | strip_nonactionable_sections)
assert_contains "content before a stripped section survives" "KEEP-BEFORE" "$out"
assert_not_contains "the stripped section is gone" "DROP-MIDDLE" "$out"
assert_contains "content after a stripped section survives" "KEEP-AFTER" "$out"

# A body with neither section must pass through untouched.
PLAIN='## Findings

- **Severity**: Medium
- **Problem**: only actionable things here'
assert_eq "a body with neither section is unchanged" \
  "$PLAIN" "$(printf '%s' "$PLAIN" | strip_nonactionable_sections)"

# The heading is matched as a heading. A finding that merely mentions the
# phrase in prose must not silently delete the rest of the review.
INLINE='## Findings

- **Problem**: this would be Too large for this loop if taken literally
- **Expected fix**: KEEP-INLINE'
out=$(printf '%s' "$INLINE" | strip_nonactionable_sections)
assert_contains "a prose mention does not trigger stripping" "KEEP-INLINE" "$out"

# Review bodies quote code, and a `##` line inside a fence is not a heading.
# Both directions were reproduced before this was handled: a fenced example of
# the heading swallowed every finding after it, and a `##` line inside a
# deferred section's example resumed output and leaked the rest of it.
FENCED_HEADING='## Findings

- **Expected fix**: use this shape:

```markdown
## Too large for this loop
```

- **Problem**: KEEP-AFTER-FENCE'
out=$(printf '%s' "$FENCED_HEADING" | strip_nonactionable_sections)
assert_contains "a fenced heading does not start stripping" "KEEP-AFTER-FENCE" "$out"
assert_contains 'the fenced example itself survives' '```markdown' "$out"

FENCED_INSIDE='## Findings

KEEP-BEFORE-FENCE

## Too large for this loop

```
## not a heading
```

DROP-AFTER-INNER-FENCE'
out=$(printf '%s' "$FENCED_INSIDE" | strip_nonactionable_sections)
assert_contains "content before the deferred section survives" "KEEP-BEFORE-FENCE" "$out"
assert_not_contains "a fenced ## inside a deferred section does not resume it" \
  "DROP-AFTER-INNER-FENCE" "$out"
assert_not_contains "and its fenced example stays out too" "not a heading" "$out"

# Tilde fences are markdown too.
TILDE='## Findings

~~~
## Too large for this loop
~~~

KEEP-AFTER-TILDE'
assert_contains "tilde fences are honoured as well" \
  "KEEP-AFTER-TILDE" "$(printf '%s' "$TILDE" | strip_nonactionable_sections)"

# Quoting a fence means wrapping it in a longer one, or a different character.
# A naive toggle closes a four-backtick fence on three, and a tilde fence on
# backticks — both reproduced, and both deleted the finding that followed.
LONG_FENCE='## Findings

````markdown
```
## Too large for this loop
````

- **Problem**: KEEP-PAST-LONG-FENCE'
assert_contains "three backticks do not close a four-backtick fence" \
  "KEEP-PAST-LONG-FENCE" "$(printf '%s' "$LONG_FENCE" | strip_nonactionable_sections)"

MIXED_FENCE='## Findings

~~~
```
## Too large for this loop
~~~

- **Problem**: KEEP-PAST-TILDE-FENCE'
assert_contains "backticks do not close a tilde fence" \
  "KEEP-PAST-TILDE-FENCE" "$(printf '%s' "$MIXED_FENCE" | strip_nonactionable_sections)"

# A closer carries nothing but whitespace, so an info string opens a fence and
# never closes one.
INFO_STRING='## Findings

```
```python
## Too large for this loop
```

- **Problem**: KEEP-PAST-INFO-STRING'
assert_contains "an info string does not close a fence" \
  "KEEP-PAST-INFO-STRING" "$(printf '%s' "$INFO_STRING" | strip_nonactionable_sections)"

# An opener with no closer is text, not a fence to end of input. Inside a
# deferred section the alternative swallows every heading after it.
UNTERMINATED='## Findings

KEEP-BEFORE-UNTERMINATED

## Too large for this loop

```
never closed

## Expected fix

KEEP-AFTER-UNTERMINATED'
out=$(printf '%s' "$UNTERMINATED" | strip_nonactionable_sections)
assert_contains "an unterminated fence does not swallow the rest of the body" \
  "KEEP-AFTER-UNTERMINATED" "$out"
assert_not_contains "the deferred section is still stripped" "never closed" "$out"

# ── End to end: the section never reaches the feedback agent ─────────────────
# The helper being right is not the same as it being wired in. Drive `alucard
# continue` with a reviewer body that carries both sections and read the
# feedback prompt the harness writes to disk.
echo "── assembled feedback prompt ──"

MOCK_BIN="$TMP_ROOT/bin"; mkdir -p "$MOCK_BIN"
TARGET="$TMP_ROOT/target"
REMOTE="$TMP_ROOT/remote.git"
BRANCH="feat/under-review"

git init -q --bare "$REMOTE"
mkdir -p "$TARGET"
git -C "$TARGET" init -q -b main
git -C "$TARGET" config user.email test@example.invalid
git -C "$TARGET" config user.name test
printf '# Target\n' > "$TARGET/README.md"
git -C "$TARGET" add .
git -C "$TARGET" commit -qm initial
git -C "$TARGET" remote add origin "$REMOTE"
git -C "$TARGET" push -qu origin main
git -C "$TARGET" checkout -q -b "$BRANCH"
printf 'change\n' >> "$TARGET/README.md"
git -C "$TARGET" commit -qam change
git -C "$TARGET" push -q origin "$BRANCH"
git -C "$TARGET" checkout -q main

cp "$SCRIPT_DIR/fixtures/credentials.env" "$TMP_ROOT/alucard.env"

cat > "$MOCK_BIN/docker" <<'MOCK'
#!/bin/bash
set -euo pipefail
case "${1:-}" in
  run) ;;
  *) exit 0 ;;
esac
for _a in "$@"; do
  case "$_a" in
    *:/work-output:rw)
      printf 'CHANGES_REQUESTED\n' > "${_a%%:*}/.alucard-review"
      {
        printf '## Findings\n\n'
        printf -- '- **Problem**: KEEP-FINDING-ONE\n\n'
        printf '## Too large for this loop\n\n'
        printf 'DROP-TOO-LARGE\n\n'
        printf '## Out of scope (follow-up)\n\n'
        printf 'DROP-OUT-OF-SCOPE\n'
      } > "${_a%%:*}/.alucard-review-body" ;;
  esac
done
printf '%s\n' '{"type":"result","subtype":"success","is_error":false,"result":"done"}'
MOCK

cat > "$MOCK_BIN/timeout" <<'MOCK'
#!/bin/bash
set -euo pipefail
if [ "$1" = "--kill-after" ]; then shift 2; fi
shift
exec "$@"
MOCK

cat > "$MOCK_BIN/gh" <<'MOCK'
#!/bin/bash
set -euo pipefail
case "$1 $2" in
  "pr checks") echo "no checks reported" >&2; exit 1 ;;
  "pr list")   printf '77\n' ;;
  "pr view")
    case "$*" in
      *state,headRefName*) printf '{"state":"OPEN","headRefName":"%s"}\n' "$ALUCARD_TEST_BRANCH" ;;
      *comments*)          printf '{"comments":[]}\n' ;;
      *reviews*)           printf '\n' ;;
      *headRefOid*)        printf 'deadbeef\n' ;;
      *)                   printf '{}\n' ;;
    esac ;;
  *) : ;;
esac
MOCK
chmod +x "$MOCK_BIN/docker" "$MOCK_BIN/timeout" "$MOCK_BIN/gh"

rm -rf "$TMP_ROOT/logs"
set +e
PATH="$MOCK_BIN:$PATH" ALUCARD_TEST_BRANCH="$BRANCH" \
  "$ALUCARD" continue 77 "$TARGET" --no-build --max-review-cycles 2 \
    --env-file "$TMP_ROOT/alucard.env" --logs-root "$TMP_ROOT/logs" \
    > "$TMP_ROOT/out" 2>&1
set -e

fb=$(cat "$TMP_ROOT"/logs/alucard-*/prompts/*feedback*.txt 2>/dev/null || true)
if [ -z "$fb" ]; then
  fail "harness captured a feedback prompt (see $TMP_ROOT/out)"
else
  pass "harness captured a feedback prompt"
  assert_contains "the actionable finding reaches feedback" "KEEP-FINDING-ONE" "$fb"
  assert_not_contains "the too-large section does not" "DROP-TOO-LARGE" "$fb"
  assert_not_contains "the out-of-scope section does not" "DROP-OUT-OF-SCOPE" "$fb"
fi

echo
echo "── $PASS passed, $FAIL failed ──"
[ "$FAIL" -eq 0 ]
