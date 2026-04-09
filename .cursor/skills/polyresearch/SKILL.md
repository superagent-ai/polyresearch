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
3. Run `polyresearch init` if `.polyresearch-node` does not exist.
4. If `.polyresearch/` exists, run its setup. Otherwise follow `PREPARE.md`.
5. Check your GitHub identity: `gh api user --jq '.login'`
6. Identify your role from your instructions or `PROGRAM.md`:
   - If told "you are the lead," follow the lead loop.
   - Otherwise, follow the contributor loop.
7. If the repo is a fork and issues are disabled:
   `gh api repos/{owner}/{name} --method PATCH -f has_issues=true`
8. For any CLI command details: `polyresearch <command> --help`

## Core principle

GitHub visibility first. All work must be visible on GitHub. Local-only work is
invisible to other contributors, the lead, and the maintainer.

The `polyresearch duties` command enforces this. The CLI gates `claim` and
`generate` on it: those commands refuse to proceed if blocking duties exist.

## The contributor loop

```
LOOP FOREVER:

  0. polyresearch duties
     If BLOCKING items exist, resolve each one before continuing.

  1. polyresearch status
     Look for approved, unclaimed theses.

  2. If a claimable thesis exists:
     a. polyresearch claim <issue>
     b. Read PROGRAM.md for direction and constraints.
     c. For each experiment:
        - Make changes within the editable surface (PROGRAM.md CAN list).
        - Run evaluator per PREPARE.md: <run-command> > run.log 2>&1
        - Parse the metric per PREPARE.md.
        - IMMEDIATELY post:
          polyresearch attempt <issue> --metric <val> --baseline <val> \
            --observation <obs> --summary "<summary>"
        - polyresearch duties
     d. If observation was improved:
        polyresearch submit <issue>
        Do this NOW. Do not keep tinkering.
     e. If no improvement after exhausting ideas:
        polyresearch release <issue> --reason <reason>

  3. Check for review work (PRs with policy-pass, no decision,
     not authored by you):
     a. polyresearch review-claim <pr>
     b. Evaluate candidate SHA and base SHA per PREPARE.md.
     c. polyresearch review <pr> --metric <candidate> --baseline <base> \
          --observation <obs>

  4. Repeat from step 0.
```

## The lead loop

The lead runs contributor duties PLUS these, in strict priority order.

```
Each iteration, before any experiments:

  0. polyresearch duties
     Resolve ALL blocking items. Lead blocking items include:
     - Decidable PRs without decisions
     - Open PRs without policy-check
     - Stale results.tsv

  1. polyresearch sync          # on main branch, always first
  2. polyresearch audit         # check for inconsistencies

  3. Process open PRs:
     - For each PR without policy-check:
       polyresearch policy-check <pr>
     - For each PR with enough reviews and no decision:
       polyresearch decide <pr>

  4. Check queue depth:
     - If below min_queue_depth:
       polyresearch generate --title "<title>" --body "<body>"
     - Read results.tsv and all thesis history before generating.
     - Deduplicate against existing open and closed theses.

  5. Now proceed with contributor loop (experiments, etc.)

  Between experiment batches, re-run steps 0-4.
```

## Maximizing resource utilization

Do not leave compute idle while doing GitHub duties.

**Interleave duties with running experiments.** If an evaluation takes 30+
minutes, launch it in the background. While it runs, execute the full duty
cycle: `duties` > `sync` > process PRs > check queue > `generate`. Return to
check experiment progress after.

**Parallel evaluations.** API latency per call (5-10s) is the bottleneck, not
rate limits. When the evaluator supports it, run multiple evaluations
concurrently. Each process runs sequential API calls; parallelism comes from
running N processes.

**Parallel experiment setup.** When running N experiments in parallel:

1. Create N working directories. Copy evaluator files; do not symlink
   (relative paths break).
2. Write the prompt/config variant into each directory.
3. Launch all N as background processes.
4. As each completes, IMMEDIATELY post `polyresearch attempt`.
   Do not wait for the batch.
5. Between batches, run `polyresearch duties` to handle accumulated
   obligations.

**Never serialize what can overlap.** If you are waiting on I/O (API calls,
evaluation runs), do GitHub work. If GitHub is current and no duties are
blocking, start more experiments.

## Critical rules

- Post `polyresearch attempt` after EVERY experiment. Never batch.
- Run `polyresearch submit` immediately when you observe `improved`.
  Do not "keep trying."
- Lead: process the PR backlog before starting new experiments. Period.
- Lead: re-run `polyresearch sync` after any decision that closes a thesis.
- Never modify files in `.polyresearch/` or files outside the editable surface
  in `PROGRAM.md`.
- Observations use snake_case: `improved`, `no_improvement`, `crashed`,
  `infra_failure`.
- If an experiment exceeds 2x the expected time (per PREPARE.md), kill it and
  record as `crashed`.
- Never stop the loop. Do not ask the human if you should continue.

## Evaluation variance

If metrics are noisy (variance > 10% of `metric_tolerance`), run the evaluator
multiple times and report the mean.

Note the number of runs and range in the `--summary` field, e.g.:
`--summary "3 runs, range 0.932-0.948, mean 0.940, <description>"`

A candidate that consistently scores higher across 3+ runs can be considered
`improved` even if no single run exceeds tolerance.

## Deeper reading

- Protocol: `POLYRESEARCH.md` in the repo root
- Research playbook: `PROGRAM.md` -- goal, editable surface, constraints, strategy
- Evaluation setup: `PREPARE.md` -- how to run, parse metrics, ground truth
- Experiment history: `results.tsv` -- every past attempt, what worked and failed
- CLI help: `polyresearch <command> --help` for flags and usage
