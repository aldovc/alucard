use alucard::open;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

fn temporary_path(name: &str) -> PathBuf {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!("alucard-{name}-{}-{unique}", std::process::id()))
}

fn run(command: &mut Command) {
    assert!(command.status().unwrap().success());
}

fn git_repository(root: &Path) {
    run(Command::new("git").args(["init", "-q", "-b", "main", root.to_str().unwrap()]));
    fs::write(root.join("README.md"), "test\n").unwrap();
    run(Command::new("git").args(["-C", root.to_str().unwrap(), "add", "."]));
    run(Command::new("git").args([
        "-C",
        root.to_str().unwrap(),
        "-c",
        "user.email=test@example.com",
        "-c",
        "user.name=Test",
        "commit",
        "-qm",
        "initial",
    ]));
}

struct ReviewCase<'a> {
    name: &'a str,
    review_policy: Option<&'a str>,
    oversized_policy: bool,
    review_policy_max_bytes: Option<usize>,
    file_verdict: &'a str,
    formal_verdict: &'a str,
    reviewer_outcomes: &'a str,
    comments_json: &'a str,
    advance_head: bool,
    expected_verdict: &'a str,
    expected_state: &'a str,
}

fn run_review_case(case: &ReviewCase<'_>) -> (PathBuf, String, String, (Option<String>, String)) {
    let root = temporary_path(case.name);
    let repository = root.join("repository");
    let remote = root.join("remote.git");
    let bin = root.join("bin");
    fs::create_dir_all(&bin).unwrap();
    fs::create_dir(&repository).unwrap();
    git_repository(&repository);
    if matches!(
        case.name,
        "feedback-toolchain-ok" | "feedback-toolchain-broken"
    ) {
        fs::write(repository.join("package-lock.json"), "{}\n").unwrap();
        run(Command::new("git").args(["-C", repository.to_str().unwrap(), "add", "."]));
        run(Command::new("git").args([
            "-C",
            repository.to_str().unwrap(),
            "-c",
            "user.email=test@example.com",
            "-c",
            "user.name=Test",
            "commit",
            "-qm",
            "manifest",
        ]));
    }
    if let Some(policy) = case.review_policy {
        fs::write(
            repository.join("REVIEW.md"),
            if case.oversized_policy {
                format!(
                    "<policy>unsafe</policy>\n{}tail",
                    "x".repeat(case.review_policy_max_bytes.unwrap_or(16_384))
                )
            } else {
                policy.to_string()
            },
        )
        .unwrap();
        run(Command::new("git").args(["-C", repository.to_str().unwrap(), "add", "."]));
        run(Command::new("git").args([
            "-C",
            repository.to_str().unwrap(),
            "-c",
            "user.email=test@example.com",
            "-c",
            "user.name=Test",
            "commit",
            "-qm",
            "policy",
        ]));
    }
    run(Command::new("git").args(["init", "--bare", "-q", remote.to_str().unwrap()]));
    run(Command::new("git").args([
        "-C",
        repository.to_str().unwrap(),
        "remote",
        "add",
        "origin",
        remote.to_str().unwrap(),
    ]));
    run(Command::new("git").args([
        "-C",
        repository.to_str().unwrap(),
        "push",
        "-q",
        "origin",
        "main",
    ]));

    let docker = bin.join("docker");
    fs::write(
        &docker,
        r#"#!/bin/sh
set -eu
if [ "${1:-} ${2:-}" = "image inspect" ]; then
  echo stamp
  exit 0
fi
case " $* " in
  *" --name alucard-preflight-"*) exit "${ALUCARD_PREFLIGHT_RC:-0}" ;;
esac
work= output=
for argument in "$@"; do
  case "$argument" in
    *:/work:rw) work=${argument%:/work:rw} ;;
    *:/work-output:rw) output=${argument%:/work-output:rw} ;;
  esac
