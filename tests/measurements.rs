use alucard::{
    archive_prompt, extract_agent_usage, log_event, write_iteration_measurement,
    IterationMeasurement,
};
use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

fn temporary_path(name: &str) -> PathBuf {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!("alucard-{name}-{}-{stamp}", std::process::id()))
}

#[test]
fn extracts_claude_and_codex_usage_without_inventing_a_cost() {
    let root = temporary_path("usage");
    fs::create_dir_all(&root).unwrap();
    let claude = root.join("claude.jsonl");
    fs::write(&claude, r#"{"type":"result","num_turns":2,"total_cost_usd":1.5,"modelUsage":{"m":{"inputTokens":10,"outputTokens":4,"cacheReadInputTokens":3,"cacheCreationInputTokens":2}}}"#).unwrap();
    assert_eq!(extract_agent_usage(&claude).unwrap().cost, Some(1.5));
    let codex = root.join("codex.jsonl");
    fs::write(&codex, "{\"type\":\"turn.completed\",\"usage\":{\"input_tokens\":10,\"cached_input_tokens\":3,\"output_tokens\":4}}\n{\"type\":\"item.completed\",\"item\":{\"type\":\"command_execution\"}}\n").unwrap();
    let usage = extract_agent_usage(&codex).unwrap();
    assert_eq!((usage.input, usage.cache_read, usage.cost), (7, 3, None));
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
    assert!(fs::read_to_string(root.join("events.log"))
        .unwrap()
        .contains("worker started"));
    assert_eq!(
        fs::read_to_string(root.join("prompts/iter-1.txt")).unwrap(),
        "prompt body"
    );
    let measurements = fs::read_to_string(root.join("measurements.jsonl")).unwrap();
    assert!(measurements.contains("\"usage_missing\":0"));
    assert!(measurements.contains("\"cost_complete\":false"));
    fs::remove_dir_all(root).unwrap();
}
