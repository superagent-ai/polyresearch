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
polyresearch bootstrap https://github.com/owner/repo --fork myorg
```

This clones the repo (or forks it first with `--fork`), writes template files, initializes your machine as a node, and spawns an agent to fill in project-specific details. When it finishes you'll have:

- **`PROGRAM.md`** -- the research playbook. Describes the goal, which files agents can edit, strategy hints. This is the only file agents read.
- **`PREPARE.md`** -- the evaluation setup. Benchmark command, metric parsing, ground truth. Lives outside the editable surface so agents can't change how they're judged.
- **`results.tsv`** -- the experiment ledger. Every attempt ever recorded.
- **`.polyresearch-node.toml`** -- your machine's identity and capacity setting. Gitignored.

Review `PROGRAM.md` and `PREPARE.md`, tweak them for your project, commit, and push.

### 2. Run the lead

```bash
polyresearch lead
```

The lead syncs the results ledger, policy-checks open PRs, decides candidates (merge or reject), and generates new theses when the queue runs low. It runs in a loop until you stop it. Use `--once` for a single iteration.

### 3. Run contributors

On any machine (yours, a teammate's, a rented GPU box):

```bash
polyresearch contribute https://github.com/owner/repo
```

The contributor clones the repo, claims theses from the issue queue, spawns an agent for each one, records results, and submits PRs. Launch as many contributor machines as you want -- they all pull from the same queue.

### Hardware utilization

A single evaluation often doesn't saturate a machine. The `contribute` command handles this automatically: it reads `capacity` from your node config and `eval_cores`/`eval_memory_gb` from `PREPARE.md`, probes the hardware, and claims multiple theses in parallel when resources allow.

`polyresearch pace` shows the reasoning -- your hardware budget, live free resources, and GitHub API quota -- so you can see exactly what the contributor will do.

### Run on a remote machine

Run the contributor directly on the server so file access, git operations, and evaluations are all local. Use `tmux` so the session survives disconnects:

```bash
ssh user@remote-host
tmux new-session -s polyresearch
polyresearch contribute https://github.com/owner/repo
# Detach with Ctrl-B D. Reconnect with: tmux attach -t polyresearch
```

If your laptop sleeps or your network drops, the contributor keeps working. Reconnect later with `tmux attach -t polyresearch`.

## CLI

The `polyresearch` CLI handles all coordination: claiming theses, recording attempts, submitting candidates, syncing results, deciding PRs. Agents don't need to understand the protocol. The CLI runs the protocol; agents run experiments.

Full command reference in [cli/README.md](cli/README.md).

## Design

**Protocol, not a platform.** Two markdown files (`PROGRAM.md` and `PREPARE.md`) and an optional environment directory dropped into any repo. No opinions on agent, model, sandbox, or language.

**Structured comments as state.** Coordination happens through structured HTML comments on GitHub Issues and PRs. State is derived from the comment trail, not from labels or a database. Every transition is append-only and auditable.

**Claim-based work distribution.** Theses live on GitHub Issues. Contributors claim them atomically through the CLI. Stale claims expire after a configurable timeout and return to the queue.

**The evaluation is the trust boundary.** `PREPARE.md` defines how results are judged. The evaluation code lives outside the editable surface. Agents cannot modify the evaluator or the scoring logic.

**Peer review.** When enabled, reviewers independently check out the candidate and the baseline, run the evaluation themselves, and post their own measurements. The lead only merges when reviewers agree.

**Human-in-the-loop.** Set `auto_approve: false` and the lead waits for the maintainer to `/approve` or `/reject` each thesis and PR. Maintainer feedback steers future thesis generation.

**Failed experiments are data.** Every attempt gets a row in `results.tsv` and stays as an unmerged branch. The lead reads the full history to generate new theses and avoid dead ends.

**Resource pacing.** Each node sets a `capacity` percentage in `.polyresearch-node.toml` (default 75). `polyresearch pace` probes the hardware, prints the project's share plus live load, and determines how many theses to run in parallel given each eval's footprint.

## Examples


| Example                                      | What it does                                                                                                 |
| -------------------------------------------- | ------------------------------------------------------------------------------------------------------------ |
| [corewar](examples/corewar/)                 | Evolve a Redcode warrior against a frozen gauntlet. Free to evaluate, fast iteration, deterministic results. 218% score improvement over 27 experiments. |
| [eslint](examples/eslint/)                   | Optimize ESLint's core linting performance on a dual-workload benchmark. Real-world codebase, V8-level depth. Single-file linting 24% faster over 75 experiments. |
| [postcss](examples/postcss/)                 | Optimize PostCSS's CSS processing on a dual-workload benchmark. Plugin pipeline 16% faster over 50 experiments. |


## License

MIT 

By Superagent Technologies, Inc.
