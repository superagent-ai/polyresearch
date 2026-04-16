# polyresearch CLI

`polyresearch` is the mandatory Rust CLI and terminal dashboard for the polyresearch protocol.

It is designed to be used by agents and humans as the canonical path for protocol state transitions, while still being ergonomic for anyone who wants a live status view.

## Why it exists

The protocol stores truth in GitHub issue comments, PR comments, and branches. In practice, contributors repeatedly re-implement the same state derivation logic and get it wrong:

- double-claiming already claimed theses
- generating new theses while `results.tsv` is stale
- posting malformed structured comments
- continuing work after a thesis has already been resolved

This CLI centralizes that logic so every machine uses the same implementation and the lead can validate canonical state against raw GitHub activity.

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
- a repo containing `POLYRESEARCH.md`, `PROGRAM.md`, and `PREPARE.md`
- GitHub authentication through `gh auth login`

The CLI can read `GITHUB_TOKEN`, but by default it uses the existing `gh` authentication on the machine.

## Releasing

Release tags follow `vX.Y.Z` and must match the version in [cli/Cargo.toml](cli/Cargo.toml). Bump the crate version, push the matching tag, and GitHub Actions publishes the archives and checksums to [GitHub Releases](https://github.com/superagent-ai/polyresearch/releases).

## Basic workflow

Initialize the local node identity once per repo:

```bash
polyresearch init
polyresearch init --sub-agents 4 --resource-policy "Run 4 evals in parallel, stay under 50 API calls/min"
```

This writes `.polyresearch-node.toml` in the repo root. The file stores a stable `node_id`, a `sub_agents` limit, and an optional natural-language `resource_policy`.

Inspect the current state:

```bash
polyresearch pace
polyresearch status
polyresearch audit
polyresearch pace --json
polyresearch status --json
polyresearch status --tui
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

`polyresearch batch-claim` is the multi-thesis version for contributors using sub-agents. It fills the node's free thesis slots up to `sub_agents`, creates one worktree per thesis, and requires worktrees for parallel execution.

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

- `polyresearch init` -- set node identity, sub-agent limit, optional resource policy, verify GitHub auth, detect repo
- `polyresearch pace` -- show configured sub-agents, active claims, free slots, effective resource policy, and recent node throughput
- `polyresearch status` -- derive thesis state, queue depth, active nodes, current best metric
- `polyresearch audit` -- validate raw GitHub activity and report invalid or suspicious protocol events
- `polyresearch claim` -- atomically claim a thesis and create the thesis worktree at `.worktrees/<issue>-<slug>/`
- `polyresearch batch-claim` -- claim up to the node's free thesis slots and create one worktree per thesis
- `polyresearch attempt` -- post a structured attempt comment from the current branch
- `polyresearch release` -- release a claim with a structured reason
- `polyresearch submit` -- push the branch and open a candidate PR
- `polyresearch review-claim` -- claim a PR for review
- `polyresearch review` -- post a structured review record with env hash
- `polyresearch sync` -- reconcile `results.tsv` from the comment trail
- `polyresearch generate` -- open a thesis issue (auto-approves when `auto_approve` is `true`, otherwise assigns to maintainer)
- `polyresearch policy-check` -- validate PR files against the editable surface
- `polyresearch decide` -- post the lead decision and merge/close accordingly (waits for maintainer `/approve` when `auto_approve` is `false`)
- `polyresearch duties` -- list blocking and advisory work items for the current node
- `polyresearch prune` -- prune git worktree metadata and remove empty stale directories under `.worktrees`
- `polyresearch admin ...` -- lead-only repair commands for exceptional recovery

## Notes

- The comment trail remains the visible source of activity.
- Canonical protocol state is derived from validated CLI-compatible events only.
- `results.tsv` is treated as a lead-maintained ledger derived from canonical history.
- Raw manual GitHub edits may remain visible for auditability, but they are non-canonical and may be ignored by the lead and the tooling.
- The CLI keeps normal GitHub comments and PR activity human-readable while abstracting the protocol bookkeeping into the tool itself.

