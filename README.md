# Alucard — autonomous overnight worker loop

## What it is

A containerized agent loop that picks GitHub issues off a queue, completes them one at a time in isolated git worktrees, opens PRs, and runs unattended overnight. Workflow:

1. **Human (Aldo) writes a PRD or plan** during the day. This is the 80% of the work.
2. **`/to-issues` skill** breaks the plan into vertical-slice GitHub issues, labeled `afk` (autonomous-doable) or `hitl` (needs human).
3. **Alucard runs overnight** — pulls the AFK queue, picks one, implements, tests, commits, opens a PR, repeats until queue empty or iteration cap hit.
4. **Aldo reviews PRs in the morning** and merges what's good.

Alucard never pushes to main. Each iteration produces an independent PR.

## Architecture

- **CLI / host orchestrator** (`alucard`) — bash CLI that loops, manages git worktrees on the host, queries the GitHub issue queue, and shells out to `docker run` per iteration.
- **Compatibility shim** (`alucard.sh`) — forwards old invocations to the CLI.
- **Container** (Dockerfile + entrypoint) — disposable per iteration. Runs `claude` CLI in headless mode with broad permissions but bounded by kernel-level isolation.
- **Worker prompt** (`alucard-prompt.md`) — the system instructions the agent gets each iteration.
- **Skill** (`/to-issues`) — Claude Code skill that produces correctly-labeled issues from a plan/PRD, lives in `.claude/skills/to-issues/SKILL.md` in the target repo.

## Threat model and safety design

**The risk:** `claude` runs in `bypassPermissions` mode (no prompts, full tool access) so the agent doesn't get stuck overnight on a missing tool permission. Without isolation, a confused or prompt-injected agent could `rm -rf` your home directory or exfiltrate credentials.

**The defenses, layered:**

1. **Kernel boundary (primary):** Docker container with `--read-only` root, `--cap-drop ALL`, `--security-opt no-new-privileges`, dedicated unprivileged user. Filesystem damage is bounded to the bind-mounted worktree directory. The host's `/home`, `/etc`, dotfiles, and other repos are unreachable.
2. **Resource caps:** `--memory 4g --cpus 2` — runaway loops can't OOM the host.
3. **Disposable worktrees:** Each iteration gets a fresh worktree at `${REPO}/.alucard-worktrees/iter-N`. Orchestrator removes it after each iteration. `EXIT` trap handles interrupted runs.
4. **PR-only output:** Agent never pushes to main. Branch protection on main as belt-and-suspenders.
5. **Credential scoping:** GitHub token is fine-grained PAT, single repo, 30-day expiry. Anthropic key is dedicated worker key with monthly budget cap set in the console.
6. **Pattern blacklist (last line):** `--disallowedTools` removes obvious foot-guns like `rm -rf /*`, `sudo`, `curl | sh`. Pattern-matching is leaky but cheap.
7. **Hard caps per iteration:** `--max-turns 60`, `--max-budget-usd 5`, `timeout 30m`.

**What's still possible:** credential exfiltration via network. Bounded by token scoping — worst case is "attacker gets push access to one repo for up to 30 days," which is recoverable.

## File layout

```
alucard/
├── README.md                    # this file
├── alucard                      # CLI
├── alucard.sh                   # compatibility shim
├── Dockerfile                   # node:20-slim + git + gh + claude code + alucard user
├── entrypoint.sh                # configures git identity and gh auth at container start
├── alucard-prompt.md            # worker system prompt
├── alucard.env.example          # template for credentials (real one is gitignored)
└── .gitignore                   # ignores alucard.env and logs/
```

The target repo (the one Alucard works on) needs:

```
target-repo/
├── .gitignore                   # add: .alucard-worktrees/
├── .claude/
│   └── skills/
│       └── to-issues/
│           └── SKILL.md         # the to-issues skill
└── (your code)
```

Alucard tooling is designed to live in a separate folder so it is reusable across projects. Point the CLI at a target repo with a positional path, `--repo`, or `ALUCARD_TARGET_REPO`.

## Setup checklist for the new folder

### Prerequisites on the host