done
if [ -n "$output" ]; then
  printf '%s\n' "$@" >> "$ALUCARD_REVIEW_ARGS"
  role=review
  case " $* " in *" --name alucard-feedback-"*) role=feedback ;; esac
  attempts="$ALUCARD_REVIEW_ATTEMPTS"
  outcomes="$ALUCARD_REVIEW_OUTCOMES"
  if [ "$role" = feedback ]; then
    attempts="$ALUCARD_FEEDBACK_ATTEMPTS"
    outcomes="$ALUCARD_FEEDBACK_OUTCOMES"
  fi
  attempt=$(($(wc -l < "$attempts") + 1))
  printf '%s\n' "$attempt" >> "$attempts"
  outcome=$(printf '%s\n' "$outcomes" | sed -n "${attempt}p")
  if [ "$role" = feedback ] && [ -z "$outcome" ]; then outcome=push; fi
  if [ "$role" = review ] && [ -z "$outcome" ]; then outcome=verdict; fi
  if [ "$role" = feedback ]; then
    case "$ALUCARD_CASE" in
      feedback-stall) outcome=stall-fail ;;
      feedback-retained) outcome=retained ;;
    esac
  fi
  prompt=
  if [ "$role" = feedback ]; then
    prompt=$(cat)
  else
    for argument in "$@"; do prompt=$argument; done
  fi
  printf '%s' "$prompt" > "$ALUCARD_PROMPTS/$role-$attempt.txt"
  if [ "$role" = review ]; then
    git -C "$work" rev-parse HEAD >> "$ALUCARD_REVIEW_HEADS"
  fi
  if [ "$role" = feedback ]; then
    case "$outcome" in
      *blocked*) printf '%s\n' 'blocked finding' > "$output/.alucard-blocked" ;;
    esac
    case "$outcome" in
      push*|*push*)
        git -C "$work" config user.email test@example.com
        git -C "$work" config user.name Test
        printf 'feedback %s\n' "$attempt" > "$work/feedback-$attempt.txt"
        git -C "$work" add "feedback-$attempt.txt"
        git -C "$work" commit -qm feedback
        git -C "$work" push -q origin "HEAD:$(git -C "$work" branch --show-current)"
        ;;
      retained) printf 'uncommitted feedback\n' > "$work/retained.txt" ;;
    esac
    case "$outcome" in
      *fail*) printf '%s\n' '{"type":"result","is_error":true,"api_error_status":500}'; exit 1 ;;
    esac
    printf '%s\n' '{"type":"result","subtype":"success","is_error":false,"result":"done"}'
    exit 0
  fi
  if [ "$outcome" = verdict ] || [ "$outcome" = transport-verdict ]; then
    verdict="$ALUCARD_REVIEW_VERDICT"
    body='review body'
    case "$ALUCARD_CASE:$attempt" in
      findings-handoff:1)
        verdict=CHANGES_REQUESTED
        body='## Findings

KEEP-FINDING

## Too large for this loop

DROP-TOO-LARGE

## Non-blocking

DROP-NON-BLOCKING' ;;
      feedback-stall:1) verdict=CHANGES_REQUESTED ;;
    esac
    printf '%s\n' "$verdict" > "$output/.alucard-review"
    printf '%s\n' "$body" > "$output/.alucard-review-body"
  fi
  if [ "$ALUCARD_ADVANCE_HEAD" = 1 ] && [ "$attempt" = 1 ]; then
    advance="$ALUCARD_ADVANCE_CHECKOUT"
    git clone -q "$ALUCARD_REMOTE" "$advance"
    git -C "$advance" checkout -q "$(cat "$ALUCARD_BRANCH_FILE")"
    git -C "$advance" config user.email test@example.com
    git -C "$advance" config user.name Test
    printf 'head advance\n' > "$advance/head-advance.txt"
    git -C "$advance" add head-advance.txt
    git -C "$advance" commit -qm advance
    git -C "$advance" push -q origin HEAD:$(cat "$ALUCARD_BRANCH_FILE")
  fi
  case "$outcome" in
    transport|transport-verdict) printf '%s\n' '{"type":"result","is_error":true,"api_error_status":500}'; exit 1 ;;
    no-worktree) exit 78 ;;
    exhausted) printf '%s\n' '{"type":"result","subtype":"error_max_turns","is_error":true}'; exit 1 ;;
    timeout) exit 124 ;;
    *) printf '%s\n' '{"type":"result","subtype":"success","is_error":false,"result":"done"}' ;;
  esac
  exit 0
