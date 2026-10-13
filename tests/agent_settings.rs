use alucard::{
    build_inputs_hash, classify_agent_output, compose_role_prompt, AgentExit, AgentInvocation,
    AgentRole, AgentSettings, Provider,
};
use std::fs;
use std::path::Path;
use uuid::Uuid;

fn settings(contents: &str) -> AgentSettings {
    let path = std::env::temp_dir().join(format!("alucard-agent-settings-{}.env", Uuid::new_v4()));
    fs::write(&path, contents).unwrap();
    let settings = AgentSettings::from_environment(Some(&path)).unwrap();
    fs::remove_file(path).unwrap();
    settings
}

fn invocation(role: AgentRole) -> AgentInvocation<'static> {
    AgentInvocation {
        role,
        name: "alucard-test",
        image: "alucard:test",
        env_file: Path::new("/tmp/alucard.env"),
        worktree: Path::new("/tmp/worktree"),
        prompt: "prompt",
        timeout_minutes: 1,
        network: None,
        playwright_cache: None,
        database_url: None,
        github_repo: None,
    }
}

#[test]
fn role_settings_and_docker_arguments_match_bash_contract() {
    let agent_settings = settings(
        "ALUCARD_PROVIDER=claude\nALUCARD_CI_FIX_PROVIDER=codex\nALUCARD_CLAUDE_MODEL=opus\nALUCARD_WORKER_CLAUDE_MODEL=worker-opus\nALUCARD_CI_FIX_CODEX_MODEL=fix-model\nALUCARD_WORKER_MAX_TURNS=181\n",
    );
    assert_eq!(
        agent_settings.provider(AgentRole::Worker),
        Ok(Provider::Claude)
    );
    assert_eq!(
        agent_settings.provider(AgentRole::CiFix),
        Ok(Provider::Codex)
    );
    assert_eq!(
        agent_settings.setting(AgentRole::Worker, "CLAUDE_MODEL", "sonnet"),
        "worker-opus"
    );
    assert_eq!(
        agent_settings.setting(AgentRole::Reviewer, "CLAUDE_MODEL", "sonnet"),
        "opus"
    );

    let invocation = AgentInvocation {
        role: AgentRole::CiFix,
        name: "alucard-fix-1",
        image: "alucard:test",
        env_file: Path::new("/tmp/alucard.env"),
        worktree: Path::new("/tmp/worktree"),
        prompt: "prompt",
        timeout_minutes: 7,
        network: Some("alucard-test"),
        playwright_cache: Some(Path::new("/tmp/browsers")),
        database_url: Some("postgresql://postgres:postgres@postgres:5432/postgres"),
        github_repo: Some("example/repo"),
    };
    let args = invocation.docker_args(&agent_settings).unwrap();
    assert!(args
        .windows(2)
        .any(|pair| pair == ["--network", "alucard-test"]));
    assert!(args.windows(2).any(|pair| pair == ["-m", "fix-model"]));
    assert!(args.iter().any(|value| value == "GH_REPO=example/repo"));
    assert!(args
        .iter()
        .any(|value| value == "PLAYWRIGHT_BROWSERS_PATH=/opt/alucard-browsers"));
    assert!(
        args.iter()
            .any(|value| value
                == "DATABASE_URL=postgresql://postgres:postgres@postgres:5432/postgres")
    );
}

#[test]
fn blank_settings_fall_back_like_bash_parameter_expansion() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let example = AgentSettings::from_environment(Some(&root.join("alucard.env.example"))).unwrap();
    assert_eq!(example.provider(AgentRole::Worker), Ok(Provider::Claude));

    let configured = settings(
        "ALUCARD_PROVIDER=codex\nALUCARD_WORKER_PROVIDER=\nALUCARD_CODEX_MODEL=global\nALUCARD_WORKER_CODEX_MODEL=\nALUCARD_MAX_TURNS=190\nALUCARD_WORKER_MAX_TURNS=\nALUCARD_MAX_BUDGET=11\nALUCARD_WORKER_MAX_BUDGET=\nALUCARD_EFFORT=high\nALUCARD_WORKER_EFFORT=\n",
    );
    assert_eq!(configured.provider(AgentRole::Worker), Ok(Provider::Codex));
    assert_eq!(
        configured.setting(AgentRole::Worker, "CODEX_MODEL", "default"),
        "global"
    );
    assert_eq!(configured.turns(AgentRole::Worker), "180");
    assert_eq!(configured.budget(AgentRole::Worker), "10");
    assert_eq!(configured.setting(AgentRole::Worker, "EFFORT", ""), "high");

    let defaults = settings(
        "ALUCARD_PROVIDER=\nALUCARD_WORKER_PROVIDER=\nALUCARD_CODEX_MODEL=\nALUCARD_WORKER_CODEX_MODEL=\nALUCARD_MAX_TURNS=\nALUCARD_WORKER_MAX_TURNS=\nALUCARD_MAX_BUDGET=\nALUCARD_WORKER_MAX_BUDGET=\n",
    );
    assert_eq!(defaults.provider(AgentRole::Worker), Ok(Provider::Claude));
    assert_eq!(
        defaults.setting(AgentRole::Worker, "CODEX_MODEL", "default"),
        "gpt-5.6-terra"
    );
    assert_eq!(defaults.turns(AgentRole::Worker), "180");
    assert_eq!(defaults.budget(AgentRole::Worker), "10");
}

