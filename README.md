# polyresearch

Distributed [autoresearch](https://github.com/karpathy/autoresearch), with coordination.

Autoresearch gives a coding agent a codebase, a metric, and a loop: read `program.md`, modify code, run the eval, keep or revert, repeat. Polyresearch keeps that exact agent experience, but wraps it in a CLI that adds shared queues, claims, review, results syncing, and multi-machine coordination.

In short:

- **Autoresearch**: one agent, one machine, one branch.
- **Polyresearch**: the same experiment loop, coordinated across many machines and contributors.

## How It Works

A polyresearch project is any repo with these files:

- `PROGRAM.md` — the agent-facing experiment loop, constraints, editable surface, and result contract. This is the direct analogue of autoresearch's `program.md`.
- `PREPARE.md` — evaluation setup: benchmark command, output format, metric parsing, and resource footprint.
- `results.tsv` — the experiment ledger, maintained by the CLI.
- `.polyresearch/` — harnesses and runtime files such as `bench.js`, `thesis.md`, and `result.json`.

The agent reads `PROGRAM.md`, `PREPARE.md`, and `.polyresearch/thesis.md`, then runs experiments exactly like autoresearch. The CLI handles everything the agent should not have to reason about: claiming work, syncing results, opening PRs, policy-checking, deciding PRs, and coordinating across machines.

## Install

```bash
cargo install polyresearch
```

No external skill installation is required.

## Quick Start

### 1. Bootstrap a project

```bash
polyresearch bootstrap https://github.com/owner/repo --goal "make it faster"
```

This clones the target repo, writes `PROGRAM.md` and `PREPARE.md` templates, initializes `.polyresearch-node.toml`, and spawns your configured coding agent to fill in the project-specific details.

If you want to review the files before running anything else:

```bash
polyresearch bootstrap https://github.com/owner/repo   --goal "make it faster"   --pause-after-bootstrap
```

### 2. Start the lead loop

From the project root:

```bash
polyresearch lead
```

The lead loop is deterministic. It syncs `results.tsv`, processes open PRs, and only spawns an agent for the creative part: generating new thesis ideas when the queue is low.

### 3. Add contributors

From another machine, or the same one:

```bash
polyresearch contribute
```

Or contribute directly from a repo URL:

```bash
polyresearch contribute https://github.com/owner/repo
```

The contributor loop is also deterministic. It claims theses, spawns one coding agent per worktree, records attempts, submits improvements, releases dead ends, and scales parallelism automatically from `capacity` and `PREPARE.md` resource hints.

Useful flags:

```bash
polyresearch contribute --once --max-parallel 1
```

- `--once` runs a single contributor-loop pass and exits. This is ideal for smoke tests.
- `--max-parallel N` places a manual cap on the auto-scaled parallelism.
- `--max-parallel 1` means “work exactly one thesis at a time.”

## Bring Your Own Agent

Polyresearch is agent-agnostic. The CLI shells out to whatever headless coding agent you configure.

`.polyresearch-node.toml`:

```toml
capacity = 80

[agent]
command = "claude -p --permission-mode bypassPermissions"
```

Default is Claude Code headless. Change it to any agent that accepts a prompt and works in a directory:

```toml
[agent]
command = "codex --prompt"
command = "cursor --bg --prompt"
command = "python3 ~/my-agent.py --prompt"
```

## Running on a Remote Machine

Run the CLI directly on the server. Don't tell an agent to SSH and work remotely from inside its own prompt.

```bash
ssh user@remote-host
cd /path/to/project
polyresearch contribute
```

For long-running contributors, use `tmux` or a service manager such as `systemd`.

## Why CLI-First

The overnight failures that motivated this rewrite mostly came from agents being bad protocol operators: wrong commands, wrong order, wrong repo, wrong machine, bad GitHub API usage.

The CLI-first model fixes that split of responsibilities:

- **Agent**: read `PROGRAM.md`, run experiments, write `.polyresearch/result.json`.
- **CLI**: claim, submit, release, sync, policy-check, decide, review, pace, and queue management.

That keeps the autoresearch spirit intact while removing the bookkeeping burden from the agent.

## Advanced Usage

### Metaresearch

Polyresearch can wrap itself.

A meta-project can treat each thesis as “optimize github.com/owner/repo”. The agent-facing loop in the meta-project's `PROGRAM.md` can call:

- `polyresearch bootstrap`
- `polyresearch lead`
- `polyresearch contribute`

on the target repo, then use the resulting improvement as the metric for the meta-thesis.

That means the same system can:

- pick candidate repos,
- launch real polyresearch projects for them,
- record which categories of repos yield wins,
- and use that history to generate better project picks over time.

No separate orchestration framework is required. Polyresearch can coordinate polyresearch.

## Design Principles

- **Autoresearch-compatible.** `PROGRAM.md` is the agent interface, just like autoresearch's `program.md`.
- **Deterministic coordination.** The CLI owns protocol state transitions.
- **Structured GitHub state.** Claims, attempts, releases, reviews, and decisions live in GitHub comments and PRs.
- **All attempts are data.** The full history stays visible in `results.tsv`.
- **Machine-aware concurrency.** `capacity` plus `PREPARE.md` resource hints control parallelism.
- **Agent-agnostic.** Any headless agent that accepts a prompt can participate.

## Examples

| Example | What it does |
| --- | --- |
| [corewar](examples/corewar/) | Evolve a Redcode warrior against a frozen gauntlet. |
| [eslint](examples/eslint/) | Optimize ESLint's linting performance on real workloads. |
| [postcss](examples/postcss/) | Optimize PostCSS CSS processing on benchmarked workloads. |

## CLI Reference

The full command reference lives in [cli/README.md](cli/README.md).

## License

MIT

By Superagent Technologies, Inc.
