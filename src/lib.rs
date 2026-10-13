//! SQLite ledger support for the Alucard orchestrator.

use fs2::FileExt;
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::env;
use std::ffi::CStr;
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::thread;
use std::time::{Duration, Instant};
use uuid::Uuid;

/// The provider selected for an agent role.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Provider {
    Claude,
    Codex,
}

impl Provider {
    fn parse(value: &str) -> Result<Self, String> {
        match value {
            "claude" => Ok(Self::Claude),
            "codex" => Ok(Self::Codex),
            _ => Err(format!(
                "unrecognized provider '{value}': must be 'claude' or 'codex'"
            )),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentRole {
    Worker,
    CiFix,
    Reviewer,
    Feedback,
    Wrapup,
}

impl AgentRole {
    pub fn setting_name(self) -> &'static str {
        match self {
            Self::Worker | Self::Wrapup => "WORKER",
            Self::CiFix => "CI_FIX",
            Self::Reviewer => "REVIEWER",
            Self::Feedback => "FEEDBACK",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentSettings {
    values: std::collections::BTreeMap<String, String>,
}

impl AgentSettings {
    pub fn from_environment(env_file: Option<&Path>) -> Result<Self, String> {
        let mut values: std::collections::BTreeMap<_, _> = env::vars().collect();
        if let Some(path) = env_file {
            let contents = fs::read_to_string(path)
                .map_err(|error| format!("cannot read env file {}: {error}", path.display()))?;
            for line in contents.lines() {
                let line = line.trim();
                if line.is_empty() || line.starts_with('#') {
                    continue;
                }
                let line = line.strip_prefix("export ").unwrap_or(line);
                let Some((key, value)) = line.split_once('=') else {
                    continue;
                };
                let value = value.trim().trim_matches(['\'', '"']);
                values.insert(key.trim().to_string(), value.to_string());
            }
        }
        Ok(Self { values })
    }

    fn get(&self, key: &str) -> Option<&str> {
        self.values
            .get(key)
            .map(String::as_str)
            .filter(|value| !value.is_empty())
    }

    pub fn provider(&self, role: AgentRole) -> Result<Provider, String> {
        let key = format!("ALUCARD_{}_PROVIDER", role.setting_name());
        Provider::parse(
            self.get(&key)
                .or_else(|| self.get("ALUCARD_PROVIDER"))
                .unwrap_or("claude"),
        )
    }

    pub fn setting(&self, role: AgentRole, suffix: &str, default: &str) -> String {
        let role_key = format!("ALUCARD_{}_{}", role.setting_name(), suffix);
        self.get(&role_key)
            .or_else(|| self.get(&format!("ALUCARD_{suffix}")))
            .unwrap_or(default)
            .to_string()
    }

    pub fn turns(&self, role: AgentRole) -> String {
        let default = match role {
            AgentRole::Worker => "180",
            AgentRole::CiFix => "30",
            AgentRole::Reviewer => "45",
            AgentRole::Feedback => "50",
            AgentRole::Wrapup => "12",
        };
        self.setting(role, "MAX_TURNS", default)
    }

    pub fn budget(&self, role: AgentRole) -> String {
        let default = match role {
            AgentRole::Worker => "10",
            AgentRole::CiFix => "2",
            AgentRole::Reviewer => "2",
            AgentRole::Feedback => "2",
            AgentRole::Wrapup => "1",
        };
        self.setting(role, "MAX_BUDGET", default)
    }
}

pub fn compose_role_prompt(
    tool_dir: &Path,
    role_file: &str,
    trailer_file: Option<&str>,
) -> io::Result<String> {
    let mut prompt = fs::read_to_string(tool_dir.join(role_file))?;
    prompt.push_str("\n\n");
    prompt.push_str(&fs::read_to_string(
        tool_dir.join("alucard-engineering-policy.md"),
    )?);
    if let Some(trailer) = trailer_file {
        prompt.push_str("\n\n");
        prompt.push_str(&fs::read_to_string(tool_dir.join(trailer))?);
    }
    Ok(prompt)
}

pub fn build_inputs_hash(tool_dir: &Path) -> io::Result<String> {
    let mut hasher = Sha256::new();
    for name in ["Dockerfile", "entrypoint.sh"] {
        let path = tool_dir.join(name);
        if path.is_file() {
            hasher.update(format!("=== {name}\n").as_bytes());
            hasher.update(fs::read(path)?);
        }
    }
    Ok(format!("{:x}", hasher.finalize()))
}

pub fn build_image(tool_dir: &Path, image: &str) -> Result<(), String> {
    build_image_with_docker(tool_dir, image, Path::new("docker"))
}

fn build_image_with_docker(tool_dir: &Path, image: &str, docker: &Path) -> Result<(), String> {
    let hash = build_inputs_hash(tool_dir).map_err(|error| error.to_string())?;
    let previous_id = docker_image_id(docker, image);
    let status = Command::new(docker)
        .args([
            "build",
            "--label",
            &format!("alucard.build-inputs={hash}"),
            "-t",
            image,
        ])
        .arg(tool_dir)
        .status()
        .map_err(|error| error.to_string())?;
    if !status.success() {
        return Err(format!("docker build failed with {status}"));
    }
    let Some(previous_id) = previous_id else {
        return Ok(());
    };
    let Some(new_id) = docker_image_id(docker, image) else {
        return Ok(());
    };
    if new_id == previous_id || docker_image_tag_count(docker, &previous_id) != Some(0) {
        return Ok(());
    }

    let display_id = previous_id.get(7..19).unwrap_or(&previous_id);
    eprintln!("Removing superseded image {display_id}...");
    let removed = Command::new(docker)
        .args(["rmi", &previous_id])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success());
    if !removed {
        eprintln!("Warning: could not remove superseded image {display_id} (a container may still hold it).");
    }
    Ok(())
}

fn docker_image_id(docker: &Path, image: &str) -> Option<String> {
    let output = Command::new(docker)
        .args(["image", "inspect", "-f", "{{.Id}}", image])
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_string())
        .filter(|id| !id.is_empty())
}

fn docker_image_tag_count(docker: &Path, image: &str) -> Option<usize> {
    let output = Command::new(docker)
        .args(["image", "inspect", "-f", "{{len .RepoTags}}", image])
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().parse().ok())
        .flatten()
}

#[cfg(test)]
mod tests {
    use super::{build_image_with_docker, AgentInvocation, AgentRole, AgentSettings};
    use std::env;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::Path;

    fn run_build(
        previous: &str,
        next: &str,
        tags: usize,
        build_rc: i32,
        rmi_rc: i32,
    ) -> (Result<(), String>, String) {
        let dir =
            std::env::temp_dir().join(format!("alucard-build-image-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&dir).unwrap();
        fs::write(dir.join("Dockerfile"), "FROM scratch\n").unwrap();
        fs::write(dir.join("entrypoint.sh"), "#!/bin/sh\n").unwrap();
        let trace = dir.join("trace");
        let built = dir.join("built");
        let docker = dir.join("docker");
        let script = format!(
            "#!/bin/sh\nprintf '%s\\n' \"$*\" >> {trace}\ncase \"$1 $2\" in\n  'image inspect')\n    case \"$*\" in\n      *'{{{{.Id}}}}'*)\n        if [ -f {built} ]; then id='{next}'; else id='{previous}'; fi\n        [ -n \"$id\" ] || exit 1\n        printf '%s\\n' \"$id\" ;;\n      *'{{{{len .RepoTags}}}}'*) printf '%s\\n' '{tags}' ;;\n    esac ;;\n  'build '*) touch {built}; exit {build_rc} ;;\n  'rmi '*) exit {rmi_rc} ;;\nesac\n",
            trace = trace.display(),
            built = built.display(),
        );
        fs::write(&docker, script).unwrap();
        fs::set_permissions(&docker, fs::Permissions::from_mode(0o755)).unwrap();
        let result = build_image_with_docker(&dir, "alucard:test", &docker);
        let trace = fs::read_to_string(trace).unwrap();
        fs::remove_dir_all(dir).unwrap();
        (result, trace)
    }

    #[test]
    fn build_cleanup_matches_bash_contract() {
        let old = "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        let new = "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

        let (result, trace) = run_build(old, new, 0, 0, 0);
        assert!(result.is_ok());
        assert!(trace.contains(&format!("rmi {old}")));

        let (result, trace) = run_build(old, old, 0, 0, 0);
        assert!(result.is_ok());
        assert!(!trace.contains("rmi"));

        let (result, trace) = run_build(old, new, 2, 0, 0);
        assert!(result.is_ok());
        assert!(!trace.contains("rmi"));

        let (result, trace) = run_build(old, new, 0, 1, 0);
        assert!(result.is_err());
        assert!(!trace.contains("rmi"));

        let (result, trace) = run_build(old, new, 0, 0, 1);
        assert!(result.is_ok());
        assert!(trace.contains(&format!("rmi {old}")));
    }

    #[test]
    fn invoke_cleans_up_when_codex_stdin_closes_early() {
        let dir = env::temp_dir().join(format!("alucard-invoke-cleanup-{}", uuid::Uuid::new_v4()));
        let bin = dir.join("bin");
        fs::create_dir_all(&bin).unwrap();
        let trace = dir.join("trace");
        let timeout = bin.join("timeout");
        let docker = bin.join("docker");
        fs::write(&timeout, "#!/bin/sh\nexec 0<&-\nsleep 1\n").unwrap();
        fs::write(
            &docker,
            format!("#!/bin/sh\nprintf '%s\\n' \"$*\" >> {}\n", trace.display()),
        )
        .unwrap();
        for command in [&timeout, &docker] {
            fs::set_permissions(command, fs::Permissions::from_mode(0o755)).unwrap();
        }
        let env_file = dir.join("alucard.env");
        fs::write(&env_file, "ALUCARD_PROVIDER=codex\n").unwrap();
        let settings = AgentSettings::from_environment(Some(&env_file)).unwrap();
        let path = env::var_os("PATH").unwrap_or_default();
        env::set_var(
            "PATH",
            format!("{}:{}", bin.display(), path.to_string_lossy()),
        );

        let prompt = "x".repeat(8 * 1024 * 1024);
        let invocation = AgentInvocation {
            role: AgentRole::Worker,
            name: "alucard-test-cleanup",
            image: "alucard:test",
            env_file: &env_file,
            worktree: Path::new(&dir),
            prompt: &prompt,
            timeout_minutes: 1,
            network: None,
            playwright_cache: None,
            database_url: None,
            github_repo: None,
        };
        let result = invocation.invoke(&settings, &dir.join("agent.log"));

        env::set_var("PATH", path);
        assert!(matches!(result, Err(error) if error.contains("cannot send prompt to codex")));
        assert_eq!(
            fs::read_to_string(&trace).unwrap(),
            "rm -f alucard-test-cleanup\n"
        );
        fs::remove_dir_all(dir).unwrap();
    }
}

pub struct AgentInvocation<'a> {
    pub role: AgentRole,
    pub name: &'a str,
    pub image: &'a str,
    pub env_file: &'a Path,
    pub worktree: &'a Path,
    pub prompt: &'a str,
    pub timeout_minutes: u64,
    pub network: Option<&'a str>,
    pub playwright_cache: Option<&'a Path>,
    pub database_url: Option<&'a str>,
    pub github_repo: Option<&'a str>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentExit {
    Clean,
    Timeout,
    Transport,
    Billing,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentResult {
    pub status: Option<i32>,
    pub classification: AgentExit,
}

impl AgentInvocation<'_> {
    pub fn docker_args(&self, settings: &AgentSettings) -> Result<Vec<String>, String> {
        let provider = settings.provider(self.role)?;
        let uid = unsafe { libc::geteuid() };
        let gid = unsafe { libc::getegid() };
        let mut args = vec![
            "run".into(),
            "--rm".into(),
            "--name".into(),
            self.name.into(),
            "--network".into(),
            self.network.unwrap_or("bridge").into(),
            "--memory".into(),
            "4g".into(),
            "--cpus".into(),
            "2".into(),
            "--read-only".into(),
            "--tmpfs".into(),
            "/tmp:exec,size=2g".into(),
            "--tmpfs".into(),
            format!("/home/alucard:size=2g,uid={uid},gid={gid}"),
            "--ulimit".into(),
            "nofile=8192:8192".into(),
            "--user".into(),
            format!("{uid}:{gid}"),
            "--security-opt".into(),
            "no-new-privileges".into(),
            "--cap-drop".into(),
            "ALL".into(),
            "--env-file".into(),
            self.env_file.display().to_string(),
            "-e".into(),
            "HOME=/home/alucard".into(),
            "-e".into(),
            "GIT_CONFIG_GLOBAL=/tmp/alucard-gitconfig".into(),
            "-e".into(),
            "GH_CONFIG_DIR=/tmp/gh".into(),
            "-v".into(),
            format!("{}:/work:rw", self.worktree.display()),
        ];
        if provider == Provider::Codex {
            args.insert(2, "-i".into());
        }
        if let Some(cache) = self.playwright_cache {
            args.extend([
                "-v".into(),
                format!("{}:/opt/alucard-browsers:rw", cache.display()),
                "-e".into(),
                "PLAYWRIGHT_BROWSERS_PATH=/opt/alucard-browsers".into(),
                "-e".into(),
                "PLAYWRIGHT_SKIP_BROWSER_GC=1".into(),
            ]);
        }
        if let Some(url) = self.database_url {
            args.extend(["-e".into(), format!("DATABASE_URL={url}")]);
        }
        if let Some(repo) = self.github_repo {
            args.extend(["-e".into(), format!("GH_REPO={repo}")]);
        }
        args.push(self.image.into());
        match provider {
            Provider::Codex => args.extend([
                "codex".into(),
                "exec".into(),
                "--dangerously-bypass-approvals-and-sandbox".into(),
                "--json".into(),
                "--ephemeral".into(),
                "-m".into(),
                settings.setting(self.role, "CODEX_MODEL", "gpt-5.6-terra"),
                "-".into(),
            ]),
            Provider::Claude => {
                args.extend([
                    "claude".into(),
                    "-p".into(),
                    "--verbose".into(),
                    "--output-format".into(),
                    "stream-json".into(),
                    "--include-partial-messages".into(),
                    "--permission-mode".into(),
                    "bypassPermissions".into(),
                    "--max-turns".into(),
                    settings.turns(self.role),
                    "--max-budget-usd".into(),
                    settings.budget(self.role),
                    "--model".into(),
                    settings.setting(self.role, "CLAUDE_MODEL", "sonnet"),
                    "--fallback-model".into(),
                    settings.setting(self.role, "CLAUDE_FALLBACK_MODEL", "haiku"),
                ]);
                let effort = settings.setting(self.role, "EFFORT", "");
                if !effort.is_empty() {
                    args.extend(["--effort".into(), effort]);
                }
                args.extend(["--disallowedTools".into(), "Bash(rm -rf /*),Bash(sudo *),Bash(curl * | sh),Bash(curl * | bash),Bash(wget * | sh),Bash(dd if=*),Bash(mkfs*)".into(), "--".into(), self.prompt.into()]);
            }
        }
        Ok(args)
    }

    /// Runs Docker with direct file output so a descendant cannot block pipe draining after timeout.
    pub fn invoke(&self, settings: &AgentSettings, log_file: &Path) -> Result<AgentResult, String> {
        let provider = settings.provider(self.role)?;
        let docker_args = self.docker_args(settings)?;
        let log = File::create(log_file)
            .map_err(|error| format!("cannot create {}: {error}", log_file.display()))?;
        let mut command = Command::new("timeout");
        command
            .args([
                "--kill-after",
                "30s",
                &format!("{}m", self.timeout_minutes),
                "docker",
            ])
            .args(&docker_args)
            .current_dir(self.worktree)
            .stdout(Stdio::from(
                log.try_clone().map_err(|error| error.to_string())?,
            ))
            .stderr(Stdio::from(log));
        if provider == Provider::Codex {
            command.stdin(Stdio::piped());
        }
        let mut child = command
            .spawn()
            .map_err(|error| format!("cannot start docker: {error}"))?;
        let status = (|| {
            if provider == Provider::Codex {
                child
                    .stdin
                    .take()
                    .expect("codex stdin is piped")
                    .write_all(self.prompt.as_bytes())
                    .map_err(|error| format!("cannot send prompt to codex: {error}"))?;
            }
            child
                .wait()
                .map_err(|error| format!("cannot wait for docker: {error}"))
        })();
        let _ = Command::new("docker")
            .args(["rm", "-f", self.name])
            .status();
        let status = status?;
        let log = fs::read_to_string(log_file)
            .unwrap_or_default()
            .to_ascii_lowercase();
        let classification = match status.code() {
            Some(0) => AgentExit::Clean,
            Some(124 | 137) => AgentExit::Timeout,
            _ if log.contains("billing") || log.contains("credit") || log.contains("quota") => {
                AgentExit::Billing
            }
            _ if [
                "connection",
                "network error",
                "overloaded",
                "rate limit",
                "gateway",
                "econnreset",
                "api error",
            ]
            .iter()
            .any(|marker| log.contains(marker)) =>
            {
                AgentExit::Transport
            }
            _ => AgentExit::Failed,
        };
        Ok(AgentResult {
            status: status.code(),
            classification,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LocalTask {
    pub id: String,
    pub title: String,
    pub body: String,
    #[serde(skip)]
    pub state: TaskState,
    #[serde(skip)]
    pub line: usize,
    #[serde(skip)]
    pub pr_number: Option<i64>,
    #[serde(skip)]
    pub previous_pr_number: Option<i64>,
    #[serde(skip)]
    pub blockers: Vec<String>,
    #[serde(skip)]
    pub issue_blockers: Vec<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, serde::Deserialize)]
pub struct GithubTask {
    pub number: i64,
    pub title: String,
    pub body: String,
    pub labels: Vec<GithubLabel>,
    #[serde(default, skip_serializing)]
    pub state: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, serde::Deserialize)]
pub struct GithubLabel {
    pub name: String,
}

pub fn github_ticket_blockers(body: &str) -> Vec<i64> {
    let mut blockers = Vec::new();
    for line in body.lines() {
        let mut rest = line;
        while let Some(index) = rest.find("Blocked by #") {
            rest = &rest[index + "Blocked by #".len()..];
            let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
            if let Ok(number) = rest[..digits].parse() {
                blockers.push(number);
            }
            rest = &rest[digits..];
        }
    }
    let mut in_section = false;
    for line in body.lines() {
        if let Some(heading) = line.strip_prefix("##") {
            in_section = heading
                .trim_start_matches(char::is_whitespace)
                .strip_prefix("Blocked by")
                .is_some_and(|rest| {
                    rest.is_empty() || rest.chars().next().is_some_and(char::is_whitespace)
                });
            continue;
        }
        if in_section {
            let mut rest = line;
            while let Some(index) = rest.find('#') {
                rest = &rest[index + 1..];
                let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
                if let Ok(number) = rest[..digits].parse() {
                    blockers.push(number);
                }
                rest = &rest[digits..];
            }
        }
    }
    blockers.sort_unstable();
    blockers.dedup();
    blockers
}

pub fn github_queue(
    issues: Vec<GithubTask>,
    open_issue_numbers: &[i64],
    open_prs: &[GithubPullRequest],
) -> Vec<GithubTask> {
    let mut queue: Vec<_> = issues
        .into_iter()
        .filter(|issue| {
            let labels: Vec<_> = issue
                .labels
                .iter()
                .map(|label| label.name.as_str())
                .collect();
            labels.contains(&"ready-for-agent")
                && !labels
                    .iter()
                    .any(|label| matches!(*label, "ready-for-human" | "in-progress" | "wip"))
                && github_ticket_blockers(&issue.body)
                    .iter()
                    .all(|blocker| !open_issue_numbers.contains(blocker))
                && !open_prs.iter().any(|pr| pr.covers(issue.number))
        })
        .collect();
    queue.sort_by_key(|issue| {
        (
            !issue
                .labels
                .iter()
                .any(|label| label.name == "priority:high"),
            issue.number,
        )
    });
    queue
}

pub fn resolve_pinned_issue(
    issue: GithubTask,
    is_pull_request: bool,
    open_issues: &[i64],
    open_prs: &[GithubPullRequest],
) -> Result<GithubTask, String> {
    if issue.state.as_deref() != Some("OPEN") {
        return Err(format!("issue #{} is closed", issue.number));
    }
    if is_pull_request {
        return Err(format!(
            "issue #{} is a pull request; use alucard continue {}",
            issue.number, issue.number
        ));
    }
    let open_blockers: Vec<_> = github_ticket_blockers(&issue.body)
        .into_iter()
        .filter(|number| open_issues.contains(number))
        .collect();
    if !open_blockers.is_empty() {
        return Err(format!(
            "issue #{} is blocked by {}",
            issue.number,
            open_blockers
                .iter()
                .map(|number| format!("#{number}"))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    if let Some(pr) = open_prs.iter().find(|pr| pr.covers(issue.number)) {
        return Err(format!(
            "issue #{} is occupied by PR #{}; use alucard continue {}",
            issue.number, pr.number, pr.number
        ));
    }
    Ok(issue)
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
pub struct GithubPullRequest {
    pub number: i64,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub body: String,
    #[serde(default, rename = "closingIssuesReferences")]
    pub closing_issues_references: ClosingIssuesReferences,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Deserialize)]
pub struct ClosingIssuesReferences {
    #[serde(default)]
    pub nodes: Vec<IssueReference>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
pub struct IssueReference {
    pub number: i64,
}

impl GithubPullRequest {
    pub fn covers(&self, issue_number: i64) -> bool {
        self.closing_issues_references
            .nodes
            .iter()
            .any(|issue| issue.number == issue_number)
            || [self.title.as_str(), self.body.as_str()]
                .iter()
                .any(|text| references_issue(text, issue_number))
    }
}

fn references_issue(text: &str, issue_number: i64) -> bool {
    let reference = format!("#{issue_number}");
    text.match_indices(&reference).any(|(index, _)| {
        !text[index + reference.len()..]
            .chars()
            .next()
            .is_some_and(|character| character.is_ascii_alphanumeric() || character == '_')
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskState {
    Pending,
    InFlight,
    Done,
    Human,
}

impl TaskState {
    fn from_marker(marker: char) -> Self {
        match marker {
            ' ' => Self::Pending,
            '>' => Self::InFlight,
            'x' => Self::Done,
            'h' => Self::Human,
            _ => unreachable!("validated task state"),
        }
    }

    fn ledger_status(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::InFlight => "in_flight",
            Self::Done => "done",
            Self::Human => "human",
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct ParsedTasks {
    pub parent_context: String,
    pub tasks: Vec<LocalTask>,
}

pub fn parse_tasks(contents: &str) -> Result<ParsedTasks, Vec<String>> {
    let contents = contents.replace('\r', "");
    let lines: Vec<_> = contents.lines().collect();
    let mut parent = Vec::new();
    let mut tasks = Vec::new();
    let mut current: Option<LocalTask> = None;
    let mut errors = Vec::new();

    for (index, line) in lines.iter().enumerate() {
        if line.strip_prefix("##").is_some_and(|rest| {
            rest.trim_start_matches(|character: char| character.is_ascii_whitespace())
                .starts_with('[')
        }) {
            let heading = line.strip_prefix("## ").unwrap_or("");
            let Some((marker, rest)) = heading
                .strip_prefix('[')
                .and_then(|value| {
                    value
                        .chars()
                        .next()
                        .map(|marker| (marker, &value[marker.len_utf8()..]))
                })
                .and_then(|(marker, value)| value.strip_prefix("] ").map(|rest| (marker, rest)))
            else {
                errors.push(format!(
                    "line {}: malformed task heading: {line}",
                    index + 1
                ));
                continue;
            };
            let Some((id, title)) = rest.split_once(": ") else {
                errors.push(format!(
                    "line {}: malformed task heading: {line}",
                    index + 1
                ));
                continue;
            };
            if !matches!(marker, ' ' | '>' | 'x' | 'h')
                || id.is_empty()
                || title.is_empty()
                || !id.chars().all(|character| {
                    character.is_ascii_alphanumeric() || matches!(character, '_' | '-')
                })
            {
                errors.push(format!(
                    "line {}: malformed task heading: {line}",
                    index + 1
                ));
                continue;
            }
            if let Some(task) = current.take() {
                tasks.push(task);
            }
            let (title, pr_number) = split_pr_suffix(title, "(PR #");
            let (title, previous_pr_number) = split_pr_suffix(title, "(previous attempt: PR #");
            current = Some(LocalTask {
                id: id.to_string(),
                title: title.to_string(),
                body: String::new(),
                state: TaskState::from_marker(marker),
                line: index + 1,
                pr_number,
                previous_pr_number,
                blockers: Vec::new(),
                issue_blockers: Vec::new(),
            });
        } else if let Some(task) = current.as_mut() {
            if !task.body.is_empty() {
                task.body.push('\n');
            }
            task.body.push_str(line);
        } else {
            parent.push(*line);
        }
    }
    if let Some(task) = current {
        tasks.push(task);
    }
    for task in &mut tasks {
        task.body = task.body.trim_matches('\n').to_string();
        for line in task
            .body
            .lines()
            .filter_map(|line| line.strip_prefix("Blocked by:"))
        {
            for token in line.split(|character: char| character == ',' || character.is_whitespace())
            {
                if let Some(number) = token
                    .strip_prefix('#')
                    .and_then(|number| number.parse().ok())
                {
                    task.issue_blockers.push(number);
                } else if !token.is_empty() && token != "none" {
                    task.blockers.push(token.to_string());
                }
            }
        }
        let mut body = task.body.as_str();
        while let Some(index) = body.find("Blocked by #") {
            let number = &body[index + "Blocked by #".len()..];
            let digits = number.bytes().take_while(u8::is_ascii_digit).count();
            if let Ok(number) = number[..digits].parse() {
                task.issue_blockers.push(number);
            }
            body = if digits == 0 {
                &number[number.chars().next().map_or(0, char::len_utf8)..]
            } else {
                &number[digits..]
            };
        }
    }
    if errors.is_empty() {
        Ok(ParsedTasks {
            parent_context: parent.join("\n").trim_end_matches('\n').to_string(),
            tasks,
        })
    } else {
        Err(errors)
    }
}

fn split_pr_suffix<'a>(title: &'a str, prefix: &str) -> (&'a str, Option<i64>) {
    let Some(start) = title.rfind(prefix) else {
        return (title, None);
    };
    let number = &title[start + prefix.len()..];
    let Some(number) = number.strip_suffix(')') else {
        return (title, None);
    };
    if !number.bytes().all(|character| character.is_ascii_digit()) {
        return (title, None);
    }
    let Ok(number) = number.parse() else {
        return (title, None);
    };
    (title[..start].trim_end(), Some(number))
}

pub fn local_queue(tasks: &[LocalTask]) -> Vec<LocalTask> {
    local_queue_with_open_issues(tasks, None)
}

pub fn local_queue_with_open_issues(
    tasks: &[LocalTask],
    open_issues: Option<&[i64]>,
) -> Vec<LocalTask> {
    let states: std::collections::HashMap<_, _> = tasks
        .iter()
        .map(|task| (task.id.as_str(), task.state))
        .collect();
    tasks
        .iter()
        .filter(|task| {
            task.state == TaskState::Pending
                && task.issue_blockers.iter().all(|blocker| {
                    open_issues
                        .map(|open| !open.contains(blocker))
                        .unwrap_or(false)
                })
                && task
                    .blockers
                    .iter()
                    .all(|blocker| states.get(blocker.as_str()) == Some(&TaskState::Done))
        })
        .cloned()
        .collect()
}

pub fn import_local_tasks(
    connection: &mut Connection,
    repository: &str,
    source_path: &Path,
    tasks: &[LocalTask],
) -> Result<(), DatabaseError> {
    let transaction = connection.transaction()?;
    transaction.execute(
        "INSERT OR IGNORE INTO repositories (id, owner, name, cache_path, created_at) VALUES (?1, ?1, ?1, ?1, strftime('%Y-%m-%dT%H:%M:%fZ','now'))",
        [repository],
    )?;
    let source_path = source_path.to_string_lossy();
    for task in tasks {
        let blockers = serde_json::to_string(&task.blockers).expect("string blockers serialize");
        transaction.execute(
            "INSERT INTO tasks (id, repository_id, source, source_path, external_id, title, status, blockers_json, pr_number, previous_pr_number, updated_at)
             VALUES (lower(hex(randomblob(16))), ?1, 'local', ?2, ?3, ?4, ?5, ?6, ?7, ?8, strftime('%Y-%m-%dT%H:%M:%fZ','now'))
             ON CONFLICT(repository_id, source, source_path, external_id) DO UPDATE SET
               title = excluded.title,
               status = CASE WHEN tasks.status IN ('claimed', 'in_flight') THEN tasks.status ELSE excluded.status END,
               blockers_json = excluded.blockers_json,
               pr_number = excluded.pr_number, previous_pr_number = excluded.previous_pr_number,
               updated_at = excluded.updated_at
             WHERE title IS NOT excluded.title OR status IS NOT excluded.status OR blockers_json IS NOT excluded.blockers_json
               OR pr_number IS NOT excluded.pr_number OR previous_pr_number IS NOT excluded.previous_pr_number",
            rusqlite::params![repository, source_path, task.id, task.title, task.state.ledger_status(), blockers, task.pr_number, task.previous_pr_number],
        )?;
    }
    transaction.commit()?;
    Ok(())
}

pub fn import_github_tasks(
    connection: &mut Connection,
    repository: &str,
    tasks: &[GithubTask],
) -> Result<(), DatabaseError> {
    let transaction = connection.transaction()?;
    transaction.execute(
        "INSERT OR IGNORE INTO repositories (id, owner, name, cache_path, created_at) VALUES (?1, ?1, ?1, ?1, strftime('%Y-%m-%dT%H:%M:%fZ','now'))",
        [repository],
    )?;
    for task in tasks {
        let blockers = serde_json::to_string(&github_ticket_blockers(&task.body))
            .expect("issue blockers serialize");
        transaction.execute(
            "INSERT INTO tasks (id, repository_id, source, source_path, external_id, title, status, blockers_json, updated_at)
             VALUES (lower(hex(randomblob(16))), ?1, 'github', '', ?2, ?3, 'pending', ?4, strftime('%Y-%m-%dT%H:%M:%fZ','now'))
             ON CONFLICT(repository_id, source, source_path, external_id) DO UPDATE SET
               title = excluded.title,
               status = CASE WHEN tasks.status IN ('claimed', 'in_flight') THEN tasks.status ELSE excluded.status END,
               blockers_json = excluded.blockers_json,
               updated_at = excluded.updated_at
             WHERE title IS NOT excluded.title OR status IS NOT excluded.status OR blockers_json IS NOT excluded.blockers_json",
            rusqlite::params![repository, task.number.to_string(), task.title, blockers],
        )?;
    }
    transaction.commit()?;
    Ok(())
}

const MIGRATIONS: &[(i64, &str)] = &[
    (1, include_str!("../migrations/0001_init.sql")),
    (2, include_str!("../migrations/0002_run_owners.sql")),
];

pub struct DatabaseStatus {
    pub schema_version: i64,
    pub pending_migrations: usize,
}

#[derive(Debug, PartialEq, Eq)]
pub struct Run {
    pub id: String,
    pub repository_id: String,
    pub command: String,
    pub status: String,
    pub base_sha: Option<String>,
    pub image: Option<String>,
    pub logs_path: Option<String>,
}

pub struct PreparedRun {
    pub run: Run,
    pub directory: PathBuf,
    pub clone: PathBuf,
    pub branch: String,
}

pub struct RunOptions<'a> {
    pub repository: &'a Path,
    pub logs_root: &'a Path,
    pub cache_root: &'a Path,
    pub base: &'a str,
    pub image: Option<&'a str>,
    pub task: Option<&'a str>,
    pub issue: Option<i64>,
}

#[derive(Debug, PartialEq, Eq)]
pub struct Claim {
    pub id: String,
    pub repository_id: String,
    pub resource_kind: ClaimResource,
    pub resource_key: String,
    pub run_id: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClaimResource {
    Task,
    PullRequest,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TaskStatus {
    Pending,
    Claimed,
    InFlight,
    Done,
    Human,
}

impl TaskStatus {
    fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Claimed => "claimed",
            Self::InFlight => "in_flight",
            Self::Done => "done",
            Self::Human => "human",
        }
    }
}

impl ClaimResource {
    fn as_str(self) -> &'static str {
        match self {
            Self::Task => "task",
            Self::PullRequest => "pr",
        }
    }
}

#[derive(Debug)]
pub enum ClaimError {
    Database(DatabaseError),
    ClaimedBy { run_id: String },
    RunNotInRepository,
    TaskNotEligible,
    LeaseNotActive,
}

impl std::fmt::Display for ClaimError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Database(error) => error.fmt(formatter),
            Self::ClaimedBy { run_id } => write!(formatter, "claimed by run {run_id}"),
            Self::RunNotInRepository => write!(formatter, "run does not belong to the repository"),
            Self::TaskNotEligible => write!(formatter, "task is not eligible for a claim"),
            Self::LeaseNotActive => write!(formatter, "lease is not active"),
        }
    }
}

impl std::error::Error for ClaimError {}

impl PartialEq for ClaimError {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::ClaimedBy { run_id: left }, Self::ClaimedBy { run_id: right }) => left == right,
            (Self::RunNotInRepository, Self::RunNotInRepository)
            | (Self::TaskNotEligible, Self::TaskNotEligible)
            | (Self::LeaseNotActive, Self::LeaseNotActive) => true,
            _ => false,
        }
    }
}

impl Eq for ClaimError {}

impl From<DatabaseError> for ClaimError {
    fn from(error: DatabaseError) -> Self {
        Self::Database(error)
    }
}

impl From<rusqlite::Error> for ClaimError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Database(error.into())
    }
}

pub fn acquire_task(path: &Path, task_id: &str, run_id: &str) -> Result<Claim, ClaimError> {
    let mut connection = open(path)?;
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let (repository_id, status): (String, String) = transaction
        .query_row(
            "SELECT repository_id, status FROM tasks WHERE id = ?1",
            [task_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(|error| match error {
            rusqlite::Error::QueryReturnedNoRows => ClaimError::TaskNotEligible,
            other => ClaimError::Database(other.into()),
        })?;
    if let Some(run_id) = active_claim(&transaction, &repository_id, ClaimResource::Task, task_id)?
    {
        return Err(ClaimError::ClaimedBy { run_id });
    }
    if status != "pending" {
        return Err(ClaimError::TaskNotEligible);
    }
    let claim = acquire(
        &transaction,
        &repository_id,
        ClaimResource::Task,
        task_id,
        run_id,
    )?;
    let changed = transaction.execute(
        "UPDATE tasks SET status = 'claimed', updated_at = datetime('now') WHERE id = ?1",
        (task_id,),
    )?;
    if changed != 1 {
        return Err(ClaimError::TaskNotEligible);
    }
    transaction.commit()?;
    Ok(claim)
}

pub fn acquire_pr(
    path: &Path,
    repository_id: &str,
    pr_number: i64,
    run_id: &str,
) -> Result<Claim, ClaimError> {
    let mut connection = open(path)?;
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let claim = acquire(
        &transaction,
        repository_id,
        ClaimResource::PullRequest,
        &pr_number.to_string(),
        run_id,
    )?;
    transaction.commit()?;
    Ok(claim)
}

pub fn heartbeat(path: &Path, lease_id: &str, run_id: &str) -> Result<(), ClaimError> {
    update_active_claim(path, lease_id, run_id, "heartbeat_at = datetime('now')")
}

pub fn release(path: &Path, lease_id: &str, run_id: &str) -> Result<(), ClaimError> {
    update_active_claim(
        path,
        lease_id,
        run_id,
        "state = 'released', heartbeat_at = datetime('now')",
    )
}

pub fn transition_task(
    path: &Path,
    task_id: &str,
    lease_id: &str,
    run_id: &str,
    status: TaskStatus,
) -> Result<(), ClaimError> {
    let mut connection = open(path)?;
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let changed = transaction.execute(
        "UPDATE tasks SET status = ?1, updated_at = datetime('now')
         WHERE id = ?2 AND EXISTS (
             SELECT 1 FROM claims
             WHERE id = ?3 AND run_id = ?4 AND resource_kind = 'task'
               AND resource_key = ?2 AND state = 'active'
         )",
        (status.as_str(), task_id, lease_id, run_id),
    )?;
    if changed != 1 {
        return Err(ClaimError::LeaseNotActive);
    }
    transaction.commit()?;
    Ok(())
}

fn acquire(
    transaction: &rusqlite::Transaction<'_>,
    repository_id: &str,
    resource_kind: ClaimResource,
    resource_key: &str,
    run_id: &str,
) -> Result<Claim, ClaimError> {
    let run_belongs_to_repository: bool = transaction.query_row(
        "SELECT EXISTS(SELECT 1 FROM runs WHERE id = ?1 AND repository_id = ?2)",
        (run_id, repository_id),
        |row| row.get(0),
    )?;
    if !run_belongs_to_repository {
        return Err(ClaimError::RunNotInRepository);
    }
    if let Some(run_id) = active_claim(transaction, repository_id, resource_kind, resource_key)? {
        return Err(ClaimError::ClaimedBy { run_id });
    }
    let claim = Claim {
        id: Uuid::new_v4().to_string(),
        repository_id: repository_id.to_string(),
        resource_kind,
        resource_key: resource_key.to_string(),
        run_id: run_id.to_string(),
    };
    transaction.execute(
        "INSERT INTO claims (id, repository_id, resource_kind, resource_key, run_id, state, acquired_at, heartbeat_at)
         VALUES (?1, ?2, ?3, ?4, ?5, 'active', datetime('now'), datetime('now'))",
        (&claim.id, &claim.repository_id, resource_kind.as_str(), &claim.resource_key, &claim.run_id),
    )?;
    Ok(claim)
}

fn active_claim(
    transaction: &rusqlite::Transaction<'_>,
    repository_id: &str,
    resource_kind: ClaimResource,
    resource_key: &str,
) -> Result<Option<String>, ClaimError> {
    transaction
        .query_row(
            "SELECT run_id FROM claims
             WHERE repository_id = ?1 AND resource_kind = ?2 AND resource_key = ?3 AND state = 'active'",
            (repository_id, resource_kind.as_str(), resource_key),
            |row| row.get(0),
        )
        .optional()
        .map_err(Into::into)
}

fn update_active_claim(
    path: &Path,
    lease_id: &str,
    run_id: &str,
    assignment: &str,
) -> Result<(), ClaimError> {
    let connection = open(path)?;
    let changed = connection.execute(
        &format!(
            "UPDATE claims SET {assignment} WHERE id = ?1 AND run_id = ?2 AND state = 'active'"
        ),
        (lease_id, run_id),
    )?;
    if changed == 1 {
        Ok(())
    } else {
        Err(ClaimError::LeaseNotActive)
    }
}

/// Executes an external command without involving a shell.
pub fn run_command(
    program: &str,
    arguments: &[&str],
    cwd: &Path,
    timeout: Option<Duration>,
) -> io::Result<Output> {
    let mut command = Command::new(program);
    command
        .args(arguments)
        .current_dir(cwd)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn()?;
    let stdout = drain(child.stdout.take().expect("stdout is piped"));
    let stderr = drain(child.stderr.take().expect("stderr is piped"));
    let status = match timeout {
        None => child.wait()?,
        Some(timeout) => {
            let deadline = Instant::now() + timeout;
            loop {
                if let Some(status) = child.try_wait()? {
                    break status;
                }
                if Instant::now() >= deadline {
                    if let Err(error) = child.kill() {
                        if error.kind() != io::ErrorKind::InvalidInput {
                            return Err(error);
                        }
                    }
                    break child.wait()?;
                }
                thread::sleep(Duration::from_millis(10));
            }
        }
    };
    Ok(Output {
        status,
        stdout: join_output(stdout)?,
        stderr: join_output(stderr)?,
    })
}

fn drain<R: Read + Send + 'static>(mut reader: R) -> thread::JoinHandle<io::Result<Vec<u8>>> {
    thread::spawn(move || {
        let mut output = Vec::new();
        reader.read_to_end(&mut output)?;
        Ok(output)
    })
}

fn join_output(output: thread::JoinHandle<io::Result<Vec<u8>>>) -> io::Result<Vec<u8>> {
    output
        .join()
        .map_err(|_| io::Error::other("command output reader panicked"))?
}

fn run_checked(program: &str, arguments: &[&str], cwd: &Path) -> Result<Output, RunError> {
    let output = run_command(program, arguments, cwd, None)?;
    if output.status.success() {
        Ok(output)
    } else {
        Err(RunError::Command {
            program: program.to_string(),
            stderr: String::from_utf8_lossy(&output.stderr).trim().to_string(),
        })
    }
}

pub fn prepare_run(path: &Path, options: RunOptions<'_>) -> Result<PreparedRun, RunError> {
    migrate(path)?;
    interrupt_running_runs(path)?;

    let repository = options.repository.canonicalize()?;
    let repository_name = repository
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "repository has no name"))?;
    let repository_id = repository.to_string_lossy().to_string();
    let cache_key = repository_cache_key(&repository);
    let cache = options.cache_root.join(&cache_key);
    fs::create_dir_all(options.cache_root)?;
    let lock = File::create(options.cache_root.join(format!("{cache_key}.lock")))?;
    lock.lock_exclusive()?;
    if cache.exists() {
        run_checked("git", &["fetch", "--quiet", "origin"], &cache)?;
    } else {
        run_checked(
            "git",
            &[
                "clone",
                "--quiet",
                repository.to_str().unwrap_or_default(),
                cache.to_str().unwrap_or_default(),
            ],
            options.cache_root,
        )?;
    }
    let remote_base = format!("origin/{}", options.base);
    let base_sha =
        String::from_utf8_lossy(&run_checked("git", &["rev-parse", &remote_base], &cache)?.stdout)
            .trim()
            .to_string();

    let run_id = Uuid::new_v4().to_string();
    let directory = options.logs_root.join(&run_id);
    fs::create_dir_all(&directory)?;
    let clone = directory.join("repository");
    run_checked(
        "git",
        &[
            "clone",
            "--quiet",
            cache.to_str().unwrap_or_default(),
            clone.to_str().unwrap_or_default(),
        ],
        &directory,
    )?;
    FileExt::unlock(&lock)?;
    if !clone.join(".git").is_dir() {
        return Err(RunError::InvalidClone);
    }
    let suffix = options
        .issue
        .map(|issue| format!("issue-{issue}"))
        .or_else(|| options.task.map(|task| format!("task-{task}")));
    let branch = format!(
        "alucard/{run_id}/iter-1{}",
        suffix.map(|value| format!("-{value}")).unwrap_or_default()
    );
    run_checked(
        "git",
        &["switch", "--quiet", "-c", &branch, &base_sha],
        &clone,
    )?;

    let mut connection = open(path)?;
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    transaction.execute(
        "INSERT OR IGNORE INTO repositories (id, owner, name, cache_path, created_at)
         VALUES (?1, '', ?2, ?3, datetime('now'))",
        params![repository_id, repository_name, cache.to_string_lossy()],
    )?;
    transaction.execute(
        "INSERT INTO runs (id, repository_id, command, status, started_at, heartbeat_at, base_sha, image, logs_path, pid, hostname)
         VALUES (?1, ?2, 'run', 'running', datetime('now'), datetime('now'), ?3, ?4, ?5, ?6, ?7)",
        params![
            run_id,
            repository_id,
            base_sha,
            options.image,
            directory.to_string_lossy(),
            std::process::id(),
            hostname()?,
        ],
    )?;
    transaction.commit()?;
    Ok(PreparedRun {
        run: Run {
            id: run_id,
            repository_id,
            command: "run".to_string(),
            status: "running".to_string(),
            base_sha: Some(base_sha),
            image: options.image.map(str::to_string),
            logs_path: Some(directory.to_string_lossy().to_string()),
        },
        directory,
        clone,
        branch,
    })
}

fn repository_cache_key(repository: &Path) -> String {
    format!(
        "{:x}",
        Sha256::digest(repository.as_os_str().as_encoded_bytes())
    )
}

pub fn interrupt_running_runs(path: &Path) -> Result<usize, DatabaseError> {
    let mut connection = open(path)?;
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let hostname = hostname()?;
    let mut statement = transaction.prepare(
        "SELECT id, pid FROM runs WHERE status = 'running' AND hostname = ?1 AND pid IS NOT NULL",
    )?;
    let abandoned = statement
        .query_map([hostname], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
        })?
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .filter(|(_, pid)| !process_is_alive(*pid))
        .map(|(id, _)| id)
        .collect::<Vec<_>>();
    drop(statement);
    let mut interrupted = 0;
    for run_id in abandoned {
        interrupted += transaction.execute(
            "UPDATE runs SET status = 'interrupted', ended_at = datetime('now') WHERE id = ?1 AND status = 'running'",
            [run_id],
        )?;
    }
    transaction.commit()?;
    Ok(interrupted)
}

fn hostname() -> io::Result<String> {
    let mut value = [0; 256];
    if unsafe { libc::gethostname(value.as_mut_ptr().cast(), value.len()) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(unsafe { CStr::from_ptr(value.as_ptr().cast()) }
        .to_string_lossy()
        .into_owned())
}

fn process_is_alive(pid: i64) -> bool {
    let Ok(pid) = libc::pid_t::try_from(pid) else {
        return false;
    };
    if pid <= 0 {
        return false;
    }
    unsafe {
        libc::kill(pid, 0) == 0 || io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
    }
}

pub fn recent_runs(path: &Path) -> Result<Vec<Run>, DatabaseError> {
    let connection = open(path)?;
    let mut statement = connection.prepare(
        "SELECT id, repository_id, command, status, base_sha, image, logs_path
         FROM runs ORDER BY started_at DESC, rowid DESC LIMIT 20",
    )?;
    let runs = statement
        .query_map([], run_from_row)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(DatabaseError::from)?;
    Ok(runs)
}

pub fn find_run(path: &Path, run_id: &str) -> Result<Option<Run>, DatabaseError> {
    let connection = open(path)?;
    connection
        .query_row(
            "SELECT id, repository_id, command, status, base_sha, image, logs_path FROM runs WHERE id = ?1",
            [run_id],
            run_from_row,
        )
        .optional()
        .map_err(Into::into)
}

pub fn run_claims(
    path: &Path,
    run_id: &str,
) -> Result<Vec<(String, String, String)>, DatabaseError> {
    let connection = open(path)?;
    let mut statement = connection.prepare(
        "SELECT resource_kind, resource_key, state FROM claims WHERE run_id = ?1 ORDER BY acquired_at",
    )?;
    let claims = statement
        .query_map([run_id], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(claims)
}

pub fn run_pull_requests(path: &Path, run_id: &str) -> Result<Vec<(i64, String)>, DatabaseError> {
    let connection = open(path)?;
    let mut statement = connection.prepare(
        "SELECT DISTINCT pull_requests.number, pull_requests.branch
         FROM pull_requests JOIN claims ON claims.repository_id = pull_requests.repository_id
             AND ((claims.resource_kind = 'task' AND claims.resource_key = pull_requests.task_id)
                 OR (claims.resource_kind = 'pr' AND claims.resource_key = CAST(pull_requests.number AS TEXT)))
         WHERE claims.run_id = ?1 ORDER BY pull_requests.number",
    )?;
    let pull_requests = statement
        .query_map([run_id], |row| Ok((row.get(0)?, row.get(1)?)))?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(pull_requests)
}

fn run_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Run> {
    Ok(Run {
        id: row.get(0)?,
        repository_id: row.get(1)?,
        command: row.get(2)?,
        status: row.get(3)?,
        base_sha: row.get(4)?,
        image: row.get(5)?,
        logs_path: row.get(6)?,
    })
}

#[derive(Debug)]
pub enum RunError {
    Database(DatabaseError),
    Io(io::Error),
    Command { program: String, stderr: String },
    InvalidClone,
}

impl From<DatabaseError> for RunError {
    fn from(error: DatabaseError) -> Self {
        Self::Database(error)
    }
}
impl From<io::Error> for RunError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}
impl From<rusqlite::Error> for RunError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Database(error.into())
    }
}
impl std::fmt::Display for RunError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Database(error) => error.fmt(formatter),
            Self::Io(error) => error.fmt(formatter),
            Self::Command { program, stderr } => write!(formatter, "{program} failed: {stderr}"),
            Self::InvalidClone => write!(
                formatter,
                "git clone did not create a self-contained repository"
            ),
        }
    }
}
impl std::error::Error for RunError {}

