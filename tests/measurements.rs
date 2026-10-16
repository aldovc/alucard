mod test_support;

use alucard::{
    archive_prompt, extract_agent_usage, iteration_usage_comment, iteration_usage_comment_for_dirs,
    log_event, usage_report, usage_report_for_dirs, write_iteration_measurement,
    write_stage_measurement, IterationMeasurement, StageMeasurement,
};
use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

fn temporary_path(name: &str) -> PathBuf {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!("alucard-{name}-{}-{stamp}", std::process::id()))
}

fn git(repository: &PathBuf, args: &[&str]) -> String {
    let output = Command::new("git")
        .args(args)
        .current_dir(repository)
        .output()
        .unwrap();
    assert!(output.status.success());
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

#[test]
fn extracts_claude_and_codex_usage() {
    let root = temporary_path("usage");
    fs::create_dir_all(&root).unwrap();
    let claude = root.join("claude.jsonl");
    fs::write(&claude, r#"{"type":"result","num_turns":2,"total_cost_usd":1.5,"modelUsage":{"m":{"inputTokens":10,"outputTokens":4,"cacheReadInputTokens":3,"cacheCreationInputTokens":2}}}"#).unwrap();
    assert_eq!(extract_agent_usage(&claude).unwrap().cost, Some(1.5));
    let codex = root.join("codex.jsonl");
    fs::write(&codex, "{\"type\":\"turn.completed\",\"usage\":{\"input_tokens\":10,\"cached_input_tokens\":3,\"output_tokens\":4}}\n{\"type\":\"item.completed\",\"item\":{\"type\":\"command_execution\"}}\n").unwrap();
    let usage = extract_agent_usage(&codex).unwrap();
    assert_eq!((usage.input, usage.cache_read, usage.cost), (7, 3, None));
    let codex_cache_write = root.join("codex-cache-write.jsonl");
    fs::write(&codex_cache_write, "{\"type\":\"turn.completed\",\"usage\":{\"input_tokens\":10817,\"cached_input_tokens\":0,\"cache_write_input_tokens\":10814,\"output_tokens\":5}}\n{\"type\":\"item.completed\",\"item\":{\"type\":\"command_execution\"}}\n").unwrap();
    let usage = extract_agent_usage(&codex_cache_write).unwrap();
    assert_eq!(
        (
            usage.input,
            usage.output,
            usage.cache_read,
            usage.cache_write,
            usage.turns
        ),
        (10817, 5, 0, 10814, 1)
    );
    let codex_multiple = root.join("codex-multiple.jsonl");
    fs::write(&codex_multiple, "{\"type\":\"turn.completed\",\"usage\":{\"input_tokens\":1000,\"cached_input_tokens\":400,\"output_tokens\":100}}\n{\"type\":\"item.completed\",\"item\":{\"type\":\"command_execution\"}}\n{\"type\":\"turn.completed\",\"usage\":{\"input_tokens\":2000,\"cached_input_tokens\":600,\"output_tokens\":200}}\n{\"type\":\"item.completed\",\"item\":{\"type\":\"command_execution\"}}\n").unwrap();
    let usage = extract_agent_usage(&codex_multiple).unwrap();
    assert_eq!(
        (usage.input, usage.output, usage.cache_read, usage.turns),
        (2000, 300, 1000, 2)
    );
    assert!(extract_agent_usage(&root.join("missing.jsonl")).is_none());
    let empty = root.join("empty.jsonl");
    fs::write(&empty, "{\"type\":\"system\"}\n").unwrap();
    assert!(extract_agent_usage(&empty).is_none());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn formats_bash_compatible_usage_table_and_iteration_comment() {
    let root = temporary_path("usage-report");
    fs::create_dir_all(&root).unwrap();
    let claude = r#"{"type":"result","num_turns":42,"total_cost_usd":1.234,"modelUsage":{"claude-sonnet-4-5":{"inputTokens":1000,"outputTokens":2000,"cacheReadInputTokens":50000,"cacheCreationInputTokens":9000}}}"#;
    fs::write(root.join("iter-1-attempt-1.jsonl"), claude).unwrap();
    fs::write(root.join("iter-1-review-2.jsonl"), claude).unwrap();
    fs::write(root.join("iter-1-review-2-attempt-2.jsonl"), claude).unwrap();
    fs::write(root.join("iter-1-feedback-2.jsonl"), "").unwrap();
    let stale_claude_then_codex = format!(
        "{{\"type\":\"result\",\"num_turns\":99,\"total_cost_usd\":9.99,\"modelUsage\":{{\"claude\":{{\"inputTokens\":999}}}}}}\n{}{{\"type\":\"turn.completed\",\"usage\":{{\"input_tokens\":10,\"cached_input_tokens\":3,\"output_tokens\":4}}}}\n{{\"type\":\"item.completed\",\"item\":{{\"type\":\"command_execution\"}}}}\n",
        "{\"type\":\"system\"}\n".repeat(19)
    );
    fs::write(
        root.join("iter-1-feedback-3.jsonl"),
        stale_claude_then_codex,
    )
    .unwrap();
    fs::write(root.join("iter-10-review-1.jsonl"), claude).unwrap();

    let comment = root.join("comment");
    let report = root.join("report");
    let gh = root.join("gh");
    fs::write(
        &gh,
        "#!/bin/sh\ncase \"$1 $2\" in\n  'pr view') printf '%s\\n' '' ;;\n  'pr comment') printf '%s' \"$5\" > \"$ALUCARD_COMMENT\" ;;\nesac\n",
    )
    .unwrap();
    #[cfg(unix)]
    fs::set_permissions(&gh, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    let output = Command::new("bash")
        .args([
            "-c",
            r#"source "$1/alucard"
print_usage_report "$2" > "$3"
post_iteration_usage 1 754 "$1" "$2" 754"#,
            "bash",
            env!("CARGO_MANIFEST_DIR"),
            root.to_str().unwrap(),
            report.to_str().unwrap(),
        ])
        .env("ALUCARD_COMMENT", &comment)
        .env(
            "PATH",
            format!("{}:{}", root.display(), std::env::var("PATH").unwrap()),
        )
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    assert_eq!(usage_report(&root), fs::read_to_string(&report).unwrap());
    assert_eq!(
        iteration_usage_comment(&root, 1, Some(754)).unwrap(),
        fs::read_to_string(&comment).unwrap()
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn transport_retry_usage_includes_every_attempt_in_table_and_comment() {
    let root = temporary_path("transport-retry-usage");
    let first = root.join("first-attempt");
    let final_attempt = root.join("final-attempt");
    fs::create_dir_all(&first).unwrap();
    fs::create_dir_all(&final_attempt).unwrap();
    fs::write(
        first.join("iter-1-attempt-1.jsonl"),
        r#"{"type":"result","num_turns":1,"total_cost_usd":0.1,"modelUsage":{"m":{"inputTokens":10,"outputTokens":4}}}"#,
    )
    .unwrap();
    fs::write(
        final_attempt.join("iter-1-attempt-2.jsonl"),
        r#"{"type":"result","num_turns":2,"total_cost_usd":0.2,"modelUsage":{"m":{"inputTokens":20,"outputTokens":5}}}"#,
    )
    .unwrap();
    let logs = vec![first, final_attempt];

    let table = usage_report_for_dirs(&logs);
    assert!(table.contains("worker 1-attempt-1"));
    assert!(table.contains("worker 1-attempt-2"));
    let total = table
        .lines()
        .find(|line| line.starts_with("TOTAL"))
        .unwrap();
    assert_eq!(
        total.split_whitespace().collect::<Vec<_>>(),
        ["TOTAL", "30", "9", "0", "0", "3", "$0.300"]
    );

    let comment = iteration_usage_comment_for_dirs(&logs, 1, None).unwrap();
    assert!(comment.contains("| worker attempt 1 | 10 | 4 | 0 | 0 | 1 | $0.100 |"));
    assert!(comment.contains("| worker attempt 2 | 20 | 5 | 0 | 0 | 2 | $0.200 |"));
    assert!(comment.contains("| **TOTAL** | **30** | **9** | **0** | **0** | **3** | **$0.300** |"));

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn events_prompts_and_usage_summary_are_local_artifacts() {
    let root = temporary_path("artifacts");
    fs::create_dir_all(&root).unwrap();
    log_event(&root, "worker started");
    archive_prompt(&root, "iter-1", "prompt body");
    fs::write(
        root.join("iter-1-attempt-1.jsonl"),
        "{\"type\":\"turn.completed\",\"usage\":{\"input_tokens\":1}}\n",
    )
    .unwrap();
    let retry = root.join("retry");
    fs::create_dir(&retry).unwrap();
    fs::write(
        retry.join("iter-1-attempt-2.jsonl"),
        "{\"type\":\"turn.completed\",\"usage\":{\"input_tokens\":2}}\n",
    )
    .unwrap();
    let database = root.join("ledger.db");
    alucard::migrate(&database).unwrap();
    write_iteration_measurement(IterationMeasurement {
        database: &database,
        run_id: "unknown",
        log_dir: &root,
        log_dirs: &[&root, &retry],
        iteration: 1,
        pr: None,
        base_sha: "base",
        status: Some(0),
        duration: 1,
        ci_result: "green",
        review_verdict: "APPROVED",
        review_cycles: 1,
    });
    let event = fs::read_to_string(root.join("events.log")).unwrap();
    let (timestamp, message) = event.trim_end().split_once('\t').unwrap();
    assert_eq!(message, "worker started");
    assert_eq!(timestamp.len(), 20);
    assert!(timestamp
        .bytes()
        .enumerate()
        .all(|(index, byte)| matches!(index, 4 | 7 | 10 | 13 | 16 | 19) || byte.is_ascii_digit()));
    assert_eq!(
        [
            timestamp.as_bytes()[4],
            timestamp.as_bytes()[7],
            timestamp.as_bytes()[10],
            timestamp.as_bytes()[13],
            timestamp.as_bytes()[16],
            timestamp.as_bytes()[19],
        ],
        [b'-', b'-', b'T', b':', b':', b'Z']
    );
    assert_eq!(
        fs::read_to_string(root.join("prompts/iter-1.txt")).unwrap(),
        "prompt body"
    );
    let measurements = fs::read_to_string(root.join("measurements.jsonl")).unwrap();
    assert!(measurements.contains("\"usage_missing\":0"));
    assert!(measurements.contains("\"cost_complete\":false"));
    assert!(measurements.contains("\"ci_result\":\"green\""));
    assert!(measurements.contains("\"review_verdict\":\"APPROVED\""));
    let measurement: serde_json::Value = serde_json::from_str(&measurements).unwrap();
    assert_eq!(measurement["roles"]["worker"]["invocations"], 2);
    let timestamp = measurement["ts"].as_str().unwrap();
    assert_eq!(timestamp.len(), 20);
    assert!(timestamp
        .bytes()
        .enumerate()
        .all(|(index, byte)| matches!(index, 4 | 7 | 10 | 13 | 16 | 19) || byte.is_ascii_digit()));
    assert_eq!(
        [
            timestamp.as_bytes()[4],
            timestamp.as_bytes()[7],
            timestamp.as_bytes()[10],
            timestamp.as_bytes()[13],
            timestamp.as_bytes()[16],
            timestamp.as_bytes()[19],
        ],
        [b'-', b'-', b'T', b':', b':', b'Z']
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn stage_measurement_buckets_test_paths_and_renames() {
    let root = temporary_path("diff");
    fs::create_dir_all(root.join("tests")).unwrap();
    git(&root, &["init", "-q"]);
    git(&root, &["config", "user.email", "test@example.com"]);
    git(&root, &["config", "user.name", "Test User"]);
    fs::write(root.join("tests/original.rs"), "fn original() {}\n").unwrap();
    git(&root, &["add", "."]);
    git(&root, &["commit", "-qm", "base"]);
    let base = git(&root, &["rev-parse", "HEAD"]);
    fs::rename(
        root.join("tests/original.rs"),
        root.join("tests/renamed.rs"),
    )
    .unwrap();
    fs::write(root.join("tests/new.rs"), "fn new_test() {}\n").unwrap();
    git(&root, &["add", "."]);
    git(&root, &["commit", "-qm", "changes"]);
    let head = git(&root, &["rev-parse", "HEAD"]);

    write_stage_measurement(
        &root,
        &root,
        "main",
        StageMeasurement {
            iteration: 1,
            pr: None,
            stage: "worker",
            cycle: 0,
            base_sha: &base,
            prev_sha: &base,
            head_sha: &head,
        },
    );

    let record: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(root.join("measurements.jsonl")).unwrap())
            .unwrap();
    let diff = &record["from_base"];
    assert_eq!(diff["tests"]["added"], 1);
    assert_eq!(diff["renames"], 1);
    assert!(diff["paths"]
        .as_array()
        .unwrap()
        .iter()
        .any(|path| path == "tests/renamed.rs"));
    assert_eq!(record["prev_sha"], base);
    assert_eq!(record["from_prev"], record["from_base"]);
    assert_eq!(record["is_merge"], false);

    let worker_branch = git(&root, &["branch", "--show-current"]);
    git(&root, &["switch", "-qc", "side", &base]);
    fs::write(root.join("side.txt"), "side\n").unwrap();
    git(&root, &["add", "."]);
    git(&root, &["commit", "-qm", "side"]);
    git(&root, &["switch", "-q", &worker_branch]);
    git(&root, &["merge", "--no-ff", "side", "-m", "merge side"]);
    let merge_head = git(&root, &["rev-parse", "HEAD"]);
    fs::remove_file(root.join("measurements.jsonl")).unwrap();

    write_stage_measurement(
        &root,
        &root,
        "main",
        StageMeasurement {
            iteration: 1,
            pr: None,
            stage: "worker",
            cycle: 0,
            base_sha: &base,
            prev_sha: &base,
            head_sha: &merge_head,
        },
    );

    let record: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(root.join("measurements.jsonl")).unwrap())
            .unwrap();
    assert_eq!(record["is_merge"], true);
    fs::remove_file(root.join("measurements.jsonl")).unwrap();

    write_stage_measurement(
        &root,
        &root,
        "main",
        StageMeasurement {
            iteration: 1,
            pr: None,
            stage: "worker",
            cycle: 0,
            base_sha: "missing-base",
            prev_sha: "missing-base",
            head_sha: &merge_head,
        },
    );

    let record: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(root.join("measurements.jsonl")).unwrap())
            .unwrap();
    let diff = &record["from_base"];
    assert_eq!(diff["unavailable"], true);
    assert_eq!(diff["from"], "missing-base");
    assert_eq!(diff["to"], merge_head);
    let fields = diff.as_object().unwrap();
    assert_eq!(fields.len(), 4);
    assert!(fields["reason"].is_string());
    assert!(fields["from"].is_string());
    assert!(fields["to"].is_string());
    assert_eq!(record["from_prev"], record["from_base"]);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn stage_measurement_reports_when_the_tracked_base_advances() {
    let root = temporary_path("base-drift");
    fs::create_dir_all(&root).unwrap();
    git(&root, &["init", "-q", "-b", "main"]);
    git(&root, &["config", "user.email", "test@example.com"]);
    git(&root, &["config", "user.name", "Test User"]);
    fs::write(root.join("base.txt"), "base\n").unwrap();
    git(&root, &["add", "."]);
    git(&root, &["commit", "-qm", "base"]);
    let base = git(&root, &["rev-parse", "HEAD"]);
    git(&root, &["update-ref", "refs/remotes/origin/main", &base]);
    git(&root, &["switch", "-qc", "worker"]);
    fs::write(root.join("worker.txt"), "worker\n").unwrap();
    git(&root, &["add", "."]);
    git(&root, &["commit", "-qm", "worker"]);
    let head = git(&root, &["rev-parse", "HEAD"]);
    git(&root, &["switch", "-q", "main"]);
    fs::write(root.join("base.txt"), "advanced\n").unwrap();
    git(&root, &["commit", "-am", "advance base", "-q"]);
    let advanced_base = git(&root, &["rev-parse", "HEAD"]);
    git(
        &root,
        &["update-ref", "refs/remotes/origin/main", &advanced_base],
    );
    git(&root, &["switch", "-q", "worker"]);

    write_stage_measurement(
        &root,
        &root,
        "main",
        StageMeasurement {
            iteration: 1,
            pr: None,
            stage: "worker",
            cycle: 0,
            base_sha: &base,
            prev_sha: &base,
            head_sha: &head,
        },
    );

    let record: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(root.join("measurements.jsonl")).unwrap())
            .unwrap();
    assert_eq!(record["base_drifted"], true);
    fs::remove_dir_all(root).unwrap();
}
