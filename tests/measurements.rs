use alucard::{
    archive_prompt, extract_agent_usage, iteration_usage_comment, log_event, usage_report,
    write_iteration_measurement, write_stage_measurement, IterationMeasurement,
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
    fs::write(root.join("iter-10-review-1.jsonl"), claude).unwrap();

    assert_eq!(
        usage_report(&root),
        format!(
            "\n=== Alucard Token Usage: {} ===\nAgent                        Input    Output    Cache Read   Cache Write  Turns      Cost\n--------------------------------------------------------------------------------------\nworker 1-attempt-1            1000      2000         50000          9000     42    $1.234\nreview 1.2                    1000      2000         50000          9000     42    $1.234\nreview 1.2-attempt-2          1000      2000         50000          9000     42    $1.234\nreview 10.1                   1000      2000         50000          9000     42    $1.234\n--------------------------------------------------------------------------------------\nTOTAL                         4000      8000        200000         36000    168    $4.936\n\n",
            root.file_name().unwrap().to_str().unwrap()
        )
    );
    assert_eq!(
        iteration_usage_comment(&root, 1, Some(754)).unwrap(),
        "**🤖 Alucard token usage (iteration 1)**\n\nDuration: 12m 34s · Review rounds: 1 · Feedback rounds: 1\n\n<details><summary>Per-agent breakdown</summary>\n\n| Agent | Input | Output | Cache Read | Cache Write | Turns | Cost |\n|-------|------:|-------:|-----------:|------------:|------:|-----:|\n| worker attempt 1 | 1000 | 2000 | 50000 | 9000 | 42 | $1.234 |\n| review 1.2 | 1000 | 2000 | 50000 | 9000 | 42 | $1.234 |\n| review 1.2-attempt-2 | 1000 | 2000 | 50000 | 9000 | 42 | $1.234 |\n| **TOTAL** | **3000** | **6000** | **150000** | **27000** | **126** | **$3.702** |\n\n</details>"
    );
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
    let database = root.join("ledger.db");
    alucard::migrate(&database).unwrap();
    write_iteration_measurement(IterationMeasurement {
        database: &database,
        run_id: "unknown",
        log_dir: &root,
        iteration: 1,
        pr: None,
        base_sha: "base",
        status: Some(0),
        duration: 1,
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
    let measurement: serde_json::Value = serde_json::from_str(&measurements).unwrap();
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

    write_stage_measurement(&root, &root, "main", 1, None, &base, &head);

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

    write_stage_measurement(&root, &root, "main", 1, None, &base, &merge_head);

    let record: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(root.join("measurements.jsonl")).unwrap())
            .unwrap();
    assert_eq!(record["is_merge"], true);
    fs::remove_file(root.join("measurements.jsonl")).unwrap();

    write_stage_measurement(&root, &root, "main", 1, None, "missing-base", &merge_head);

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

    write_stage_measurement(&root, &root, "main", 1, None, &base, &head);

    let record: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(root.join("measurements.jsonl")).unwrap())
            .unwrap();
    assert_eq!(record["base_drifted"], true);
    fs::remove_dir_all(root).unwrap();
}