pub fn database_path(override_path: Option<PathBuf>) -> io::Result<PathBuf> {
    override_path
        .or_else(|| env::var_os("ALUCARD_DB_PATH").map(PathBuf::from))
        .or_else(|| {
            env::var_os("XDG_STATE_HOME")
                .map(PathBuf::from)
                .map(|path| path.join("alucard/alucard.db"))
        })
        .or_else(|| {
            env::var_os("HOME")
                .map(PathBuf::from)
                .map(|path| path.join(".local/state/alucard/alucard.db"))
        })
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                "database path is not set; use --db-path or ALUCARD_DB_PATH",
            )
        })
}

pub fn migrate(path: &Path) -> Result<DatabaseStatus, DatabaseError> {
    let mut connection = open(path)?;
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    transaction.execute_batch(
        "CREATE TABLE IF NOT EXISTS schema_migrations (
            version INTEGER PRIMARY KEY
        );",
    )?;

    for &(version, sql) in MIGRATIONS {
        if !is_applied(&transaction, version)? {
            transaction.execute_batch(sql)?;
            transaction.execute(
                "INSERT INTO schema_migrations (version) VALUES (?1)",
                [version],
            )?;
        }
    }
    transaction.commit()?;
    status(path)
}

pub fn status(path: &Path) -> Result<DatabaseStatus, DatabaseError> {
    let connection = open(path)?;
    let applied = applied_versions(&connection)?;
    let schema_version = applied.iter().copied().max().unwrap_or(0);
    let pending_migrations = MIGRATIONS
        .iter()
        .filter(|(version, _)| !applied.contains(version))
        .count();
    Ok(DatabaseStatus {
        schema_version,
        pending_migrations,
    })
}

