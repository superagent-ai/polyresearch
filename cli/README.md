# polyresearch CLI

The `polyresearch` CLI is the orchestration layer for the polyresearch protocol. It handles all coordination -- claims, submissions, reviews, syncing, PR decisions, thesis generation -- so agents only need to read `PROGRAM.md`, run experiments, and write results.

Three high-level commands (`bootstrap`, `lead`, `contribute`) run complete loops and spawn agents as subprocesses. The low-level commands below exist for debugging and manual operation.

## Install

```bash
cargo install polyresearch
```

### Other install options

<details>
<summary>Download a pre-built binary</summary>

Pre-built archives are available from [GitHub Releases](https://github.com/superagent-ai/polyresearch/releases).

| OS | Architecture | Archive |
| --- | --- | --- |
| macOS | Apple Silicon | `polyresearch-aarch64-apple-darwin.tar.xz` |
| macOS | Intel | `polyresearch-x86_64-apple-darwin.tar.xz` |
| Linux | x86_64 (glibc) | `polyresearch-x86_64-unknown-linux-gnu.tar.xz` |

Each archive expands to a directory containing `polyresearch` and `README.md`. Move `polyresearch` somewhere on your `PATH`, such as `~/.local/bin` or `/usr/local/bin`.

**macOS (Apple Silicon):**

```bash
curl -LO https://github.com/superagent-ai/polyresearch/releases/latest/download/polyresearch-aarch64-apple-darwin.tar.xz
tar -xJf polyresearch-aarch64-apple-darwin.tar.xz
sudo cp polyresearch-aarch64-apple-darwin/polyresearch /usr/local/bin/
```

**macOS (Intel):**

```bash
curl -LO https://github.com/superagent-ai/polyresearch/releases/latest/download/polyresearch-x86_64-apple-darwin.tar.xz
tar -xJf polyresearch-x86_64-apple-darwin.tar.xz
sudo cp polyresearch-x86_64-apple-darwin/polyresearch /usr/local/bin/
```

**Linux (x86_64):**

```bash
curl -LO https://github.com/superagent-ai/polyresearch/releases/latest/download/polyresearch-x86_64-unknown-linux-gnu.tar.xz
tar -xJf polyresearch-x86_64-unknown-linux-gnu.tar.xz
sudo cp polyresearch-x86_64-unknown-linux-gnu/polyresearch /usr/local/bin/
```

</details>

<details>
<summary>Build from source</summary>

From the repo root:

```bash
cargo install --path cli
```

</details>

## Requirements

- `git`
- `gh`
- A repo containing `PROGRAM.md` and `PREPARE.md`
- GitHub authentication through `gh auth login`

The CLI can read `GITHUB_TOKEN`, but by default it uses the existing `gh` authentication on the machine.

## Releasing

Release tags follow `vX.Y.Z` and must match the version in [Cargo.toml](Cargo.toml). Bump the crate version, push the matching tag, and GitHub Actions publishes the archives and checksums to [GitHub Releases](https://github.com/superagent-ai/polyresearch/releases).

## Quick start

Bootstrap a new project:

```bash
polyresearch bootstrap https://github.com/owner/repo
polyresearch bootstrap https://github.com/owner/repo --fork myorg
polyresearch bootstrap https://github.com/owner/repo --no-fork
```

Bootstrap auto-forks when you lack push access to the target repo. Use `--fork <org>` to fork to a specific organization, or `--no-fork` to clone directly.

Run the lead loop:

```bash
polyresearch lead
polyresearch lead --once    # single iteration, then exit
polyresearch lead --agent-command "codex --full-auto"
```

Run as a contributor:

```bash
polyresearch contribute https://github.com/owner/repo
polyresearch contribute --once
polyresearch contribute --max-parallel 4
polyresearch contribute --capacity 50 --agent-command "codex --full-auto"
```

All four node config settings can be overridden at runtime via `--capacity`, `--api-budget`, `--request-delay`, and `--agent-command`. These are pure runtime overrides and do not modify `.polyresearch-node.toml`.

These three commands handle complete workflows. The sections below document the lower-level commands for manual operation and debugging.

### Hardware utilization

A single evaluation often doesn't saturate a machine. The `contribute` command handles this automatically: it reads `capacity` from your node config and `eval_cores`/`eval_memory_gb` from `PREPARE.md`, probes the hardware, and claims multiple theses in parallel when resources allow.

`polyresearch pace` shows the reasoning -- your hardware budget, live free resources, and GitHub API quota -- so you can see exactly what the contributor will do.

## Manual workflow

Initialize the local node identity (the high-level commands do this automatically):

```bash
polyresearch init
polyresearch init --capacity 50
polyresearch init --api-budget 10000 --request-delay 200 --agent-command "codex --full-auto"
```

This writes `.polyresearch-node.toml` in the repo root. The file stores a stable `node_id`, a `capacity` percentage (1..=100) giving the share of the total machine this project may use, an `api_budget` for GitHub API rate pacing, a `request_delay_ms` between API calls, and an `[agent] command` for the experiment runner.

Inspect the current state:

```bash
polyresearch pace
polyresearch status
polyresearch audit
polyresearch pace --json
polyresearch status --json
polyresearch status --tui
```

Debug agent subprocess failures by showing the full command line and working directory:

```bash
polyresearch --verbose contribute --once
POLYRESEARCH_VERBOSE=1 polyresearch lead --once
```

Debug GitHub traffic when you need to inspect request pacing or rate-limit headers:

```bash
polyresearch --github-debug pace --json
POLYRESEARCH_GITHUB_DEBUG=1 polyresearch status
```

Contributor flow:

```bash
polyresearch claim 88
polyresearch attempt 88 --metric 0.6244 --baseline 0.5000 --observation improved --summary "River all-in with two pair+"
polyresearch submit 88
polyresearch release 88 --reason no_improvement
polyresearch batch-claim
```

`polyresearch claim` creates a dedicated worktree at `.worktrees/<issue>-<slug>/` from `main` and prints the path. Change into that worktree before editing or running evaluations. The main worktree stays on `main` so the lead can sync, decide, and generate without races from concurrent contributors.

`polyresearch batch-claim --count N` claims `N` approved theses and creates one worktree per thesis. Pick `N` from `polyresearch pace` after dividing your effective hardware budget by each eval's resource footprint.

Review flow:

```bash
polyresearch review-claim 93
polyresearch review 93 --metric 0.6244 --baseline 0.5000 --observation improved
```

Lead flow:

```bash
polyresearch sync
polyresearch duties
polyresearch generate --title "New thesis" --body "Hypothesis and rationale"
polyresearch policy-check 93
polyresearch decide 93
polyresearch prune
polyresearch admin reconcile-ledger
```

Run the lead from the repository root on `main`. Launch contributors as separate agents in their own thesis worktrees.

Maintainer approval (when `auto_approve` is `false` in `PROGRAM.md`):

The maintainer comments `/approve` or `/reject` directly on thesis issues and candidate PRs in GitHub. Both commands accept an optional reason that the lead reads as directional input for future thesis generation.

```
/approve focus on normalization layers
/reject this direction already failed for architectural reasons
```

## Output modes

- Default output is human-readable terminal text.
- `--json` returns structured JSON for agent consumption.
- `status --tui` opens the ratatui dashboard for a live human view.

## Command reference

### Global flags

These flags apply to every command.

| Flag | Type | Default | Description |
|------|------|---------|-------------|
| `--repo <owner/repo>` | string | current repo | Target repository |
| `--github-debug` | bool | `false` | Log GitHub API requests and responses |
| `--json` | bool | `false` | Output structured JSON instead of human-readable text |
| `--dry-run` | bool | `false` | Preview actions without side effects |
| `--verbose` | bool | `false` | Show full subprocess commands and working directories on failure (env: `POLYRESEARCH_VERBOSE`) |

### Node overrides

Shared by `init`, `bootstrap`, `lead`, and `contribute`. These are runtime overrides and do not modify `.polyresearch-node.toml`.

| Flag | Type | Default | Description |
|------|------|---------|-------------|
| `--capacity <n>` | int (1-100) | from config | Percentage of the machine this project may use |
| `--api-budget <n>` | int | from config | GitHub API rate budget (max requests) |
| `--request-delay <ms>` | int | from config | Milliseconds to wait between API calls |
| `--agent-command <cmd>` | string | from config | Shell command for the experiment runner |

### Enum values

Several flags accept a fixed set of values.

**Observation** (used by `attempt --observation` and `review --observation`):

`improved` | `no_improvement` | `crashed` | `infra_failure`

**ReleaseReason** (used by `release --reason` and `admin release-claim --reason`):

`no_improvement` | `timeout` | `infra_failure`

---

### High-level orchestration

#### `bootstrap`

Auto-fork, write templates, init node, and spawn the setup agent.

Usage: `polyresearch bootstrap <url> [flags]`

| Flag | Type | Default | Description |
|------|------|---------|-------------|
| `--fork <org>` | string | — | Fork to a specific organization |
| `--no-fork` | bool | `false` | Clone directly without forking (conflicts with `--fork`) |
| `--goal <text>` | string | — | Initial research goal |
| `--pause-after-bootstrap` | bool | `false` | Exit after setup instead of starting the loop |

Also accepts [node overrides](#node-overrides).

#### `lead`

Continuous lead loop: sync, policy-check, decide, generate.

Usage: `polyresearch lead [flags]`

| Flag | Type | Default | Description |
|------|------|---------|-------------|
| `--once` | bool | `false` | Run a single iteration then exit |
| `--sleep-secs <n>` | int | `60` | Seconds to sleep between loop iterations |

Also accepts [node overrides](#node-overrides).

#### `contribute`

Continuous contributor loop: claim, experiment, record, submit.

Usage: `polyresearch contribute [url] [flags]`

| Flag | Type | Default | Description |
|------|------|---------|-------------|
| `--once` | bool | `false` | Run a single iteration then exit |
| `--max-parallel <n>` | int | — | Maximum number of parallel thesis evaluations |
| `--sleep-secs <n>` | int | `60` | Seconds to sleep between loop iterations |

Also accepts [node overrides](#node-overrides).

---

### Status and inspection

#### `init`

Set node identity and config. Writes `.polyresearch-node.toml` in the repo root.

Usage: `polyresearch init [flags]`

| Flag | Type | Default | Description |
|------|------|---------|-------------|
| `--node <id>` | string | auto-generated | Explicit node identifier |

Also accepts [node overrides](#node-overrides).

#### `pace`

Show the hardware budget (machine resources, your max, live free), GitHub API quota, active claims, and recent node throughput.

Usage: `polyresearch pace`

No command-specific flags.

#### `status`

Derive thesis state, queue depth, active nodes, and current best metric.

Usage: `polyresearch status [flags]`

| Flag | Type | Default | Description |
|------|------|---------|-------------|
| `--tui` | bool | `false` | Open the ratatui live dashboard |

#### `audit`

Validate raw GitHub activity and report invalid or suspicious protocol events.

Usage: `polyresearch audit`

No command-specific flags.

#### `duties`

List blocking and advisory work items for the current node.

Usage: `polyresearch duties`

No command-specific flags.

---

### Contributor

#### `claim`

Atomically claim a thesis and create the thesis worktree at `.worktrees/<issue>-<slug>/`.

Usage: `polyresearch claim <issue>`

| Argument | Type | Description |
|----------|------|-------------|
| `<issue>` | int | GitHub issue number (required) |

#### `batch-claim`

Claim up to the node's free thesis slots and create one worktree per thesis.

Usage: `polyresearch batch-claim [flags]`

| Flag | Type | Default | Description |
|------|------|---------|-------------|
| `--count <n>` | int | — | Number of theses to claim (defaults to available slots) |

#### `attempt`

Post a structured attempt comment from the current branch.

Usage: `polyresearch attempt <issue> --metric <f> --baseline <f> --observation <obs> --summary <text> [flags]`

| Argument / Flag | Type | Default | Description |
|-----------------|------|---------|-------------|
| `<issue>` | int | — | GitHub issue number (required) |
| `--metric <f>` | float | — | Measured metric value (required) |
| `--baseline <f>` | float | — | Baseline metric value (required) |
| `--observation <obs>` | Observation | — | Experiment outcome (required); see [enum values](#enum-values) |
| `--summary <text>` | string | — | Human-readable summary of the attempt (required) |
| `--annotations <text>` | string | — | Additional structured annotations |

#### `annotate`

Post an annotation comment on a thesis issue.

Usage: `polyresearch annotate <issue> --text <text>`

| Argument / Flag | Type | Default | Description |
|-----------------|------|---------|-------------|
| `<issue>` | int | — | GitHub issue number (required) |
| `--text <text>` | string | — | Annotation text (required) |

#### `release`

Release a claim with a structured reason.

Usage: `polyresearch release <issue> --reason <reason>`

| Argument / Flag | Type | Default | Description |
|-----------------|------|---------|-------------|
| `<issue>` | int | — | GitHub issue number (required) |
| `--reason <reason>` | ReleaseReason | — | Why the claim is being released (required); see [enum values](#enum-values) |

#### `submit`

Push the branch and open a candidate PR.

Usage: `polyresearch submit <issue>`

| Argument | Type | Description |
|----------|------|-------------|
| `<issue>` | int | GitHub issue number (required) |

---

### Review

#### `review-claim`

Claim a PR for review.

Usage: `polyresearch review-claim <pr>`

| Argument | Type | Description |
|----------|------|-------------|
| `<pr>` | int | GitHub pull request number (required) |

#### `review`

Post a structured review record with environment hash.

Usage: `polyresearch review <pr> --metric <f> --baseline <f> --observation <obs>`

| Argument / Flag | Type | Default | Description |
|-----------------|------|---------|-------------|
| `<pr>` | int | — | GitHub pull request number (required) |
| `--metric <f>` | float | — | Measured metric value (required) |
| `--baseline <f>` | float | — | Baseline metric value (required) |
| `--observation <obs>` | Observation | — | Review outcome (required); see [enum values](#enum-values) |

---

### Lead

#### `sync`

Reconcile `results.tsv` from the comment trail.

Usage: `polyresearch sync`

No command-specific flags.

#### `generate`

Open a thesis issue. Auto-approves when `auto_approve` is `true` in `PROGRAM.md`, otherwise assigns to the maintainer.

Usage: `polyresearch generate --title <text> --body <text>`

| Flag | Type | Default | Description |
|------|------|---------|-------------|
| `--title <text>` | string | — | Thesis title (required) |
| `--body <text>` | string | — | Hypothesis and rationale (required) |

#### `policy-check`

Validate PR files against the editable surface.

Usage: `polyresearch policy-check <pr>`

| Argument | Type | Description |
|----------|------|-------------|
| `<pr>` | int | GitHub pull request number (required) |

#### `decide`

Post the lead decision and merge or close accordingly. Waits for maintainer `/approve` when `auto_approve` is `false`.

Usage: `polyresearch decide <pr>`

| Argument | Type | Description |
|----------|------|-------------|
| `<pr>` | int | GitHub pull request number (required) |

#### `prune`

Prune git worktree metadata and remove empty stale directories under `.worktrees`.

Usage: `polyresearch prune`

No command-specific flags.

---

### Admin

Lead-only repair commands for exceptional recovery. All nested under `polyresearch admin`.

#### `admin release-claim`

Force-release a stale or invalid claim.

Usage: `polyresearch admin release-claim <issue> --node <id> --reason <reason> [flags]`

| Argument / Flag | Type | Default | Description |
|-----------------|------|---------|-------------|
| `<issue>` | int | — | GitHub issue number (required) |
| `--node <id>` | string | — | Node that holds the claim (required) |
| `--reason <reason>` | ReleaseReason | — | Why the claim is being released (required); see [enum values](#enum-values) |
| `--note <text>` | string | `"Lead repair released the stale or invalid claim."` | Explanatory note posted with the release |

#### `admin acknowledge-invalid`

Acknowledge and dismiss an invalid protocol comment.

Usage: `polyresearch admin acknowledge-invalid <comment-id> --note <text>`

| Argument / Flag | Type | Default | Description |
|-----------------|------|---------|-------------|
| `<comment-id>` | int | — | GitHub comment ID (required) |
| `--note <text>` | string | — | Reason for acknowledging (required) |

#### `admin reopen-thesis`

Reopen a previously closed thesis issue.

Usage: `polyresearch admin reopen-thesis <issue> [flags]`

| Argument / Flag | Type | Default | Description |
|-----------------|------|---------|-------------|
| `<issue>` | int | — | GitHub issue number (required) |
| `--note <text>` | string | `"Lead repair reopened the thesis."` | Explanatory note posted with the reopen |

#### `admin reconcile-ledger`

Rebuild `results.tsv` from canonical protocol history.

Usage: `polyresearch admin reconcile-ledger`

No command-specific flags.

## Notes

- The comment trail is the source of truth for all protocol activity.
- Canonical protocol state is derived from validated events only.
- `results.tsv` is a lead-maintained ledger derived from canonical history.
- Raw manual GitHub edits may remain visible for auditability, but they are non-canonical and may be ignored by the tooling.