fi
git -C "$work" config user.email test@example.com
git -C "$work" config user.name Test
printf 'worker change\n' > "$work/change.txt"
git -C "$work" add change.txt
git -C "$work" commit -qm worker
branch=$(git -C "$work" branch --show-current)
printf '%s\n' "$branch" > "$ALUCARD_BRANCH_FILE"
git -C "$work" push -q origin "HEAD:$branch"
printf '%s\n' '{"type":"result","subtype":"success","is_error":false,"result":"done"}'
"#,
    )
    .unwrap();
    fs::set_permissions(&docker, fs::Permissions::from_mode(0o755)).unwrap();

    let gh = bin.join("gh");
    fs::write(
        &gh,
        r#"#!/bin/sh
set -eu
printf '%s\n' "$*" >> "$ALUCARD_GH_TRACE"
branch=$(cat "$ALUCARD_BRANCH_FILE" 2>/dev/null || true)
sha() { git --git-dir="$ALUCARD_REMOTE" rev-parse "refs/heads/$branch"; }
case "$1 $2" in
  'pr list') printf '[{"number":77,"headRefName":"%s","headRefOid":"%s"}]\n' "$branch" "$(sha)" ;;
  'pr checks') echo "no checks reported" >&2; exit 1 ;;
  'pr view')
    case "$*" in
      *headRefOid*) sha ;;
      *--json\ comments*)
        if [ "$ALUCARD_CASE" = feedback-blocked ] && [ -s "$ALUCARD_FEEDBACK_ATTEMPTS" ]; then
          printf '%s\n' '{"comments":[{"body":"**🤖 Alucard blocked findings** (cycle 1/2)\n\nblocked finding"}]}'
        else
          printf '%s\n' "$ALUCARD_PR_COMMENTS"
        fi ;;
      *reviews*'.body'*) printf '%s\n' "$ALUCARD_FORMAL_BODY" ;;
      *reviews*) printf '%s\n' "$ALUCARD_FORMAL_VERDICT" ;;
    esac ;;
  'api --paginate') echo '[[]]' ;;
