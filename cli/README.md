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

From the repo root:

```bash
cargo install --path cli
```

That installs the `polyresearch` binary from [cli/Cargo.toml](cli/Cargo.toml).

## Requirements

- `git`
- `gh`
- a repo containing `POLYRESEARCH.md`, `PROGRAM.md`, and `PREPARE.md`
- GitHub authentication through `gh auth login`

The CLI can read `GITHUB_TOKEN`, but by default it uses the existing `gh` authentication on the machine.

## Basic workflow

Initialize the local node identity once per repo:

```bash
polyresearch init
```

Inspect the current state:

```bash
polyresearch status
polyresearch audit
polyresearch status --json
polyresearch status --tui
```

Contributor flow:

```bash
polyresearch claim 88
polyresearch attempt 88 --metric 0.6244 --baseline 0.5000 --observation improved --summary "River all-in with two pair+"
polyresearch submit 88
polyresearch release 88 --reason no_improvement
```

Review flow:

```bash
polyresearch review-claim 93
polyresearch review 93 --metric 0.6244 --baseline 0.5000 --observation improved
```

Lead flow:

```bash
polyresearch sync
polyresearch generate --title "New thesis" --body "Hypothesis and rationale"
polyresearch policy-check 93
polyresearch decide 93
polyresearch admin reconcile-ledger
```

## Output modes

- Default output is human-readable terminal text.
- `--json` returns structured JSON for agent consumption.
- `status --tui` opens the ratatui dashboard for a live human view.

## Command summary

- `polyresearch init` -- set node identity, verify GitHub auth, detect repo
- `polyresearch status` -- derive thesis state, queue depth, active nodes, current best metric
- `polyresearch audit` -- validate raw GitHub activity and report invalid or suspicious protocol events
- `polyresearch claim` -- atomically claim a thesis and create the thesis branch
- `polyresearch attempt` -- post a structured attempt comment from the current branch
- `polyresearch release` -- release a claim with a structured reason
- `polyresearch submit` -- push the branch and open a candidate PR
- `polyresearch review-claim` -- claim a PR for review
- `polyresearch review` -- post a structured review record with env hash
- `polyresearch sync` -- reconcile `results.tsv` from the comment trail
- `polyresearch generate` -- open and auto-approve a thesis issue
- `polyresearch policy-check` -- validate PR files against the editable surface
- `polyresearch decide` -- post the lead decision and merge/close accordingly
- `polyresearch admin ...` -- lead-only repair commands for exceptional recovery

## Notes

- The comment trail remains the visible source of activity.
- Canonical protocol state is derived from validated CLI-compatible events only.
- `results.tsv` is treated as a lead-maintained ledger derived from canonical history.
- Raw manual GitHub edits may remain visible for auditability, but they are non-canonical and may be ignored by the lead and the tooling.
- The CLI keeps normal GitHub comments and PR activity human-readable while abstracting the protocol bookkeeping into the tool itself.

