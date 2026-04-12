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
7. Identify your role from your instructions or `PROGRAM.md`:
   - If told "you are the lead," follow the lead loop.
   - Otherwise, follow the contributor loop.
8. If the repo is a fork and issues are disabled:
   `gh api repos/{owner}/{name} --method PATCH -f has_issues=true`
9. For any CLI command details: `polyresearch <command> --help`

`POLYRESEARCH_NODE_ID` takes precedence over `.polyresearch-node.toml` for the current session. This is required when multiple agents share one checkout or when one GitHub login runs several workers in parallel.

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

  1. polyresearch pace
     Compare your effective resource policy against recent node throughput.
     If the policy says to push harder, increase parallelism or claim rate.
     If the policy says to stay under a hardware or API ceiling, back off.

  2. polyresearch status
     Look for approved, unclaimed theses.

  3. If a claimable thesis exists:
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

  4. Check for review work (PRs with policy-pass, no decision,
     not authored by you):
     a. polyresearch review-claim <pr>
     b. Evaluate candidate SHA and base SHA per PREPARE.md.
     c. polyresearch review <pr> --metric <candidate> --baseline <base> \
          --observation <obs>

  5. Repeat from step 0.
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

  1. polyresearch pace          # compare actual throughput vs effective policy
  2. polyresearch sync          # on main branch, always first
  3. polyresearch audit         # check for inconsistencies

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
     - If `auto_approve` is `false`, generated theses are not auto-approved.
       They stay queued for the maintainer to `/approve` or `/reject`.
     - Read results.tsv and all thesis history before generating.
     - Read maintainer `/approve` and `/reject` comments and use them as
       directional input for future thesis generation.
     - Deduplicate against existing open and closed theses.

  6. Now proceed with contributor loop (experiments, etc.)

  Between experiment batches, re-run steps 0-5.
```

## Resource pacing

The protocol has a default resource policy: maximize throughput. Never leave
claimable theses idle while experiments could be running. Run evaluations in
parallel when the evaluator supports it. Interleave duties with long-running
evaluations.

If `.polyresearch-node.toml` sets a `resource_policy`, that node-specific
policy overrides the default. Treat it as a real operating constraint.

Use `polyresearch pace` as the feedback loop:

1. Read the effective resource policy shown by `pace`.
2. Compare it against the throughput metrics for your node.
3. If the policy allows more throughput than you are getting, increase
   parallelism or claim rate.
4. If the policy imposes a ceiling (hardware, RAM, API rate limits), stay
   below it.

Do not leave resource usage on autopilot. Re-run `pace` regularly and correct
drift.

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
