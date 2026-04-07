# Polyresearch protocol

This is the coordination protocol for polyresearch. It defines how agents find work, run experiments, submit candidates, and verify each other's results using `gh` and `git`. All coordination happens through GitHub Issues, PRs, and structured comments. No external services.

Drop this file into your repository. Fill in the configuration section below with your project's values. Do not modify the protocol sections.

**Companion files.** This file works alongside two project-specific files and an optional directory:

- **PROGRAM.md** — the research playbook. Describes the research goal, which files you can edit (gitignore-style patterns), strategy, and constraints. Read this before every experiment.
- **PREPARE.md** — the evaluation setup. Describes how to set up, run experiments, parse the metric, and what the ground truth is. This is the trust boundary. Do not modify anything it references.
- **results.tsv** — the lab notebook. Every experiment ever run, including failures. Maintained by the lead on `main`.
- **.polyresearch/** — the reproducible environment. When present, use it for setup and execution. Do not modify it.

---

## Configuration

Fill in these values for your project. Agents read them to govern coordination.

| Parameter                | Value              |
| ------------------------ | ------------------ |
| `required_confirmations` | `0`                |
| `metric_tolerance`       | `0.01`             |
| `metric_direction`       | `higher_is_better` |
| `assignment_timeout`     | `24h`              |
| `review_timeout`         | `12h`              |

---

## Roles

**Maintainer.** The human who owns the project. Writes PROGRAM.md and PREPARE.md. Approves theses. Picks the tooling (agent, model, sandbox). Polyresearch does not mandate any specific tooling.

**Contributor.** A machine running an agent. Claims theses, runs experiments, submits candidates, and reviews others' work. You are a contributor unless told otherwise.

**Lead.** A contributor with additional responsibilities: generates theses from results history, runs policy checks on candidate PRs, decides PRs (merge or close), and maintains results.tsv as sole writer. One per project. If your instructions say "you are the lead," follow the lead sections in addition to the contributor sections.

---

## Starting a session

When you start, before doing anything else:

1. Read POLYRESEARCH.md (this file), PROGRAM.md, and PREPARE.md.
2. Read results.tsv to understand experiment history and avoid repeating dead ends.
3. Run `git log --oneline -20` on `main` to see recent state.
4. If `.polyresearch/` exists, run its setup. Otherwise follow PREPARE.md setup instructions.
5. Generate your node identifier if you don't have one. Use your machine's hostname or a short ID (e.g. `node-7f83`). Keep it consistent across sessions.
6. Identify your role. If your instructions say "you are the lead," follow the lead loop. Otherwise, follow the contributor loop.

---

## The contributor loop

LOOP FOREVER:

1. **Check for theses.** Run `gh issue list --label thesis --state open`. For each issue, read comments to determine state (see [Deriving state](#deriving-state)). Look for theses that are **approved and unclaimed**.
2. **If a claimable thesis exists:**
  a. Post a `polyresearch:claim` comment on the issue.
   b. Create a branch from `main`: `thesis/<issue-number>-<slug>`.
   c. Read PROGRAM.md for direction and constraints.
   d. Run experiments. Each attempt on its own sub-branch: `thesis/<issue-number>-<slug>-attempt-<n>`.
   e. For each attempt:
      - Make your changes within the editable surface defined in PROGRAM.md.
      - Commit your changes.
      - Run the experiment per PREPARE.md. Redirect output: `<run-command> > run.log 2>&1`.
      - Parse the metric per PREPARE.md.
      - Post a `polyresearch:attempt` comment on the thesis issue with the metric, baseline, observation, and summary.
   f. **If you find an improvement:** push the best attempt's sub-branch and open a candidate PR from it to `main`. Reference the thesis issue in the PR body.
   g. **If no improvement after exhausting ideas:** post a `polyresearch:release` comment. The thesis returns to the queue for another contributor.
3. **Check for review work.** Run `gh pr list`. For each PR, read comments. Look for PRs with a `polyresearch:policy-pass` comment and **no** `polyresearch:decision` comment. These need peer review. Skip PRs you authored.
4. **If a reviewable PR exists:**
  a. Post a `polyresearch:review-claim` comment on the PR.
   b. Check out the **candidate SHA** (the PR head).
   c. Run the evaluation per PREPARE.md. Record the candidate metric.
   d. Check out the **base SHA** (the PR's merge base on `main`).
   e. Run the same evaluation. Record the baseline metric.
   f. If `.polyresearch/` exists, compute its content hash: `find .polyresearch/ -type f | sort | xargs sha256sum | sha256sum`.
   g. Post a `polyresearch:review` comment with all fields. Your `baseline_metric` is your own measurement. Do not copy it from results.tsv or the candidate's self-report.
5. Repeat from step 1.

If there are no theses to claim and no PRs to review, wait briefly and check again.

**NEVER STOP.** Once the loop has begun, do not pause to ask the human if you should continue. Do not ask "should I keep going?" or "is this a good stopping point?" The human might be asleep or away and expects you to work indefinitely until manually stopped. If you run out of ideas during experimentation, think harder - re-read PROGRAM.md, study results.tsv for patterns in what worked and failed, try combining previous near-misses, try more radical changes. The loop runs until the human interrupts you.

**Crashes.** If an experiment crashes (OOM, bug, timeout), use your judgment. If it's something simple (typo, missing import), fix it and re-run. If the idea is fundamentally broken, log it as `crashed` in the attempt comment and move on.

**Timeouts.** If a run exceeds twice the expected time budget (per PREPARE.md), kill it and treat it as a crash.

---

## The lead loop

Everything in the contributor loop, plus these additional responsibilities. Run them as part of the same loop.

### Generate theses

Read results.tsv and PROGRAM.md. Identify patterns: what worked, what failed, what hasn't been tried. Spot trends ("all learning rate increases above 0.06 regressed," "architectural changes yielded more than hyperparameter tuning"). Open new GitHub Issues with the `thesis` label. Auto-approve them by posting a `polyresearch:approval` comment.

Guard against path dependence. If recent accepted theses share the same approach, generate at least one thesis that tries a fundamentally different direction from the current baseline.

### Policy check

When a candidate PR is opened, diff it against the editable surface in PROGRAM.md. Check every file the PR touches - it must match a pattern in the CAN list. If any file is outside the editable surface, post a `polyresearch:decision` with `outcome: policy_rejection` and close the PR. No evaluation runs.

If the candidate passes, post a `polyresearch:policy-pass` comment. The PR is now eligible for peer review.

### Decide PRs

When `required_confirmations` review records have been posted on a PR, evaluate the results:

- All reviewers observed `improved` and their metrics agree within `metric_tolerance`: post `outcome: accepted`, **merge** the PR, close the thesis issue.
- All reviewers observed `no_improvement` and agree: post `outcome: non_improvement`, **close** the PR, close the thesis issue.
- Reviewer metrics diverge beyond `metric_tolerance`: post `outcome: disagreement`, **close** the PR, close the thesis issue.
- The `base_sha` in any review record does not match current `main` HEAD: post `outcome: stale`, **close** the PR. The thesis returns to the queue.
- All or most reviewers reported `crashed` or `infra_failure`: post `outcome: infra_failure`, **close** the PR. The thesis returns to the queue.

If `required_confirmations` is `0`, skip peer review entirely. Decide based on the candidate's self-reported metric and the policy check alone.

### Maintain results.tsv

You are the sole writer. Contributors never edit this file directly. They report metrics through structured comments. You transcribe them into the canonical log.

After any thesis resolution, append rows to results.tsv on `main` for every attempt logged on the thesis:

| Event                                 | Data source                                              | Action                                               |
| ------------------------------------- | -------------------------------------------------------- | ---------------------------------------------------- |
| PR merged (`accepted`)                | `polyresearch:review` records on the PR                  | Append row with verified metric                      |
| PR closed (any non-accepted outcome)  | `polyresearch:review` records + `polyresearch:decision`  | Append row with observed metric and decision outcome |
| Attempt discarded (never became a PR) | `polyresearch:attempt` comments on thesis issue          | Append row with self-reported metric                 |
| Thesis closed without any candidate   | `polyresearch:release` + `polyresearch:attempt` comments | Append rows for all logged attempts                  |

---

## Thesis lifecycle

A thesis is a GitHub Issue with the `thesis` label. Its state is not stored anywhere. It is derived from the comments on the issue and any associated PR.

```
Submitted -> Approved -> Claimed -> Experimenting -+-> CandidateSubmitted -> InReview -+-> Merged
     |                                           |                                  +-> ClosedNoImprovement
     |                                           |                                  +-> ClosedDisagreement
     |                                           |                                  +-> ClosedStale
     |                                           |
     |                                           +-> Released (returns to Approved)
     |                                           +-> TimedOut (returns to Approved)
     |
     +-> Rejected (maintainer closes issue)
```

### Deriving state

Scan the comment trail on the issue to reconstruct the current state:

- Issue exists with `thesis` label -> **Submitted**
- Has a `/approve` comment or a `polyresearch:approval` comment -> **Approved**
- Has a `polyresearch:claim` with no subsequent `polyresearch:release` for the same node -> **Claimed**
- Claimed and has an open PR from a thesis branch -> **CandidateSubmitted**
- PR has a `polyresearch:policy-pass` comment -> **InReview**
- PR has a `polyresearch:decision` comment -> **Resolved** (check `outcome` for terminal state)

No mutable labels to get out of sync. The comment trail is the truth.

---

## Branching

```
main
  └── thesis/12-rmsnorm                    (thesis branch)
        ├── thesis/12-rmsnorm-attempt-1    (discarded, unmerged)
        ├── thesis/12-rmsnorm-attempt-2    (discarded, unmerged)
        └── thesis/12-rmsnorm-attempt-3    (candidate PR -> merged to main)
```

- `main` is the accepted ledger. Only verified improvements land here.
- Each thesis gets a branch: `thesis/<issue-number>-<slug>`.
- Each attempt gets its own sub-branch: `thesis/<issue-number>-<slug>-attempt-<n>`, forked from the thesis branch.
- The candidate PR merges the best attempt's sub-branch into `main`.
- Discarded attempts stay as unmerged branches. They are data, not waste.

---

## Structured comments

All protocol state transitions happen through structured HTML comments on GitHub Issues and PRs. Comments are append-only, attributed, and auditable.

One label remains: `thesis` on issues, for discovery via `gh issue list --label thesis`. Everything else is a structured comment.

### Format

```
<!-- polyresearch:<type>
key: value
key: value
-->
```

HTML comments are hidden in GitHub's rendered view but visible in raw source and accessible via `gh api`. Agents parse them from comment bodies.

### On thesis issues

**Approval** (maintainer, plain-text slash command):

```
/approve
```

The maintainer comments `/approve` on the issue. Agents match on the exact string `/approve` at the start of a comment body.

**Approval** (lead auto-approval):

```
<!-- polyresearch:approval
thesis: 12
-->
```

Both forms are valid approval signals. The protocol recognizes either.

**Claim** (contributor claims a thesis):

```
<!-- polyresearch:claim
thesis: 12
node: node-7f83
-->
```

**Release** (contributor releases a claim without submitting a candidate):

```
<!-- polyresearch:release
thesis: 12
node: node-7f83
reason: no_improvement | timeout | infra_failure
-->
```

**Attempt** (contributor records a completed experiment):

```
<!-- polyresearch:attempt
thesis: 12
branch: thesis/12-rmsnorm-attempt-1
metric: 1.0050
baseline_metric: 0.9934
observation: no_improvement | crashed | infra_failure
summary: Switched to GeLU activation, regression on val_bpb
-->
```

### On candidate PRs

**Policy pass** (lead confirms candidate is within editable surface):

```
<!-- polyresearch:policy-pass
thesis: 12
candidate_sha: a1b2c3d
-->
```

**Review claim** (reviewer signals they are starting evaluation):

```
<!-- polyresearch:review-claim
thesis: 12
node: node-3e91
-->
```

**Review record** (reviewer posts evaluation results):

```
<!-- polyresearch:review
thesis: 12
candidate_sha: a1b2c3d
base_sha: c0d1e2f
node: node-3e91
metric: 0.9934
baseline_metric: 0.9979
observation: improved | no_improvement | crashed | infra_failure
env_sha: 9f3a2b4c | none
timestamp: 2026-04-06T14:30:00Z
-->
```

The `baseline_metric` is your own measurement of the base SHA, not a number copied from results.tsv or the candidate. You run the evaluation twice: once on the candidate, once on the base. Each review is self-contained.

The `env_sha` is the hash of `.polyresearch/` contents, or `none` if the directory does not exist. If two reviewers report different `env_sha` values, their metrics are not comparable.

**Decision** (lead resolves the PR):

```
<!-- polyresearch:decision
thesis: 12
candidate_sha: a1b2c3d
outcome: accepted | non_improvement | disagreement | stale | policy_rejection | infra_failure
confirmations: 2
-->
```

---

## Peer review

When `required_confirmations` is greater than 0, candidate PRs go through peer review. The sequence:

1. **Policy check.** The lead diffs the candidate against the editable surface. If it touches files outside the CAN list: `outcome: policy_rejection`, PR closed. Otherwise: `polyresearch:policy-pass` posted.
2. **Review claiming.** Contributors (who did not author the PR) find PRs with `polyresearch:policy-pass` and no `polyresearch:decision`. They post `polyresearch:review-claim`.
3. **Evaluation.** The reviewer checks out the candidate SHA, runs the evaluation per PREPARE.md. Then checks out the base SHA, runs the same evaluation. Both metrics are measured independently.
4. **Review record.** The reviewer posts a `polyresearch:review` comment with both metrics, observation, environment hash, and timestamp.
5. **Decision.** When `required_confirmations` review records are posted, the lead evaluates and posts `polyresearch:decision`. Merges or closes the PR.

---

## Observations and outcomes

### What reviewers report

The `observation` field in `polyresearch:review` and `polyresearch:attempt` comments.

| Observation      | Meaning                                                   |
| ---------------- | --------------------------------------------------------- |
| `improved`       | Candidate metric beats baseline beyond `metric_tolerance` |
| `no_improvement` | Candidate metric does not beat baseline                   |
| `crashed`        | Evaluation failed to complete (OOM, bug, timeout)         |
| `infra_failure`  | Environment setup failed, could not run evaluation        |

A reviewer reports what they saw. Nothing more.

### What the lead decides

The `outcome` field in the `polyresearch:decision` comment. One per PR.

| Outcome            | Condition                                          | Action                            |
| ------------------ | -------------------------------------------------- | --------------------------------- |
| `accepted`         | Reviewers observed `improved`, metrics agree       | Merge PR, close issue             |
| `non_improvement`  | Reviewers observed `no_improvement`, metrics agree | Close PR, close issue             |
| `disagreement`     | Reviewer metrics diverge beyond `metric_tolerance` | Close PR, close issue             |
| `stale`            | Base SHA moved, candidate no longer comparable     | Close PR, thesis returns to queue |
| `policy_rejection` | Candidate touched files outside editable surface   | Close PR, close issue             |
| `infra_failure`    | Reviewers could not evaluate reliably              | Close PR, thesis returns to queue |

On `stale` and `infra_failure`, the thesis is not permanently closed. It returns to Approved because the failure was not about the hypothesis.

---

## results.tsv

Tab-separated. One header row and six columns.

```
thesis	attempt	metric	baseline	status	summary
```

| Column     | Description                                                                                                            |
| ---------- | ---------------------------------------------------------------------------------------------------------------------- |
| `thesis`   | Issue reference, e.g. `#12`                                                                                            |
| `attempt`  | Branch name, e.g. `thesis/12-rmsnorm-attempt-1`                                                                        |
| `metric`   | Measured value, or `—` if crashed                                                                                      |
| `baseline` | Metric on `main` at the time of the attempt                                                                            |
| `status`   | `accepted`, `discarded`, `crashed`, `non_improvement`, `disagreement`, `stale`, `infra_failure`, or `policy_rejection` |
| `summary`  | One-line description of what the experiment tried                                                                      |

Example:

```
thesis	attempt	metric	baseline	status	summary
#12	thesis/12-rmsnorm-attempt-1	0.9934	0.9979	accepted	RMSNorm instead of LayerNorm
#12	thesis/12-rmsnorm-attempt-2	0.9980	0.9979	discarded	RMSNorm with different init (no improvement)
#13	thesis/13-gelu-attempt-1	1.0050	0.9934	discarded	Switch to GeLU (regression)
#14	thesis/14-double-width-attempt-1	—	0.9934	crashed	Double model width (OOM)
```

The lead is the sole writer. A single writer eliminates merge conflicts and keeps the log consistent with decision outcomes.

Failed experiments are data. Every attempt gets a row: accepted, discarded, and crashed. The full history feeds thesis generation and prevents repeating dead ends.
