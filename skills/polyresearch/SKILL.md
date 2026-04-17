---
name: polyresearch
description: >-
  Coordinate distributed AI research using the polyresearch protocol and CLI.
  Use when working in a repo containing POLYRESEARCH.md, claiming theses,
  running experiments, submitting candidates, reviewing PRs, or performing
  lead duties like syncing results, generating theses, and deciding PRs.
  Triggers on: polyresearch, thesis, results.tsv, PROGRAM.md, PREPARE.md,
  or any polyresearch CLI command.
---

# Polyresearch Agent Skill

## Before you start

1. Read these files in order: `POLYRESEARCH.md`, `PROGRAM.md`, `PREPARE.md`, `results.tsv`.
2. Run `git log --oneline -20` on `main` to see recent state.
3. Create a distinct node ID for this session before running other `polyresearch` commands:
   ```bash
   LOGIN=$(gh api user --jq '.login')
   MACHINE_ID="$(hostname -s)-$(xxd -l2 -p /dev/urandom)"
   export POLYRESEARCH_NODE_ID="${LOGIN}/${MACHINE_ID}"
   ```
4. If `.polyresearch-node.toml` does not exist yet, run `polyresearch init --node "$MACHINE_ID"` to create the fallback file.
5. If `.polyresearch/` exists, run its setup. Otherwise follow `PREPARE.md`.
6. Check your GitHub identity: the `LOGIN` above must match the GitHub user you were asked to operate as.
7. Identify your role solely from your launch instructions:
   - If told "you are the lead," follow the lead loop.
   - Otherwise, follow the contributor loop.
   - Matching `lead_github_login` does NOT make you the lead. An empty
     queue does NOT make you the lead.
8. Read `capacity` from `.polyresearch-node.toml` if it exists (default `75` if absent, meaning "75% of total machine"). Run `polyresearch pace` to see the probed hardware budget and decide how many theses to run in parallel based on each eval's footprint in PREPARE.md. If only one eval fits, work one thesis at a time; if several fit, use `polyresearch batch-claim --count N` and one sub-agent per worktree (see "Running multiple sub-agents" below).
9. If the repo is a fork and issues are disabled:
   `gh api repos/{owner}/{name} --method PATCH -f has_issues=true`
10. For any CLI command details: `polyresearch <command> --help`

`POLYRESEARCH_NODE_ID` takes precedence over `.polyresearch-node.toml` for the current session. This is required when multiple agents share one checkout or when one GitHub login runs several workers in parallel.

## Bootstrap new projects

When bootstrapping a new project, fetch the three template files directly from the polyresearch repo:

```
https://raw.githubusercontent.com/superagent-ai/polyresearch/main/POLYRESEARCH.md
https://raw.githubusercontent.com/superagent-ai/polyresearch/main/PROGRAM.md
https://raw.githubusercontent.com/superagent-ai/polyresearch/main/PREPARE.md
```

Download these into the target project root. Then explore the repo, fill in `PROGRAM.md` (research goal, editable surface, config values) and `PREPARE.md` (how to run and score experiments), and hand both drafts to the maintainer for review before launching agents.

If `PROGRAM.md` or `PREPARE.md` already exist but still contain placeholders such as `replace-me`, they still need to be filled in.

## Core principle

GitHub visibility first. All work must be visible on GitHub. Local-only work is
invisible to other contributors, the lead, and the maintainer.

The `polyresearch duties` command enforces this. The CLI gates `claim` and
`generate` on it: those commands refuse to proceed if blocking duties exist.

## The contributor loop

See `POLYRESEARCH.md` "The contributor loop" for full sub-step details.

```
LOOP FOREVER:

  0. polyresearch duties
     If BLOCKING items exist, resolve each one before continuing.

  1. polyresearch pace
     Look at the Hardware budget block (machine, your max share, live free,
     multi-project note). Effective working budget = min(your max, live free).
     Divide by each eval's footprint (cores, RAM, GPU from PREPARE.md) to
     decide how many theses you can run in parallel this iteration.
     Also watch the API budget block: back off if near the GitHub quota.

  2. polyresearch status
     Look for approved, unclaimed theses.

  3. If a claimable thesis exists:
     a. For one thesis: polyresearch claim <issue>
        For several at once: polyresearch batch-claim --count N
        (see "Running multiple sub-agents" below)
     b. cd into the worktree path printed by claim.
     c. Read PROGRAM.md for direction and constraints.
     d. For each experiment:
        - Make changes within the editable surface (PROGRAM.md CAN list).
        - Run evaluator per PREPARE.md: <run-command> > run.log 2>&1
        - Parse the metric per PREPARE.md.
        - IMMEDIATELY post:
          polyresearch attempt <issue> --metric <val> --baseline <val> \
            --observation <obs> --summary "<summary>"
          Add `--annotations '<json>'` if you have structured findings future
          contributors should see.
        - polyresearch duties
     e. If observation was improved:
        polyresearch submit <issue>
        Do this NOW. Do not keep tinkering.
     f. If no improvement after exhausting ideas:
        polyresearch release <issue> --reason <reason>
        If you learned something future contributors should know, post:
        polyresearch annotate <issue> --text "<what you learned>"
     g. When the thesis is released or later resolved:
        git worktree remove <worktree-path>
        Return to the repo root before claiming again.
        Immediately continue from step 0. Do not end the session after one
        thesis cycle.

  4. Check for review work (PRs with policy-pass, no decision,
     not authored by you):
     a. polyresearch review-claim <pr>
     b. Create a disposable review worktree at the candidate SHA:
        git worktree add .worktrees/review-<pr> <candidate-sha>
        cd into it. Evaluate per PREPARE.md.
     c. In the same review worktree: git checkout <base-sha>
        Evaluate per PREPARE.md.
     d. polyresearch review <pr> --metric <candidate> --baseline <base> \
          --observation <obs>
     e. git worktree remove .worktrees/review-<pr>

  5. Repeat from step 0.
```

