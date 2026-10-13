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
    file_verdict: &'a str,
    formal_verdict: &'a str,
    reviewer_outcomes: &'a str,
    advance_head: bool,
    expected_verdict: &'a str,
    expected_state: &'a str,
}

fn run_review_case(case: &ReviewCase<'_>) -> (PathBuf, String, String, (String, String)) {
    let root = temporary_path(case.name);
    let repository = root.join("repository");
    let remote = root.join("remote.git");
    let bin = root.join("bin");
    fs::create_dir_all(&bin).unwrap();
    fs::create_dir(&repository).unwrap();
    git_repository(&repository);
    if let Some(policy) = case.review_policy {
        fs::write(
            repository.join("REVIEW.md"),
            if case.oversized_policy {
                format!("<policy>unsafe</policy>\n{}tail", "x".repeat(16_384))
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
work= output=
for argument in "$@"; do
  case "$argument" in
    *:/work:rw) work=${argument%:/work:rw} ;;
    *:/work-output:rw) output=${argument%:/work-output:rw} ;;
  esac
done
if [ -n "$output" ]; then
  printf '%s\n' "$@" >> "$ALUCARD_REVIEW_ARGS"
  attempt=$(($(wc -l < "$ALUCARD_REVIEW_ATTEMPTS") + 1))
  printf '%s\n' "$attempt" >> "$ALUCARD_REVIEW_ATTEMPTS"
  outcome=$(printf '%s\n' "$ALUCARD_REVIEW_OUTCOMES" | sed -n "${attempt}p")
  if [ "$outcome" = verdict ] || [ "$outcome" = transport-verdict ]; then
    printf '%s\n' "$ALUCARD_REVIEW_VERDICT" > "$output/.alucard-review"
    printf 'review body\n' > "$output/.alucard-review-body"
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
        .env("ALUCARD_GH_TRACE", &gh_trace)
        .env("ALUCARD_REVIEW_VERDICT", case.file_verdict)
        .env("ALUCARD_FORMAL_VERDICT", case.formal_verdict)
        .env("ALUCARD_FORMAL_BODY", "formal body")
        .env("ALUCARD_REVIEW_OUTCOMES", case.reviewer_outcomes)
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
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?, row.get::<_, Option<String>>(2)?.unwrap_or_default(), row.get::<_, String>(3)?)),
        )
        .unwrap();
    assert_eq!(result.0, case.expected_verdict);
    assert_eq!(result.1, case.expected_state);
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
            file_verdict: "APPROVED",
            formal_verdict: "CHANGES_REQUESTED",
            reviewer_outcomes: "verdict",
            advance_head: false,
            expected_verdict: "CHANGES_REQUESTED",
            expected_state: "review_changes_requested",
        },
        ReviewCase {
            name: "review-blocked-precedence",
            review_policy: Some("Review policy from the target repository.\n"),
            oversized_policy: false,
            file_verdict: "BLOCKED",
            formal_verdict: "APPROVED",
            reviewer_outcomes: "verdict",
            advance_head: false,
            expected_verdict: "BLOCKED",
            expected_state: "review_blocked",
        },
        ReviewCase {
            name: "review-missing-ci",
            review_policy: Some("Review policy from the target repository.\n"),
            oversized_policy: false,
            file_verdict: "APPROVED",
            formal_verdict: "",
            reviewer_outcomes: "verdict",
            advance_head: false,
            expected_verdict: "APPROVED",
            expected_state: "review_needs_human",
        },
        ReviewCase {
            name: "review-no-verdict",
            review_policy: Some("Review policy from the target repository.\n"),
            oversized_policy: false,
            file_verdict: "not-a-verdict",
            formal_verdict: "",
            reviewer_outcomes: "exhausted",
            advance_head: false,
            expected_verdict: "",
            expected_state: "review_missing",
        },
        ReviewCase {
            name: "review-transport-then-verdict",
            review_policy: Some("Review policy from the target repository.\n"),
            oversized_policy: false,
            file_verdict: "APPROVED",
            formal_verdict: "",
            reviewer_outcomes: "transport\nverdict",
            advance_head: false,
            expected_verdict: "APPROVED",
            expected_state: "review_needs_human",
        },
        ReviewCase {
            name: "review-transport-exhausted",
            review_policy: Some("Review policy from the target repository.\n"),
            oversized_policy: false,
            file_verdict: "",
            formal_verdict: "",
            reviewer_outcomes: "transport\ntransport\ntransport",
            advance_head: false,
            expected_verdict: "",
            expected_state: "review_missing",
        },
        ReviewCase {
            name: "review-transport-with-verdict",
            review_policy: Some("Review policy from the target repository.\n"),
            oversized_policy: false,
            file_verdict: "APPROVED",
            formal_verdict: "",
            reviewer_outcomes: "transport-verdict",
            advance_head: false,
            expected_verdict: "APPROVED",
            expected_state: "review_needs_human",
        },
        ReviewCase {
            name: "review-timeout",
            review_policy: Some("Review policy from the target repository.\n"),
            oversized_policy: false,
            file_verdict: "",
            formal_verdict: "",
            reviewer_outcomes: "timeout",
            advance_head: false,
            expected_verdict: "",
            expected_state: "review_missing",
        },
        ReviewCase {
            name: "review-no-policy",
            review_policy: None,
            oversized_policy: false,
            file_verdict: "CHANGES_REQUESTED",
            formal_verdict: "",
            reviewer_outcomes: "verdict",
            advance_head: false,
            expected_verdict: "CHANGES_REQUESTED",
            expected_state: "review_changes_requested",
        },
        ReviewCase {
            name: "review-bounded-markup-policy",
            review_policy: Some("ignored"),
            oversized_policy: true,
            file_verdict: "CHANGES_REQUESTED",
            formal_verdict: "",
            reviewer_outcomes: "verdict",
            advance_head: false,
            expected_verdict: "CHANGES_REQUESTED",
            expected_state: "review_changes_requested",
        },
        ReviewCase {
            name: "review-head-advance",
            review_policy: Some("Review policy from the target repository.\n"),
            oversized_policy: false,
            file_verdict: "APPROVED",
            formal_verdict: "",
            reviewer_outcomes: "verdict",
            advance_head: true,
            expected_verdict: "APPROVED",
            expected_state: "review_needs_human",
        },
    ];

    for case in &cases {
        let (root, prompt, trace, shas) = run_review_case(case);
        match case.name {
            "review-no-policy" => assert!(!prompt.contains("\n<review_policy>\n")),
            "review-bounded-markup-policy" => {
                assert!(prompt.contains("&lt;policy&gt;unsafe&lt;/policy&gt;"));
                assert!(prompt.contains("[alucard: truncated at 16384"));
                assert!(!prompt.contains("tail"));
            }
            _ => assert!(
                prompt.contains("<review_policy>\nReview policy from the target repository.")
            ),
        }
        if case.name == "review-transport-then-verdict" {
            assert_eq!(
                fs::read_to_string(root.join("review-attempts"))
                    .unwrap()
                    .lines()
                    .count(),
                2
            );
        }
        if case.name == "review-transport-exhausted" {
            assert_eq!(
                fs::read_to_string(root.join("review-attempts"))
                    .unwrap()
                    .lines()
                    .count(),
                3
            );
            assert!(trace.contains("class=transport"));
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
            assert!(!shas.0.is_empty());
            assert_ne!(shas.0, shas.1);
            assert!(trace.contains(&format!("Reviewed head: `{}`", shas.0)));
        }
        match case.expected_state {
            "review_blocked" => assert!(trace.contains("pr edit 77 --add-label needs-human")),
            "review_needs_human" => {
                assert!(trace.contains("pr edit 77 --add-label needs-human"));
                assert!(trace.contains("APPROVED — no CI ran, needs human"));
            }
            "review_missing" => {
                assert!(
                    trace.contains(if case.name == "review-transport-exhausted" {
                        "class=transport"
                    } else {
                        "class=exhausted"
                    })
                );
                assert!(trace.contains("no review posted — needs human"));
            }
            _ => assert!(trace.contains("CHANGES_REQUESTED")),
        }
        fs::remove_dir_all(root).unwrap();
    }
}
