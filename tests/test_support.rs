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
