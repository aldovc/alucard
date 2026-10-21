#[cfg_attr(
    any(target_os = "linux", target_os = "android"),
    link_section = ".init_array"
)]
#[cfg_attr(target_os = "macos", link_section = "__DATA,__mod_init_func")]
#[used]
static INITIALIZE: extern "C" fn() = initialize;

extern "C" fn initialize() {
    if std::env::var_os("ALUCARD_CI_POLL_INTERVAL").is_none() {
        std::env::set_var("ALUCARD_CI_POLL_INTERVAL", "0");
    }
    for key in [
        "ALUCARD_PROVIDER",
        "ALUCARD_WORKER_PROVIDER",
        "ALUCARD_CI_FIX_PROVIDER",
        "ALUCARD_REVIEWER_PROVIDER",
        "ALUCARD_FEEDBACK_PROVIDER",
        "ALUCARD_WRAPUP_PROVIDER",
    ] {
        std::env::remove_var(key);
    }
}

/// Points `repository`'s origin at `git@github.com:owner/repo.git`, served over a stub ssh from `remote`, with `https://github.com/` unreachable.
#[allow(dead_code)]
pub fn github_ssh_origin(
    root: &std::path::Path,
    repository: &std::path::Path,
    remote: &std::path::Path,
) -> [(&'static str, std::path::PathBuf); 2] {
    use std::os::unix::fs::PermissionsExt;
    let served = root.join("ssh");
    std::fs::create_dir_all(served.join("owner")).unwrap();
    std::os::unix::fs::symlink(remote, served.join("owner/repo.git")).unwrap();
    let ssh = root.join("ssh-stub");
    std::fs::write(
        &ssh,
        format!(
            "#!/bin/sh\nfor argument; do command=$argument; done\ncd '{}' && exec sh -c \"$command\"\n",
            served.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&ssh, std::fs::Permissions::from_mode(0o755)).unwrap();
    let config = root.join("gitconfig");
    std::fs::write(
        &config,
        "[url \"https://127.0.0.1:9/\"]\n\tinsteadOf = https://github.com/\n",
    )
    .unwrap();
    assert!(std::process::Command::new("git")
        .arg("-C")
        .arg(repository)
        .args([
            "remote",
            "set-url",
            "origin",
            "git@github.com:owner/repo.git"
        ])
        .status()
        .unwrap()
        .success());
    [("GIT_SSH_COMMAND", ssh), ("GIT_CONFIG_GLOBAL", config)]
}

/// Copies the required tool files and the built binary into `directory/tools`, returning the binary path.
#[allow(dead_code)]
pub fn copy_tool_installation(directory: &std::path::Path) -> std::path::PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let tools = directory.join("tools");
    let binary_dir = tools.join("bin");
    std::fs::create_dir_all(&binary_dir).unwrap();
    for file in include_str!("../required-tool-files.txt").lines() {
        std::fs::copy(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(file),
            tools.join(file),
        )
        .unwrap();
    }
    let binary = binary_dir.join("alucard");
    if std::fs::hard_link(env!("CARGO_BIN_EXE_alucard"), &binary).is_err() {
        std::fs::copy(env!("CARGO_BIN_EXE_alucard"), &binary).unwrap();
        std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    binary
}