pub fn open(path: &Path) -> Result<Connection, DatabaseError> {
    prepare_directory(path)?;
    let connection = Connection::open(path)?;
    restrict_file_permissions(path)?;
    connection.execute_batch("PRAGMA journal_mode = WAL; PRAGMA foreign_keys = ON;")?;
    connection.busy_timeout(Duration::from_secs(5))?;
    Ok(connection)
}

fn is_applied(connection: &Connection, version: i64) -> Result<bool, rusqlite::Error> {
    connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM schema_migrations WHERE version = ?1)",
        [version],
        |row| row.get(0),
    )
}

fn applied_versions(connection: &Connection) -> Result<Vec<i64>, rusqlite::Error> {
    let table_exists: bool = connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'schema_migrations')",
        [],
        |row| row.get(0),
    )?;
    if !table_exists {
        return Ok(Vec::new());
    }
    let mut statement = connection.prepare("SELECT version FROM schema_migrations")?;
    let versions = statement
        .query_map([], |row| row.get(0))?
        .collect::<Result<Vec<i64>, _>>()?;
    Ok(versions)
}

fn prepare_directory(path: &Path) -> io::Result<()> {
    let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    else {
        return Ok(());
    };
    if let Some(grandparent) = parent
        .parent()
        .filter(|grandparent| !grandparent.as_os_str().is_empty())
    {
        fs::create_dir_all(grandparent)?;
    }
    match fs::create_dir(parent) {
        Ok(()) => restrict_directory_permissions(parent)?,
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error),
    }
    Ok(())
}

fn restrict_directory_permissions(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
}

fn restrict_file_permissions(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))
}

#[derive(Debug)]
pub enum DatabaseError {
    Io(io::Error),
    Sql(rusqlite::Error),
}

impl From<io::Error> for DatabaseError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<rusqlite::Error> for DatabaseError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Sql(error)
    }
}

impl std::fmt::Display for DatabaseError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(error) => error.fmt(formatter),
            Self::Sql(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for DatabaseError {}
