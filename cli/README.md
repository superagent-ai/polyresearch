# polyresearch CLI

`polyresearch` is the deterministic coordination layer for distributed autoresearch.

The agent-facing loop lives in `PROGRAM.md`. The CLI handles the shared state machine: claims, attempts, releases, syncing, PR flow, review, and queue management.

## Requirements

- `git`
- `gh`
- a repo containing `PROGRAM.md` and `PREPARE.md`
- GitHub authentication via `gh auth login`

The CLI uses your existing `gh` authentication by default.

## Install

```bash
cargo install polyresearch
```

### Other install options

Pre-built archives are available from [GitHub Releases](https://github.com/superagent-ai/polyresearch/releases).

## High-Level Commands

These are the primary entry points. Most users only need these.

### `polyresearch bootstrap <url>`

Bootstraps a new project.

```bash
polyresearch bootstrap https://github.com/owner/repo --goal "make it faster"
```

What it does:

1. forks/clones the repo,
2. writes `PROGRAM.md`, `PREPARE.md`, and `results.tsv` if missing,
3. initializes `.polyresearch-node.toml`,
4. spawns your configured coding agent to fill in the project-specific details.

Flags:

- `--fork <owner>` or `--fork <owner/name>`
- `--goal "<text>"`
- `--pause-after-bootstrap`

### `polyresearch lead`

Runs the deterministic lead loop.

```bash
polyresearch lead
polyresearch lead --once
```

The lead loop:

- resolves blocking duties,
- syncs `results.tsv`,
- policy-checks open PRs,
- decides ready PRs,
- spawns an agent only when it needs new thesis ideas.

Useful flag:

- `--once` — run a single lead-loop iteration and exit. Good for smoke tests and debugging.

### `polyresearch contribute [url]`

Runs the deterministic contributor loop.

```bash
polyresearch contribute
polyresearch contribute https://github.com/owner/repo
polyresearch contribute --once --max-parallel 1
```

The contributor loop:

- claims approved theses,
- creates worktrees,
- writes `.polyresearch/thesis.md`,
- spawns one coding agent per worktree,
- records attempts,
- submits wins and releases dead ends,
- scales parallelism automatically from `capacity` and `PREPARE.md` resource hints.

Override parallelism manually if needed:

```bash
polyresearch contribute --max-parallel 4
```

Useful flags:

- `--once` — run a single contributor-loop iteration and exit.
- `--max-parallel N` — place a manual cap on auto-scaled parallelism.
- `--max-parallel 1` — force one thesis at a time. This is the safest smoke-test mode.

## Agent Configuration

The CLI spawns a coding agent for creative work: bootstrap, thesis generation, and experiments.

Configure it in `.polyresearch-node.toml`:

```toml
capacity = 80

[agent]
command = "claude -p --permission-mode bypassPermissions"
```

Default is Claude Code headless. You can replace it with any compatible agent:

```toml
[agent]
command = "codex --prompt"
command = "cursor --bg --prompt"
command = "python3 ~/my-agent.py --prompt"
```

## Runtime Files

The CLI and agent communicate through files inside the repo:

- `PROGRAM.md` — the agent reads this as the experiment loop.
- `PREPARE.md` — benchmark and metric parsing instructions.
- `.polyresearch/thesis.md` — thesis-specific context written by the CLI.
- `.polyresearch/result.json` — experiment result written by the agent.
- `results.tsv` — the synced ledger of attempts.

Expected `.polyresearch/result.json` shape:

```json
{
  "metric": 0.0,
  "baseline": 0.0,
  "observation": "improved",
  "summary": "What changed.",
  "attempts": [
    {
      "metric": 0.0,
      "summary": "What this attempt tried."
    }
  ]
}
```

## Node Initialization

Initialize the local node identity once per repo:

```bash
polyresearch init
polyresearch init --capacity 50
```

This writes `.polyresearch-node.toml` with:

- `node_id`
- `capacity` (percent of total machine)
- `api_budget`
- `request_delay_ms`
- `[agent] command`

## Inspecting State

```bash
polyresearch pace
polyresearch status
polyresearch audit
polyresearch status --json
polyresearch status --tui
```

`pace` reports:

- hardware budget from `capacity`,
- live free memory/load,
- GitHub API budget,
- recent node throughput.

## Low-Level Commands

These are the building blocks used internally by `lead` and `contribute`. They remain available for manual debugging, custom automation, or advanced workflows.

- `polyresearch claim`
- `polyresearch batch-claim`
- `polyresearch attempt`
- `polyresearch release`
- `polyresearch submit`
- `polyresearch review-claim`
- `polyresearch review`
- `polyresearch sync`
- `polyresearch generate`
- `polyresearch policy-check`
- `polyresearch decide`
- `polyresearch duties`
- `polyresearch prune`
- `polyresearch admin ...`

## Notes

- The comment trail remains the canonical shared state.
- The CLI derives protocol state from validated events only.
- `results.tsv` is maintained by the lead loop and sync logic.
- All GitHub traffic goes through the CLI, not through the experiment agent.