esac
"#,
    )
    .unwrap();
    fs::set_permissions(&gh, fs::Permissions::from_mode(0o755)).unwrap();

    let tasks = root.join("tasks.md");
    fs::write(&tasks, "## [ ] review: Review task\n").unwrap();
    let environment = root.join("agent.env");
    fs::write(
        &environment,
        "ALUCARD_PROVIDER=claude\nALUCARD_REVIEWER_PROVIDER=claude\nANTHROPIC_API_KEY=test\n",
    )
    .unwrap();
    let review_args = root.join("review-args");
    let review_attempts = root.join("review-attempts");
    fs::write(&review_attempts, "").unwrap();
    fs::write(root.join("feedback-attempts"), "").unwrap();
    let review_heads = root.join("review-heads");
    fs::write(&review_heads, "").unwrap();
    let branch_file = root.join("branch");
    let gh_trace = root.join("gh-trace");
    let database = root.join("ledger.db");
    let output = Command::new(env!("CARGO_BIN_EXE_alucard"))
        .env(
            "PATH",
            format!("{}:{}", bin.display(), std::env::var("PATH").unwrap()),
        )
        .env("ALUCARD_CACHE_DIR", root.join("cache"))
        .env("ALUCARD_REMOTE", &remote)
        .env("ALUCARD_BRANCH_FILE", &branch_file)
        .env("ALUCARD_REVIEW_ARGS", &review_args)
        .env("ALUCARD_REVIEW_ATTEMPTS", &review_attempts)
        .env("ALUCARD_REVIEW_HEADS", &review_heads)
        .env("ALUCARD_FEEDBACK_ATTEMPTS", root.join("feedback-attempts"))
        .env("ALUCARD_PROMPTS", {
            let prompts = root.join("prompts");
            fs::create_dir(&prompts).unwrap();
            prompts
        })
        .env("ALUCARD_GH_TRACE", &gh_trace)
        .env("ALUCARD_REVIEW_VERDICT", case.file_verdict)
        .env("ALUCARD_CASE", case.name)
        .env("ALUCARD_FORMAL_VERDICT", case.formal_verdict)
        .env("ALUCARD_FORMAL_BODY", "formal body")
        .env("ALUCARD_PR_COMMENTS", case.comments_json)
        .env(
            "REVIEW_POLICY_MAX_BYTES",
            case.review_policy_max_bytes.unwrap_or(16_384).to_string(),
        )
        .env("ALUCARD_REVIEW_OUTCOMES", case.reviewer_outcomes)
        .env(
            "ALUCARD_FEEDBACK_OUTCOMES",
            match case.name {
                "feedback-blocked" => "push-blocked",
                "feedback-fails-after-push" => "push-fail",
                _ => "push",
            },
        )
        .env(
            "ALUCARD_PREFLIGHT_RC",
            if case.name == "feedback-toolchain-broken" {
                "23"
            } else {
                "0"
            },
        )
        .env(
            "ALUCARD_ADVANCE_HEAD",
            if case.advance_head { "1" } else { "0" },
        )
        .env("ALUCARD_ADVANCE_CHECKOUT", root.join("advance"))
        .args([
            "--db-path",
            database.to_str().unwrap(),
            "run",
            "--iterations",
            "1",
            "--tasks",
            tasks.to_str().unwrap(),
            "--env-file",
            environment.to_str().unwrap(),
            "--logs-root",
            root.join("logs").to_str().unwrap(),
            "--no-build",
            "--max-review-cycles",
            "2",
            repository.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    let connection = open(&database).unwrap();
    let result = connection
        .query_row(
            "SELECT review_verdict, gate_state, reviewer_approved_sha, head_sha FROM pull_requests WHERE number = 77",
            [],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?, row.get::<_, Option<String>>(2)?, row.get::<_, String>(3)?)),
        )
        .unwrap();
    assert_eq!(result.0, case.expected_verdict, "{}", case.name);
    assert_eq!(result.1, case.expected_state, "{}", case.name);
    drop(connection);
    (
        root,
        fs::read_to_string(review_args).unwrap(),
        fs::read_to_string(gh_trace).unwrap(),
        (result.2, result.3),
    )
}

