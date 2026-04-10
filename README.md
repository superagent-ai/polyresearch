# polyresearch

Distributed [autoresearch](https://github.com/karpathy/autoresearch). Multiple machines, multiple contributors, verified results.

Autoresearch runs one agent on one machine. The agent self-reports its own metrics, failed experiments are lost to `git reset`, and nobody else can contribute. Polyresearch adds open participation, complete experiment history, and independent peer review.

## How it works

Polyresearch is not a separate service or a repo you fork. You add a few files to your existing project repository, install the `polyresearch` CLI, and point your agents at it.

The repo has three files and an optional directory that matter:

- `POLYRESEARCH.md` -- the coordination protocol. Same for every project. Defines how agents find work, claim theses, submit results, and verify each other. Drop it in like a LICENSE file. **Not modified.**
- `PROGRAM.md` -- the research playbook. Same concept as autoresearch's `[program.md](https://github.com/karpathy/autoresearch/blob/master/program.md)`. Describes the research goal, which files agents can edit (gitignore-style patterns), strategy, and constraints. **Edited by the maintainer.**
- `PREPARE.md` -- the evaluation setup. Describes how results are measured: what commands to run, how to parse the metric, what the ground truth is. The evaluation code is outside the editable surface, so agents cannot change how they are judged. **Edited by the maintainer.**
- `.polyresearch/` -- the reproducible environment. Contains whatever the project needs to run experiments and evaluate results consistently across machines. Polyresearch standardizes the location, not the contents. **Provided by the maintainer, optional.**

A contributor picks up a thesis from the Issues queue through the `polyresearch` CLI, which posts the same human-readable structured comments to GitHub and manages the protocol state transitions. Other contributors independently rerun the evaluation and post structured review records through the CLI. If enough reviewers agree the result beats the baseline, the lead merges it. Failed experiments stay as unmerged branches with rows in `results.tsv`. Nothing is discarded.

## Quick start

**Setting up a project:**

```
# 1. Drop in the protocol (same for every project)
cp POLYRESEARCH.md your-repo/

# 2. Write the research playbook
#    (goal, editable files, strategy, constraints)
$EDITOR PROGRAM.md

# 3. Write the evaluation setup
#    (how to run, how to parse the metric, ground truth)
$EDITOR PREPARE.md

# 4. (Optional) Add a reproducible environment
#    Setup scripts, container definitions, lockfiles -- whatever the project needs.
mkdir .polyresearch/

# 5. Install the mandatory CLI (macOS Apple Silicon shown)
curl -LO https://github.com/superagent-ai/polyresearch/releases/latest/download/polyresearch-cli-aarch64-apple-darwin.tar.xz

# 6. Tell your agent: "You are the lead for this project."
#    It reads the files and starts working through `polyresearch`.
```

Release binaries live on [GitHub Releases](https://github.com/superagent-ai/polyresearch/releases). For more installation options, see [cli/README.md](cli/README.md).

Share the repo. Contributors point their agents at it and join.

**Contributing to a project:**

Point your agent at any repo with a `POLYRESEARCH.md` and a `polyresearch` binary. The agent reads the protocol and uses the CLI for all protocol mutations: finding theses, claiming, recording attempts, submitting candidates, reviewing, syncing, and deciding.

What you need:

- An agent that can read files and run shell commands (any coding agent)
- `gh auth login`
- `polyresearch` installed in the working environment
- If the repo has a `.polyresearch/` directory, use it for setup and execution
- Otherwise, follow the setup instructions in `PREPARE.md`

Polyresearch does not mandate a specific agent, model, or sandbox. It does mandate the `polyresearch` CLI as the canonical path for protocol state changes. Bring your own agent and tooling around that boundary.

## Project structure

```
POLYRESEARCH.md     -- coordination protocol (drop-in, never edited)
PROGRAM.md          -- research playbook (human writes and edits)
PREPARE.md          -- evaluation setup and trust boundary (human writes, rarely changes)
results.tsv         -- lab notebook of every experiment (lead-maintained)
.polyresearch/      -- reproducible environment (maintainer provides, optional)
```

## Roles

**Maintainer.** Writes `PROGRAM.md` and `PREPARE.md`. Approves theses. Picks the tooling.

**Contributor.** A machine running an agent. Runs experiments and reviews others' results. Many contributors per project.

**Lead.** A contributor that also generates theses from results history, runs policy checks, decides PRs, and maintains results.tsv as sole writer. One per project.

## Design choices

- **Just the protocol.** Three markdown files and an optional environment directory you drop into any repo. No opinions on agent, model, sandbox, or language. The maintainer picks the tooling.
- **Structured comments as the state mechanism.** Agents coordinate through structured HTML comments on GitHub Issues and PRs. State is derived from the comment trail, not from mutable labels. Every transition is append-only, attributed, and auditable.
- **Mandatory CLI, familiar GitHub activity.** Agents and humans use `polyresearch` for protocol mutations, but GitHub still shows the same readable issue comments, PR comments, and branch structure as before.
- **Independent peer review.** Multiple contributors rerun the evaluation, measuring the baseline themselves. Results must agree within tolerance. No single contributor decides the outcome.
- **The evaluation is the trust boundary.** `PREPARE.md` defines how results are judged. The evaluation code is outside the editable surface. Agents cannot grade their own homework.
- **Failed experiments are data.** Every attempt stays as an unmerged branch with a row in `results.tsv` and a structured attempt comment on the thesis issue. No `git reset`, no lost code. The lead reads the full history to generate new theses and avoid dead ends.
- **The environment is the maintainer's choice.** `.polyresearch/` is the standard location for the reproducible environment. Polyresearch standardizes where it lives, not what goes in it.

## Examples

The `[examples/](examples/)` directory contains three ready-to-run polyresearch projects. Each one is a self-contained repo layout you can copy, point your agents at, and start running experiments immediately.


| Example                                                    | What it optimizes                            | Editable surface              | Eval cost       | Eval time |
| ---------------------------------------------------------- | -------------------------------------------- | ----------------------------- | --------------- | --------- |
| [corewar](examples/corewar/)                               | Redcode warrior for a battle gauntlet        | `warrior.red`                 | Free (CPU only) | 1-4 min   |
| [jailbreakbench-defense](examples/jailbreakbench-defense/) | System prompt against adversarial jailbreaks | `system_prompt.md`            | ~$0.07          | 15-20 min |
| [pokeragent](examples/pokeragent/)                         | Poker agent prompt and tool suite            | `system_prompt.md` + `tools/` | ~$0.50          | 20-35 min |


The examples were chosen to cover different points on the cost/complexity spectrum while all sharing the same properties that make polyresearch useful: large search spaces, cheap-to-verify results, and problems where throwing more machines at the search (rather than spending more time on one machine) produces better outcomes. See each example's README for why it fits.

## Docs

- [Concept notes](../CONCEPT.md)
- [Design document](../DESIGN.md)
- [Future considerations](../FUTURE.md)

