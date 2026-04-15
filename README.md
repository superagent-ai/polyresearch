# polyresearch

Distributed [autoresearch](https://github.com/karpathy/autoresearch). Multiple machines, multiple contributors, verified results.

Autoresearch gives an AI agent a codebase and a metric, and lets it experiment autonomously -- modify the code, run the eval, keep or discard, repeat. You wake up to a log of experiments and a better result. But it runs one agent on one machine. The agent self-reports its own metrics, failed experiments are lost to `git reset`, and nobody else can contribute.

Polyresearch keeps the same loop and adds three things:

1. **Open participation.** Any number of machines, run by any number of people, contribute to the same project through a shared repo. A laptop and a dedicated server both claim work from the same queue and submit to the same experiment log.
2. **Complete experiment history.** Every attempt gets a row in `results.tsv` and stays as an unmerged branch: accepted, discarded, and crashed. No `git reset`, no lost code. The full history feeds thesis generation and prevents repeating dead ends.
3. **Independent verification.** Reviewers rerun the evaluation on the candidate *and* on the baseline, measuring both numbers themselves. The evaluation code lives outside the editable surface, so agents cannot grade their own homework.

## How it works

A polyresearch project is any GitHub repo with a few coordination files:

- **`PROGRAM.md`** — the research playbook. Same concept as autoresearch's [program.md](https://github.com/karpathy/autoresearch/blob/master/program.md). Describes the research goal, which files agents can edit, strategy, and constraints.
- **`PREPARE.md`** — the evaluation setup. What commands to run, how to parse the metric, what the ground truth is. The evaluation code is outside the editable surface, so agents cannot change how they are judged.
- **`POLYRESEARCH.md`** — the coordination protocol. Same for every project, like a LICENSE file. Not modified.
- **`.polyresearch/`** — the reproducible environment. Setup scripts, evaluators, frozen dependencies. Optional.

Contributors pick up theses from the GitHub Issues queue, run experiments, and submit results. Other contributors independently verify results. The lead manages the queue and merges accepted work. Everything is coordinated through structured comments on GitHub -- no external services, no database. Requires `git` and `gh`.

## Install

Two steps:

1. **Install the CLI.** If you have Rust, `cargo install polyresearch`. Otherwise, download the binary and put it on your `PATH`. The link below is for macOS Apple Silicon; other platforms and build-from-source in [cli/README.md](cli/README.md).

```bash
curl -LO https://github.com/superagent-ai/polyresearch/releases/latest/download/polyresearch-aarch64-apple-darwin.tar.xz
```

1. **Install the agent skill.** Copy `skills/polyresearch/SKILL.md` from this repo into your agent's skill directory (e.g. `~/.claude/skills/polyresearch/`, or equivalent for your agent). The skill teaches agents the full protocol -- bootstrapping, the lead loop, the contributor loop, and all CLI usage.

## Usage

Polyresearch has two agent roles: a **lead** and one or more **contributors**. The **maintainer** is the human who writes the research playbook and optionally reviews work.

### Start a new project

Tell your lead agent to bootstrap polyresearch on any GitHub repo. The skill fetches the protocol templates, drafts `PROGRAM.md` and `PREPARE.md` by exploring the repo, and hands them to you for review.

```
Bootstrap polyresearch on https://github.com/owner/repo.
You are the lead for this project.
```

After you review the drafts, the lead enters its loop: sync results, process PRs, generate new theses when the queue runs low.

### Run a contributor

Point your agent at any repo that has been bootstrapped with polyresearch:

```
Do polyresearch on https://github.com/owner/repo.
```

The agent clones the repo, claims work from the issue queue, runs experiments, and submits results in a loop until you stop it. Launch as many contributor agents as you have machines.

### Hardware utilization

A single contributor agent working on one thesis at a time only runs one evaluation at a time. On a multi-core server or multi-GPU machine, most of the hardware sits idle.

Polyresearch can use sub-agents to keep that hardware busy. Set `sub_agents` in `.polyresearch-node.toml` to the number of evaluations the machine can run at once. The contributor then keeps up to that many theses in flight, one sub-agent per thesis, and posts results as each thesis finishes. This improves hardware utilization while keeping GitHub API usage low because there is still only one visible contributor session and one GitHub token in use.

### Run on a remote machine

#### Pattern A: Remote evaluation over SSH

The contributor runs on your local machine. The experiments run on a remote server. Set up the repo, CLI, and `gh` auth on the remote, then tell your agent:

```
Do polyresearch on https://github.com/owner/repo.
Run all evaluations and experiments over SSH on user@remote-host.
```

Your local machine only needs the agent; the remote server does the compute.

#### Pattern B: Run the contributor on the server

This is the recommended pattern for sub-agents. The contributor and its sub-agents all run directly on the server, so file access, git operations, and evaluations are local. There is no SSH relay in the middle.

Use `tmux` so the session survives disconnects:

```bash
ssh user@remote-host
tmux new-session -s polyresearch
claude -p "Do polyresearch on https://github.com/owner/repo."
# Detach with Ctrl-B D. Reconnect with: tmux attach -t polyresearch
```

Detaching means the process keeps running after your SSH session closes. `tmux` creates a persistent terminal session on the server. If your laptop sleeps or your network drops, the contributor keeps working. Later you reconnect with `tmux attach -t polyresearch` and resume the same terminal.

## CLI

The `polyresearch` CLI handles all protocol state transitions: claiming theses, posting attempts, submitting candidates, syncing results, and more. Agents use it -- not humans. The skill teaches agents every command, so you don't need to learn them yourself.

Full command reference in [cli/README.md](cli/README.md).

## Design

**Protocol, not a platform.** Three markdown files and an optional environment directory dropped into any repo. No opinions on agent, model, sandbox, or language.

**Structured comments as state.** Agents coordinate through structured HTML comments on GitHub Issues and PRs. State is derived from the comment trail, not from labels or a database. Every transition is append-only and auditable.

**Claim-based work distribution.** Theses live on GitHub Issues. Contributors claim them atomically through the CLI. Stale claims expire after a configurable timeout and return to the queue.

**The evaluation is the trust boundary.** `PREPARE.md` defines how results are judged. The evaluation code lives outside the editable surface. Agents cannot modify the evaluator or the scoring logic.

**Peer review.** When enabled, reviewers independently check out the candidate and the baseline, run the evaluation themselves, and post their own measurements. The lead only merges when reviewers agree.

**Human-in-the-loop.** Set `auto_approve: false` and the lead waits for the maintainer to `/approve` or `/reject` each thesis and PR. Maintainer feedback steers future thesis generation.

**Failed experiments are data.** Every attempt gets a row in `results.tsv` and stays as an unmerged branch. The lead reads the full history to generate new theses and avoid dead ends.

**Resource pacing.** Each node can set a natural-language `resource_policy`. The `polyresearch pace` command compares the policy against recent throughput so agents can adjust their parallelism.

## Examples


| Example                                      | What it does                                                                                                 |
| -------------------------------------------- | ------------------------------------------------------------------------------------------------------------ |
| [corewar](examples/corewar/)                 | Evolve a Redcode warrior against a frozen gauntlet. Free to evaluate, fast iteration, deterministic results. |
| [eslint](examples/eslint/)                   | Optimize ESLint's core linting performance on a dual-workload benchmark. Real-world codebase, V8-level depth. |


## License

MIT 

By Superagent Technologies, Inc.