- Docker installed and the user in the `docker` group (or `sudo` available)
- `git`, `gh`, `jq` installed on host (host orchestrator uses them for queue inspection)
- `gh auth login` already done as the human user (separate from the container's auth)
- The target repo cloned locally with at least one commit on `main` and a remote on GitHub

### Install the CLI

Make the scripts executable:

```bash
chmod +x /home/aldo/aldovc/alucard/alucard
chmod +x /home/aldo/aldovc/alucard/alucard.sh
chmod +x /home/aldo/aldovc/alucard/entrypoint.sh
```

Optional: put it on your `PATH`:

```bash
ln -sf /home/aldo/aldovc/alucard/alucard ~/.local/bin/alucard
```

Use it from anywhere:

```bash
alucard run /path/to/target-repo --iterations 1 --timeout-minutes 10
alucard queue /path/to/target-repo
alucard doctor /path/to/target-repo
```

### Credentials to provision

Create the local env file:

```bash
cp /home/aldo/aldovc/alucard/alucard.env.example /home/aldo/aldovc/alucard/alucard.env
```

1. **GitHub fine-grained PAT:**
   - github.com → Settings → Developer settings → Personal access tokens → Fine-grained
   - Repository access: only the target repo
   - Permissions: Contents R/W, Issues R/W, Pull requests R/W, Metadata R
   - Expiry: 30 days
   - Paste into `alucard.env` as `GITHUB_TOKEN=`

2. **Anthropic worker API key:**
   - console.anthropic.com → API keys → create new key named "alucard-worker"
   - In the workspace settings, set a monthly spend cap (e.g. $50)
   - Paste into `alucard.env` as `ANTHROPIC_API_KEY=`

### One-time setup commands in target repo

```bash
# Add gitignores
cat >> .gitignore <<'EOF'
.alucard-worktrees/
EOF

# Create labels
for L in afk hitl in-progress wip tracer-bullet alucard; do
  gh label create "$L" --force
done

# Branch protection on main (via gh or GitHub UI):
# Require PR, require status checks, no direct pushes
gh api -X PUT "repos/{owner}/{repo}/branches/main/protection" \
  --input - <<'EOF'
{
  "required_status_checks": null,
  "enforce_admins": false,
  "required_pull_request_reviews": {"required_approving_review_count": 0},
  "restrictions": null
}
EOF
```

### The `/to-issues` skill

Drop the SKILL.md from earlier in the conversation into `.claude/skills/to-issues/SKILL.md` in the target repo. This is what Aldo invokes during the day to convert a PRD into properly-labeled issues that Alucard can then pick up overnight.

### First smoke test

```bash
# Build the image
/home/aldo/aldovc/alucard/alucard build

# Create one trivial AFK issue manually for the test
cd /path/to/target-repo
gh issue create --label afk --label tracer-bullet \
  --title "Add a CHANGELOG.md" \
  --body "## Type
AFK

## What to build
Create a CHANGELOG.md at repo root with a Keep a Changelog template.

## Acceptance criteria
- [ ] CHANGELOG.md exists at repo root
- [ ] Contains 'Unreleased' section header
- [ ] Linked from README.md

## Blocked by
None — can start immediately"

# Run one iteration with a 10-min cap and watch
/home/aldo/aldovc/alucard/alucard run /path/to/target-repo -n 1 -t 10
```

If a PR appears within 10 minutes, the setup works. If not, check `/home/aldo/aldovc/alucard/logs/alucard-*/iter-1.jsonl` for what the agent saw and did.

## Open questions for the setup agent

These are choices Aldo hasn't locked yet — surface them rather than guessing:

1. **Project toolchain in the Dockerfile.** The current Dockerfile has Node + git + gh + claude. If the target repo uses Python/uv/just/Rust/etc., those need to be added or the agent will fail to run `just test`.
2. **Branch protection enforcement.** Does Aldo want admin enforcement on, or off so he can hotfix? Default off is fine for solo work.
3. **Notification on completion.** Currently the loop just exits. Aldo might want a Telegram/Discord ping when the run finishes.
4. **Concurrent iterations.** Current design is sequential — one container at a time. If Aldo wants parallel workers picking different issues, the `in-progress` label coordination needs to handle race conditions (gh API isn't atomic for label-add). Don't add unless asked.

## Quick-reference commands

```bash
# Run overnight (20 iterations max, 30 min each)
alucard run /path/to/target-repo -n 20 -t 30

# Run one iteration to test
alucard run /path/to/target-repo -n 1 -t 15

# Check what's in the queue right now
alucard queue /path/to/target-repo

# Manually un-stick an issue if Alucard crashed mid-task
gh issue edit <N> --remove-label in-progress

# Prune leftover worktrees if the orchestrator was hard-killed
git worktree prune
rm -rf .alucard-worktrees/

# Watch a live run
tail -f /home/aldo/aldovc/alucard/logs/alucard-*/iter-*.jsonl | jq -r 'select(.type=="assistant").message.content[]?.text // empty'
```

## Design references for the setup agent

The full design conversation that produced this is summarized as:
- **Worker loop** based on the "ralph" pattern (issues-as-queue, agent-as-worker, sentinel-for-termination), hardened to use bash-side queue filtering rather than model-side, PRs not main, worktree isolation per iteration, hard timeouts and budget caps.
- **Issue creation** via `/to-issues` skill that produces vertical tracer-bullet slices, AFK/HITL labeled, with size constraints (~10 files / ~30 minutes per AFK slice) and explicit blocker tracking via `Blocked by #N` body convention.
- **Containerization** chosen over unprivileged-user-only after weighing the threat model — kernel boundary is the only real defense against `rm -rf /`, and the operational cost is low for a single-machine homelab setup.
