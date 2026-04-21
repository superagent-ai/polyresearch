# polyresearch

Distributed [autoresearch](https://github.com/karpathy/autoresearch). Multiple machines, multiple contributors, verified results.

Autoresearch gives an AI agent a codebase and a metric, and lets it experiment autonomously -- modify the code, run the eval, keep or discard, repeat. You wake up to a log of experiments and a better result. But it runs one agent on one machine. The agent self-reports its own metrics, failed experiments are lost to `git reset`, and nobody else can contribute.

Polyresearch keeps the same loop and adds three things:

1. **Open participation.** Any number of machines, run by any number of people, contribute to the same project through a shared repo. A laptop and a dedicated server both claim work from the same queue and submit to the same experiment log.
2. **Complete experiment history.** Every attempt gets a row in `results.tsv` and stays as an unmerged branch: accepted, discarded, and crashed. No `git reset`, no lost code. The full history feeds thesis generation and prevents repeating dead ends.
3. **Independent verification.** Reviewers rerun the evaluation on the candidate *and* on the baseline, measuring both numbers themselves. The evaluation code lives outside the editable surface, so agents cannot grade their own homework.

## Install

```bash
cargo install polyresearch
```

Don't have Rust? See [other install options](cli/README.md#other-install-options).

## Usage

Polyresearch has two roles: a **lead** and one or more **contributors**. The human who owns the repo is the **maintainer** -- they review the research playbook and optionally approve work.

### 1. Bootstrap a project

Point it at any GitHub repo with a codebase and a metric you want to improve:

```bash
polyresearch bootstrap https://github.com/owner/repo
```

Use `--goal` to tell the agent what you're optimizing for -- it pre-fills the Goal section of `PROGRAM.md`. Bootstrap checks whether you have push access; if not, it forks to your GitHub account automatically. See the [full flag list](cli/README.md#command-summary) for `--fork`, `--no-fork`, and other options.

This writes template files, initializes your machine as a node, and pauses for you to review before spawning the bootstrap agent. When it finishes you'll have:

- `**PROGRAM.md**` -- the research playbook. Describes the goal, which files agents can edit, strategy hints. This is the only file agents read.
- `**PREPARE.md**` -- the evaluation setup. Benchmark command, metric parsing, ground truth. Lives outside the editable surface so agents can't change how they're judged.
- `**results.tsv**` -- the experiment ledger. Every attempt ever recorded.
- `**.polyresearch-node.toml**` -- your machine's identity and capacity setting. Gitignored.

Review `PROGRAM.md` and `PREPARE.md`, tweak them for your project, commit, and push.

### 2. Run the lead

From the root of your project (where you ran `bootstrap`):

```bash
polyresearch lead
```

The lead syncs the results ledger, policy-checks open PRs, decides candidates (merge or reject), and generates new theses when the queue runs low. It runs in a loop until you stop it. By default, agents run with `claude -p --dangerously-skip-permissions`. Override with `--agent-command` to use a different model or flags, e.g. `--agent-command "claude -p --dangerously-skip-permissions --model sonnet"`. Use `--once` for a single iteration. See the [full flag list](cli/README.md#command-summary) for all options.

### 3. Run contributors

On any machine (yours, a teammate's, a rented GPU box):

```bash
polyresearch contribute https://github.com/owner/repo
```

The contributor clones the repo, claims theses from the issue queue, spawns an agent for each one, records results, and submits PRs. Launch as many contributor machines as you want -- they all pull from the same queue. Use `--capacity` to limit how much of the machine polyresearch can use, or `--agent-command` to override the default agent. See the [full flag list](cli/README.md#command-summary) for all options.

### Run on a remote machine

Run the contributor directly on the server so file access, git operations, and evaluations are all local. Use `tmux` so the session survives disconnects:

```bash
ssh user@remote-host
tmux new-session -s polyresearch
polyresearch contribute https://github.com/owner/repo
# Detach with Ctrl-B D
```

Detach and reconnect later with `tmux attach -t polyresearch` -- the session persists even if your SSH connection drops.

## CLI

The `polyresearch` CLI handles all coordination: claiming theses, recording attempts, submitting candidates, syncing results, deciding PRs. Agents don't need to understand the protocol. The CLI runs the protocol; agents run experiments.

Full command reference in [cli/README.md](cli/README.md).

## Design

**Agent-agnostic.** The CLI handles all coordination. Agents only need to read `PROGRAM.md`, run experiments, and report results. No opinions on model, sandbox, or language.

**Structured comments as state.** Coordination happens through structured HTML comments on GitHub Issues and PRs. State is derived from the comment trail, not from labels or a database. Every transition is append-only and auditable.

**The evaluation is the trust boundary.** `PREPARE.md` defines how results are judged. The evaluation code lives outside the editable surface. Agents cannot modify the evaluator or the scoring logic.

**Human-in-the-loop.** Set `auto_approve: false` and the lead waits for the maintainer to `/approve` or `/reject` each thesis and PR. Maintainer feedback steers future thesis generation.

**Hardware utilization.** Each node sets a `capacity` percentage in `.polyresearch-node.toml` (default 75). `polyresearch pace` probes the hardware, prints the project's share plus live load, and determines how many theses to run in parallel given each eval's footprint.

## Examples


| Example                      | What it does                                                                                                                                                      |
| ---------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| [corewar](examples/corewar/) | Evolve a Redcode warrior against a frozen gauntlet. Free to evaluate, fast iteration, deterministic results. 218% score improvement over 27 experiments.          |
| [eslint](examples/eslint/)   | Optimize ESLint's core linting performance on a dual-workload benchmark. Real-world codebase, V8-level depth. Single-file linting 24% faster over 75 experiments. |
| [postcss](examples/postcss/) | Optimize PostCSS's CSS processing on a dual-workload benchmark. Plugin pipeline 16% faster over 50 experiments.                                                   |


## License

MIT 

By Superagent Technologies, Inc.