## Running multiple sub-agents

When `polyresearch pace` shows your effective budget fits more than one
evaluation at a time, run several theses in parallel. This is a variant of
step 3 in the contributor loop above, not a separate loop.

```
  3a. polyresearch batch-claim --count N
      Claims N approved theses and creates one worktree per thesis.
      Pick N from your effective budget / per-eval footprint.

  3b. For each claimed thesis, dispatch one sub-agent:
      - Give it the issue number and worktree path.
      - Tell it to read PROGRAM.md and PREPARE.md.
      - Tell it to work only in its assigned worktree.
      - Tell it to return every completed attempt with metric, baseline,
        observation, and summary.
      - Tell it NOT to run polyresearch CLI commands.
      - Tell it NOT to talk to GitHub.

  3c. As each sub-agent finishes its thesis:
      - post every returned attempt with `polyresearch attempt`
      - if any attempt improved, `polyresearch submit <issue>`
      - otherwise, `polyresearch release <issue> --reason no_improvement`
      - `git worktree remove` the finished thesis worktree
```

## The lead loop

The lead runs a separate management loop from the repository root worktree,
which stays on `main`. The lead never claims theses or runs experiments.
See `POLYRESEARCH.md` "The lead loop" for full sub-procedure details.

```
LOOP FOREVER:

  0. polyresearch duties
     Resolve ALL blocking items. Lead blocking items include:
     - Decidable PRs without decisions
     - Open PRs without policy-check
     - Stale results.tsv

  1. polyresearch pace          # inspect the hardware budget and throughput
  2. polyresearch sync          # on main branch, always first
  3. polyresearch audit         # check for inconsistencies
     A dirty audit blocks `policy-check`, `decide`, and `generate`.

  4. Process open PRs:
     - For each PR without policy-check:
       polyresearch policy-check <pr>
     - For each PR with enough reviews and no decision:
       - If `auto_approve` is `false`, wait for the maintainer to comment `/approve`.
         The lead should assign the PR to `maintainer_github_login` while it waits.
       polyresearch decide <pr>

  5. Check queue depth:
     - If below min_queue_depth:
       polyresearch generate --title "<title>" --body "<body>"
     - If `max_queue_depth` is set and queue depth is already at or above it:
       do not generate.
     - If `auto_approve` is `false`, generated theses are not auto-approved.
       They stay queued for the maintainer to `/approve` or `/reject`.
     - Read results.tsv and all thesis history before generating.
     - Read annotations on closed theses before generating. Treat them as
       negative knowledge.
     - Read maintainer `/approve` and `/reject` comments and use them as
       directional input for future thesis generation.
     - Deduplicate against existing open and closed theses.

  6. Wait briefly, then repeat from step 0.
```

## Resource pacing

Run `polyresearch pace` regularly. It prints:

- **Hardware budget**: the detected machine (cores, memory, GPUs), your project's
  max share at the configured `capacity %`, and a live-free snapshot (load
  average, available memory) that reflects all processes on the host. Effective
  working budget is `min(Your max, Live free)` divided by each evaluation's
  footprint. Other polyresearch projects on the same host are on the
  honor-system: their `capacity` values are not tracked by the CLI, so confirm
  the sum across projects is safe yourself.
- **API budget**: GitHub core quota, commands left, near-limit flag.
- **Throughput**: active claims for this node, attempts in the last hour and
  four hours, claimable theses idle.

See `POLYRESEARCH.md` "Node configuration" for full field semantics.

## Critical rules

These rules are extracted from `POLYRESEARCH.md`. The protocol is authoritative
if any wording differs.

- The main worktree stays on `main`. Never run `git checkout`, `git switch`, or
  any other HEAD-changing command in the repo root. All thesis work happens in
  `.worktrees/<issue>-<slug>/` and all review checkouts happen in
  `.worktrees/review-<pr>/`. Breaking this invariant breaks the lead loop for
  every concurrent agent on the same checkout.
- Without sub-agents, post `polyresearch attempt` after every experiment.
- With sub-agents, post every returned attempt as each sub-agent finishes its
  thesis. Sub-agents do not post to GitHub directly.
- Run `polyresearch submit` immediately when you observe `improved`.
  Do not "keep trying."
- After `submit` or `release`, finish cleanup and then continue the loop from
  step 0. Ending the session after one thesis cycle is a protocol violation.
- Lead: process the PR backlog before any other work. Period.
- Lead: re-run `polyresearch sync` after any decision that closes a thesis.
- Never modify files in `.polyresearch/` or files outside the editable surface
  in `PROGRAM.md`.
- Observations use snake_case: `improved`, `no_improvement`, `crashed`,
  `infra_failure`.
- If an experiment exceeds 2x the expected time (per PREPARE.md), kill it and
  record as `crashed`.
- Never stop the loop. Do not ask the human if you should continue.

## Evaluation variance

If metrics are noisy, run the evaluator multiple times and report the mean.
Note runs and range in `--summary`. See `POLYRESEARCH.md` "Evaluation variance"
for thresholds and acceptance rules.

## Deeper reading

- Protocol: `POLYRESEARCH.md` in the repo root
- Research playbook: `PROGRAM.md` -- goal, editable surface, constraints, strategy
- Evaluation setup: `PREPARE.md` -- how to run, parse metrics, ground truth
- Experiment history: `results.tsv` -- every past attempt, what worked and failed
- CLI help: `polyresearch <command> --help` for flags and usage
