use alucard::{
    build_inputs_hash, compose_role_prompt, AgentInvocation, AgentRole, AgentSettings, Provider,
};
use std::fs;
use std::path::Path;

fn settings(contents: &str) -> AgentSettings {
    let path =
        std::env::temp_dir().join(format!("alucard-agent-settings-{}.env", std::process::id()));
    fs::write(&path, contents).unwrap();
    let settings = AgentSettings::from_environment(Some(&path)).unwrap();
    fs::remove_file(path).unwrap();
    settings
}

#[test]
fn role_settings_and_docker_arguments_match_bash_contract() {
    let settings = settings(
        "ALUCARD_PROVIDER=claude\nALUCARD_CI_FIX_PROVIDER=codex\nALUCARD_CLAUDE_MODEL=opus\nALUCARD_WORKER_CLAUDE_MODEL=worker-opus\nALUCARD_CI_FIX_CODEX_MODEL=fix-model\nALUCARD_WORKER_MAX_TURNS=181\n",
    );
    assert_eq!(settings.provider(AgentRole::Worker), Ok(Provider::Claude));
    assert_eq!(settings.provider(AgentRole::CiFix), Ok(Provider::Codex));
    assert_eq!(
        settings.setting(AgentRole::Worker, "CLAUDE_MODEL", "sonnet"),
        "worker-opus"
    );
    assert_eq!(
        settings.setting(AgentRole::Reviewer, "CLAUDE_MODEL", "sonnet"),
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
    let args = invocation.docker_args(&settings).unwrap();
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
