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

## Command summary

High-level orchestration:

- `polyresearch bootstrap <url>` -- auto-fork (or `--fork <org>`, `--no-fork`), write templates, init node, spawn setup agent. Accepts `--capacity`, `--api-budget`, `--request-delay`, `--agent-command` to set initial node config values.
- `polyresearch lead` -- continuous lead loop: sync, policy-check, decide, generate. Accepts `--capacity`, `--api-budget`, `--request-delay`, `--agent-command` as runtime overrides.
- `polyresearch contribute [url]` -- continuous contributor loop: claim, experiment, record, submit. Accepts `--capacity`, `--api-budget`, `--request-delay`, `--agent-command` as runtime overrides.

Status and inspection:

- `polyresearch init` -- set node identity and config (`--capacity`, `--api-budget`, `--request-delay`, `--agent-command`), verify GitHub auth, detect repo
- `polyresearch pace` -- show the hardware budget (machine, your max, live free), GitHub API budget, active claims, and recent node throughput
- `polyresearch status` -- derive thesis state, queue depth, active nodes, current best metric
- `polyresearch audit` -- validate raw GitHub activity and report invalid or suspicious protocol events
- `polyresearch duties` -- list blocking and advisory work items for the current node

Contributor:

- `polyresearch claim` -- atomically claim a thesis and create the thesis worktree at `.worktrees/<issue>-<slug>/`
- `polyresearch batch-claim` -- claim up to the node's free thesis slots and create one worktree per thesis
- `polyresearch attempt` -- post a structured attempt comment from the current branch
- `polyresearch release` -- release a claim with a structured reason
- `polyresearch submit` -- push the branch and open a candidate PR

Review:

- `polyresearch review-claim` -- claim a PR for review
- `polyresearch review` -- post a structured review record with env hash

Lead:

- `polyresearch sync` -- reconcile `results.tsv` from the comment trail
- `polyresearch generate` -- open a thesis issue (auto-approves when `auto_approve` is `true`, otherwise assigns to maintainer)
- `polyresearch policy-check` -- validate PR files against the editable surface
- `polyresearch decide` -- post the lead decision and merge/close accordingly (waits for maintainer `/approve` when `auto_approve` is `false`)
- `polyresearch prune` -- prune git worktree metadata and remove empty stale directories under `.worktrees`
- `polyresearch admin ...` -- lead-only repair commands for exceptional recovery

## Notes

- The comment trail is the source of truth for all protocol activity.
- Canonical protocol state is derived from validated events only.
- `results.tsv` is a lead-maintained ledger derived from canonical history.
- Raw manual GitHub edits may remain visible for auditability, but they are non-canonical and may be ignored by the tooling.