#[test]
fn claude_prompts_include_role_turn_budgets() {
    let agent_settings = settings(
        "ALUCARD_PROVIDER=claude\nALUCARD_WORKER_PROVIDER=claude\nALUCARD_CI_FIX_PROVIDER=claude\nALUCARD_REVIEWER_PROVIDER=claude\nALUCARD_FEEDBACK_PROVIDER=claude\nALUCARD_WORKER_MAX_TURNS=181\nALUCARD_CI_FIX_MAX_TURNS=31\nALUCARD_REVIEWER_MAX_TURNS=46\nALUCARD_FEEDBACK_MAX_TURNS=51\nALUCARD_WRAPUP_MAX_TURNS=13\n",
    );
    let cases = [
        (AgentRole::Worker, "181"),
        (AgentRole::CiFix, "31"),
        (AgentRole::Reviewer, "46"),
        (AgentRole::Feedback, "51"),
        (AgentRole::Wrapup, "13"),
    ];
    for (role, turns) in cases {
        let args = invocation(role).docker_args(&agent_settings).unwrap();
        assert_eq!(
            args.last(),
            Some(&format!(
                "prompt\n<turn_budget>\nThis run gets {turns} turns. The harness stops you the moment the cap is reached, wherever you are.\n</turn_budget>"
            ))
        );
    }

    let codex_settings = settings("ALUCARD_CI_FIX_PROVIDER=codex\n");
    assert_eq!(
        invocation(AgentRole::CiFix)
            .docker_args(&codex_settings)
            .unwrap()
            .last(),
        Some(&"-".to_string())
    );
}

#[test]
fn prompt_and_image_stamp_match_bash_inputs() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let prompt = compose_role_prompt(
        root,
        "alucard-worker-prompt.md",
        Some("alucard-worker-github-prompt.md"),
    )
    .unwrap();
    let expected = format!(
        "{}\n\n{}\n\n{}",
        fs::read_to_string(root.join("alucard-worker-prompt.md")).unwrap(),
        fs::read_to_string(root.join("alucard-engineering-policy.md")).unwrap(),
        fs::read_to_string(root.join("alucard-worker-github-prompt.md")).unwrap(),
    );
    assert_eq!(prompt, expected);
    assert_eq!(build_inputs_hash(root).unwrap().len(), 64);
}

#[test]
fn provider_result_classification_matches_bash_contract() {
    let cases = [
        (
            Provider::Claude,
            Some(1),
            r#"{"type":"result","subtype":"success","is_error":false,"result":"The billing connection was updated."}"#,
            AgentExit::Failed,
        ),
        (
            Provider::Claude,
            Some(0),
            r#"{"type":"result","subtype":"success","is_error":false,"api_error_status":429,"result":"Completed."}"#,
            AgentExit::Transport,
        ),
        (
            Provider::Claude,
            Some(1),
            r#"{"type":"result","subtype":"error","is_error":true,"api_error_status":503,"result":"Unavailable."}"#,
            AgentExit::Transport,
        ),
        (
            Provider::Claude,
            Some(1),
            r#"{"type":"result","subtype":"error","is_error":true,"result":"rate_limit exceeded."}"#,
            AgentExit::Transport,
        ),
        (
            Provider::Claude,
            Some(1),
            r#"{"type":"result","subtype":"error","is_error":true,"result":"ratelimit exceeded."}"#,
            AgentExit::Transport,
        ),
        (
            Provider::Claude,
            Some(1),
            r#"{"type":"result","subtype":"error_max_turns","is_error":true,"result":""}"#,
            AgentExit::Exhausted,
        ),
        (
            Provider::Codex,
            Some(1),
            r#"{"type":"item.completed","item":{"type":"error","message":"stream error: connection closed"}}"#,
            AgentExit::Transport,
        ),
        (
            Provider::Codex,
            Some(1),
            r#"{"type":"item.completed","item":{"type":"error","message":"provider ratelimit exceeded"}}"#,
            AgentExit::Transport,
        ),
        (
            Provider::Codex,
            Some(1),
            r#"{"type":"item.completed","item":{"type":"error","message":"provider rate/limit exceeded"}}"#,
            AgentExit::Transport,
        ),
    ];

    for (provider, status, fixture, expected) in cases {
        assert_eq!(classify_agent_output(provider, status, fixture), expected);
    }
}

#[test]
fn classifier_preserves_wedge_and_no_worktree_outcomes() {
    let wedge = r#"{"type":"stream_event","event":{"type":"content_block_delta","delta":{"type":"text_delta","text":"Shell cwd was reset to /work (deleted)"}}}"#;
    assert_eq!(
        classify_agent_output(Provider::Claude, Some(137), wedge),
        AgentExit::Wedged
    );
    for provider in [Provider::Claude, Provider::Codex] {
        assert_eq!(
            classify_agent_output(provider, Some(78), ""),
            AgentExit::NoWorktree
        );
    }
}
