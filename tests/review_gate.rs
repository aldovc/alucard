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
    file_verdict: &'a str,
    formal_verdict: &'a str,
    reviewer_log: &'a str,
    reviewer_exit: &'a str,
    expected_verdict: &'a str,
    expected_state: &'a str,
}

fn run_review_case(case: &ReviewCase<'_>) -> (PathBuf, String, String) {
    let root = temporary_path(case.name);
    let repository = root.join("repository");
    let remote = root.join("remote.git");
    let bin = root.join("bin");
    fs::create_dir_all(&bin).unwrap();
    fs::create_dir(&repository).unwrap();
    git_repository(&repository);
    fs::write(
        repository.join("REVIEW.md"),
        "Review policy from the target repository.\n",
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
  printf '%s\n' "$@" > "$ALUCARD_REVIEW_ARGS"
  if [ -n "$ALUCARD_REVIEW_VERDICT" ]; then
    printf '%s\n' "$ALUCARD_REVIEW_VERDICT" > "$output/.alucard-review"
    printf 'review body\n' > "$output/.alucard-review-body"
  fi
  printf '%s\n' "$ALUCARD_REVIEW_LOG"
  exit "$ALUCARD_REVIEW_EXIT"
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
        .env("ALUCARD_GH_TRACE", &gh_trace)
        .env("ALUCARD_REVIEW_VERDICT", case.file_verdict)
        .env("ALUCARD_FORMAL_VERDICT", case.formal_verdict)
        .env("ALUCARD_FORMAL_BODY", "formal body")
        .env("ALUCARD_REVIEW_LOG", case.reviewer_log)
        .env("ALUCARD_REVIEW_EXIT", case.reviewer_exit)
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
            "SELECT review_verdict, gate_state FROM pull_requests WHERE number = 77",
            [],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        )
        .unwrap();
    assert_eq!(result.0, case.expected_verdict);
    assert_eq!(result.1, case.expected_state);
    drop(connection);
    (
        root,
        fs::read_to_string(review_args).unwrap(),
        fs::read_to_string(gh_trace).unwrap(),
    )
}

#[test]
fn run_reconciles_reviewer_verdicts_and_injects_review_policy() {
    let cases = [
        ReviewCase {
            name: "review-formal-precedence",
            file_verdict: "APPROVED",
            formal_verdict: "CHANGES_REQUESTED",
            reviewer_log: "{\"type\":\"result\",\"subtype\":\"success\",\"is_error\":false}",
            reviewer_exit: "0",
            expected_verdict: "CHANGES_REQUESTED",
            expected_state: "review_changes_requested",
        },
        ReviewCase {
            name: "review-blocked-precedence",
            file_verdict: "BLOCKED",
            formal_verdict: "APPROVED",
            reviewer_log: "{\"type\":\"result\",\"subtype\":\"success\",\"is_error\":false}",
            reviewer_exit: "0",
            expected_verdict: "BLOCKED",
            expected_state: "review_blocked",
        },
        ReviewCase {
            name: "review-missing-ci",
            file_verdict: "APPROVED",
            formal_verdict: "",
            reviewer_log: "{\"type\":\"result\",\"subtype\":\"success\",\"is_error\":false}",
            reviewer_exit: "0",
            expected_verdict: "APPROVED",
            expected_state: "review_needs_human",
        },
        ReviewCase {
            name: "review-no-verdict",
            file_verdict: "not-a-verdict",
            formal_verdict: "",
            reviewer_log: "{\"type\":\"result\",\"subtype\":\"error_max_turns\",\"is_error\":true}",
            reviewer_exit: "1",
            expected_verdict: "",
            expected_state: "review_missing",
        },
    ];

    for case in &cases {
        let (root, prompt, trace) = run_review_case(case);
        assert!(prompt.contains("<review_policy>\nReview policy from the target repository."));
        match case.expected_state {
            "review_blocked" => assert!(trace.contains("pr edit 77 --add-label needs-human")),
            "review_needs_human" => {
                assert!(trace.contains("pr edit 77 --add-label needs-human"));
                assert!(trace.contains("APPROVED — no CI ran, needs human"));
            }
            "review_missing" => {
                assert!(trace.contains("class=exhausted"));
                assert!(trace.contains("no review posted — needs human"));
            }
            _ => assert!(trace.contains("CHANGES_REQUESTED")),
        }
        fs::remove_dir_all(root).unwrap();
    }
}