#[test]
fn run_reconciles_reviewer_verdicts_and_injects_review_policy() {
    let cases = [
        ReviewCase {
            name: "review-formal-precedence",
            review_policy: Some("Review policy from the target repository.\n"),
            oversized_policy: false,
            review_policy_max_bytes: None,
            file_verdict: "APPROVED",
            formal_verdict: "CHANGES_REQUESTED",
            reviewer_outcomes: "verdict",
            comments_json: "{\"comments\":[]}",
            advance_head: false,
            expected_verdict: "CHANGES_REQUESTED",
            expected_state: "review_changes_requested",
        },
        ReviewCase {
            name: "review-blocked-precedence",
            review_policy: Some("Review policy from the target repository.\n"),
            oversized_policy: false,
            review_policy_max_bytes: None,
            file_verdict: "BLOCKED",
            formal_verdict: "APPROVED",
            reviewer_outcomes: "verdict",
            comments_json: "{\"comments\":[{\"body\":\"**🤖 Alucard blocked findings** (cycle 1/10)\\n\\nunfixable <finding>\"}]}",
            advance_head: false,
            expected_verdict: "BLOCKED",
            expected_state: "review_blocked",
        },
        ReviewCase {
            name: "review-missing-ci",
            review_policy: Some("Review policy from the target repository.\n"),
            oversized_policy: false,
            review_policy_max_bytes: None,
            file_verdict: "APPROVED",
            formal_verdict: "",
            reviewer_outcomes: "verdict",
            comments_json: "{\"comments\":[]}",
            advance_head: false,
            expected_verdict: "APPROVED",
            expected_state: "review_needs_human",
        },
        ReviewCase {
            name: "review-no-verdict",
            review_policy: Some("Review policy from the target repository.\n"),
            oversized_policy: false,
            review_policy_max_bytes: None,
            file_verdict: "not-a-verdict",
            formal_verdict: "",
            reviewer_outcomes: "exhausted",
            comments_json: "{\"comments\":[]}",
            advance_head: false,
            expected_verdict: "",
            expected_state: "review_missing",
        },
        ReviewCase {
            name: "review-transport-then-verdict",
            review_policy: Some("Review policy from the target repository.\n"),
            oversized_policy: false,
            review_policy_max_bytes: None,
            file_verdict: "APPROVED",
            formal_verdict: "",
            reviewer_outcomes: "transport\nverdict",
            comments_json: "{\"comments\":[]}",
            advance_head: false,
            expected_verdict: "APPROVED",
            expected_state: "review_needs_human",
        },
        ReviewCase {
            name: "review-no-worktree-then-verdict",
            review_policy: Some("Review policy from the target repository.\n"),
            oversized_policy: false,
            review_policy_max_bytes: None,
            file_verdict: "APPROVED",
            formal_verdict: "",
            reviewer_outcomes: "no-worktree\nverdict",
            comments_json: "{\"comments\":[]}",
            advance_head: false,
            expected_verdict: "APPROVED",
            expected_state: "review_needs_human",
        },
        ReviewCase {
            name: "review-transport-exhausted",
            review_policy: Some("Review policy from the target repository.\n"),
            oversized_policy: false,
            review_policy_max_bytes: None,
            file_verdict: "",
            formal_verdict: "",
            reviewer_outcomes: "transport\ntransport\ntransport",
            comments_json: "{\"comments\":[]}",
            advance_head: false,
            expected_verdict: "",
            expected_state: "review_missing",
        },
        ReviewCase {
            name: "review-no-worktree-exhausted",
            review_policy: Some("Review policy from the target repository.\n"),
            oversized_policy: false,
            review_policy_max_bytes: None,
            file_verdict: "",
            formal_verdict: "",
            reviewer_outcomes: "no-worktree\nno-worktree\nno-worktree",
            comments_json: "{\"comments\":[]}",
            advance_head: false,
            expected_verdict: "",
            expected_state: "review_missing",
        },
        ReviewCase {
            name: "review-transport-with-verdict",
            review_policy: Some("Review policy from the target repository.\n"),
            oversized_policy: false,
            review_policy_max_bytes: None,
            file_verdict: "APPROVED",
            formal_verdict: "",
            reviewer_outcomes: "transport-verdict",
            comments_json: "{\"comments\":[]}",
            advance_head: false,
            expected_verdict: "APPROVED",
            expected_state: "review_needs_human",
        },
        ReviewCase {
            name: "review-timeout",
            review_policy: Some("Review policy from the target repository.\n"),
            oversized_policy: false,
            review_policy_max_bytes: None,
            file_verdict: "",
            formal_verdict: "",
            reviewer_outcomes: "timeout",
            comments_json: "{\"comments\":[]}",
            advance_head: false,
            expected_verdict: "",
            expected_state: "review_missing",
        },
        ReviewCase {
            name: "review-no-policy",
            review_policy: None,
            oversized_policy: false,
            review_policy_max_bytes: None,
            file_verdict: "CHANGES_REQUESTED",
            formal_verdict: "",
            reviewer_outcomes: "verdict",
            comments_json: "{\"comments\":[]}",
            advance_head: false,
            expected_verdict: "CHANGES_REQUESTED",
            expected_state: "review_changes_requested",
        },
        ReviewCase {
            name: "review-bounded-markup-policy",
            review_policy: Some("ignored"),
            oversized_policy: true,
            review_policy_max_bytes: Some(32),
            file_verdict: "CHANGES_REQUESTED",
            formal_verdict: "",
            reviewer_outcomes: "verdict",
            comments_json: "{\"comments\":[]}",
            advance_head: false,
            expected_verdict: "CHANGES_REQUESTED",
            expected_state: "review_changes_requested",
        },
        ReviewCase {
            name: "review-head-advance",
            review_policy: Some("Review policy from the target repository.\n"),
            oversized_policy: false,
            review_policy_max_bytes: None,
            file_verdict: "APPROVED",
            formal_verdict: "",
            reviewer_outcomes: "verdict",
            comments_json: "{\"comments\":[]}",
            advance_head: true,
            expected_verdict: "APPROVED",
            expected_state: "review_needs_human",
        },
        ReviewCase {
            name: "review-formal-approved-only",
            review_policy: Some("Review policy from the target repository.\n"),
            oversized_policy: false,
            review_policy_max_bytes: None,
            file_verdict: "",
            formal_verdict: "APPROVED",
            reviewer_outcomes: "verdict",
            comments_json: "{\"comments\":[]}",
            advance_head: false,
            expected_verdict: "APPROVED",
            expected_state: "review_needs_human",
        },
        ReviewCase {
            name: "findings-handoff",
            review_policy: Some("Review policy from the target repository.\n"),
            oversized_policy: false,
            review_policy_max_bytes: None,
            file_verdict: "CHANGES_REQUESTED",
            formal_verdict: "CHANGES_REQUESTED",
            reviewer_outcomes: "verdict\nverdict",
            comments_json: "{\"comments\":[{\"body\":\"HUMAN-COMMENT\",\"author\":{\"login\":\"person\"},\"createdAt\":\"now\"}]}",
            advance_head: false,
            expected_verdict: "CHANGES_REQUESTED",
            expected_state: "review_changes_requested",
        },
        ReviewCase {
            name: "feedback-stall",
            review_policy: Some("Review policy from the target repository.\n"),
            oversized_policy: false,
            review_policy_max_bytes: None,
            file_verdict: "CHANGES_REQUESTED",
            formal_verdict: "",
            reviewer_outcomes: "verdict",
            comments_json: "{\"comments\":[]}",
            advance_head: false,
            expected_verdict: "CHANGES_REQUESTED",
            expected_state: "review_changes_requested",
        },
        ReviewCase {
            name: "feedback-retained",
            review_policy: Some("Review policy from the target repository.\n"),
            oversized_policy: false,
            review_policy_max_bytes: None,
            file_verdict: "CHANGES_REQUESTED",
            formal_verdict: "",
            reviewer_outcomes: "verdict",
            comments_json: "{\"comments\":[]}",
            advance_head: false,
            expected_verdict: "CHANGES_REQUESTED",
            expected_state: "review_changes_requested",
        },
        ReviewCase {
            name: "feedback-toolchain-ok",
            review_policy: Some("Review policy from the target repository.\n"),
            oversized_policy: false,
            review_policy_max_bytes: None,
            file_verdict: "CHANGES_REQUESTED",
            formal_verdict: "",
            reviewer_outcomes: "verdict\nverdict",
            comments_json: "{\"comments\":[]}",
            advance_head: false,
            expected_verdict: "CHANGES_REQUESTED",
            expected_state: "review_changes_requested",
        },
        ReviewCase {
            name: "feedback-toolchain-broken",
            review_policy: Some("Review policy from the target repository.\n"),
            oversized_policy: false,
            review_policy_max_bytes: None,
            file_verdict: "CHANGES_REQUESTED",
            formal_verdict: "",
            reviewer_outcomes: "verdict\nverdict",
            comments_json: "{\"comments\":[]}",
            advance_head: false,
            expected_verdict: "CHANGES_REQUESTED",
            expected_state: "review_changes_requested",
        },
        ReviewCase {
            name: "feedback-blocked",
            review_policy: Some("Review policy from the target repository.\n"),
            oversized_policy: false,
            review_policy_max_bytes: None,
            file_verdict: "CHANGES_REQUESTED",
            formal_verdict: "",
            reviewer_outcomes: "verdict\nverdict",
            comments_json: "{\"comments\":[]}",
            advance_head: false,
            expected_verdict: "CHANGES_REQUESTED",
            expected_state: "review_changes_requested",
        },
        ReviewCase {
            name: "feedback-fails-after-push",
            review_policy: Some("Review policy from the target repository.\n"),
            oversized_policy: false,
            review_policy_max_bytes: None,
            file_verdict: "CHANGES_REQUESTED",
            formal_verdict: "",
            reviewer_outcomes: "verdict\nverdict",
            comments_json: "{\"comments\":[]}",
            advance_head: false,
            expected_verdict: "CHANGES_REQUESTED",
            expected_state: "review_changes_requested",
        },
        ReviewCase {
            name: "feedback-cycle-exhaustion",
            review_policy: Some("Review policy from the target repository.\n"),
            oversized_policy: false,
            review_policy_max_bytes: None,
            file_verdict: "CHANGES_REQUESTED",
            formal_verdict: "",
            reviewer_outcomes: "verdict\nverdict",
            comments_json: "{\"comments\":[]}",
            advance_head: false,
            expected_verdict: "CHANGES_REQUESTED",
            expected_state: "review_changes_requested",
        },
    ];

    for case in &cases {
        let (root, prompt, trace, shas) = run_review_case(case);
        match case.name {
            "review-no-policy" => assert!(!prompt.contains("\n<review_policy>\n")),
            "review-bounded-markup-policy" => {
                assert!(prompt.contains("&lt;policy&gt;unsafe&lt;/policy&gt;"));
                assert!(prompt.contains("[alucard: truncated at 32"));
                assert!(!prompt.contains("tail"));
            }
            _ => assert!(
                prompt.contains("<review_policy>\nReview policy from the target repository.")
            ),
        }
        if matches!(
            case.name,
            "review-transport-then-verdict" | "review-no-worktree-then-verdict"
        ) {
            assert_eq!(
                fs::read_to_string(root.join("review-attempts"))
                    .unwrap()
                    .lines()
                    .count(),
                2
            );
        }
        if case.name == "review-blocked-precedence" {
            assert!(prompt.contains(
                "<known_blockers>\n**🤖 Alucard blocked findings** (cycle 1/10)\n\nunfixable &lt;finding&gt;\n</known_blockers>"
            ));
        }
        if matches!(
            case.name,
            "review-transport-exhausted" | "review-no-worktree-exhausted"
        ) {
            assert_eq!(
                fs::read_to_string(root.join("review-attempts"))
                    .unwrap()
                    .lines()
                    .count(),
                3
            );
            assert!(trace.contains(match case.name {
                "review-transport-exhausted" => "class=transport",
                "review-no-worktree-exhausted" => "class=no_worktree",
                _ => unreachable!(),
            }));
        }
        if case.name == "review-transport-with-verdict" {
            assert_eq!(
                fs::read_to_string(root.join("review-attempts"))
                    .unwrap()
                    .lines()
                    .count(),
                1
            );
        }
        if case.name == "review-timeout" {
            assert!(trace.contains("class=exhausted"));
            assert!(trace
                .contains("Raise `ALUCARD_REVIEWER_MAX_TURNS` / `ALUCARD_REVIEWER_MAX_BUDGET`"));
        }
        if case.name == "review-head-advance" {
            let approved_sha = shas.0.as_deref().unwrap();
            assert_ne!(approved_sha, shas.1);
            assert!(trace.contains(&format!("Reviewed head: `{approved_sha}`")));
        }
        if case.name == "review-formal-approved-only" {
            assert!(!trace.contains("Reviewed head:"));
            assert_eq!(shas.0, None);
        }
        if case.name == "findings-handoff" {
            let feedback = fs::read_to_string(root.join("prompts/feedback-1.txt")).unwrap();
            assert!(feedback.contains("KEEP-FINDING"), "{feedback}");
            assert!(feedback.contains("HUMAN-COMMENT"));
            assert!(!feedback.contains("DROP-TOO-LARGE"));
            assert!(!feedback.contains("DROP-NON-BLOCKING"));
            assert!(feedback.contains("No Python or Node manifest found"));
        }
        if matches!(
            case.name,
            "feedback-toolchain-ok" | "feedback-toolchain-broken"
        ) {
            let feedback = fs::read_to_string(root.join("prompts/feedback-1.txt")).unwrap();
            let expected = if case.name == "feedback-toolchain-ok" {
                "OK — `npm ci` at the repo root completes"
            } else {
                "BROKEN — `npm ci` at the repo root fails inside the container (rc=23)"
            };
            assert!(feedback.contains(expected));
        }
        if case.name == "feedback-blocked" {
            let second_review = fs::read_to_string(root.join("prompts/review-2.txt")).unwrap();
            assert!(trace.contains("Alucard blocked findings"));
            assert!(second_review.contains("blocked finding"));
        }
        if case.name == "feedback-fails-after-push" {
            assert!(trace.contains("feedback agent failed after pushing"));
            assert_eq!(
                fs::read_to_string(root.join("review-attempts"))
                    .unwrap()
                    .lines()
                    .count(),
                2
            );
        }
        if case.name == "feedback-cycle-exhaustion" {
            assert!(trace.contains("gave up after 2/2 review cycles — needs human"));
            assert_eq!(
                fs::read_to_string(root.join("feedback-attempts"))
                    .unwrap()
                    .lines()
                    .count(),
                1
            );
        }
        if case.name == "feedback-toolchain-ok" {
            let heads = fs::read_to_string(root.join("review-heads")).unwrap();
            let heads = heads.lines().collect::<Vec<_>>();
            assert_eq!(heads.len(), 2);
            assert_ne!(heads[0], heads[1]);
        }
        if matches!(case.name, "feedback-stall" | "feedback-retained") {
            assert!(trace.contains("feedback agent stopped without publishing"));
            assert_eq!(
                fs::read_to_string(root.join("review-attempts"))
                    .unwrap()
                    .lines()
                    .count(),
                1
            );
        }
        match case.expected_state {
            "review_blocked" => assert!(trace.contains("pr edit 77 --add-label needs-human")),
            "review_needs_human" => {
                assert!(trace.contains("pr edit 77 --add-label needs-human"));
                assert!(trace.contains("APPROVED — no CI ran, needs human"));
            }
            "review_missing" => {
                assert!(trace.contains(match case.name {
                    "review-transport-exhausted" => "class=transport",
                    "review-no-worktree-exhausted" => "class=no_worktree",
                    _ => "class=exhausted",
                }));
                assert!(trace.contains("no review posted — needs human"));
            }
            _ => assert!(trace.contains("CHANGES_REQUESTED")),
        }
        fs::remove_dir_all(root).unwrap();
    }
}
