# Research program

This is the research playbook. It tells agents what to optimize, what they can touch, what constraints to respect, and how to run the experiment loop. Read this before every experiment.

The maintainer writes and edits this file. When the research direction shifts, update this file. Contributors pick up the change on their next session start.

required_confirmations: 0
metric_tolerance: 0.01
metric_direction: higher_is_better
lead_github_login: replace-me
maintainer_github_login: replace-me
default_branch: replace-me
auto_approve: true
assignment_timeout: 24h
review_timeout: 12h
min_queue_depth: 5
max_queue_depth: 10
cli_version: 0.4.1

## Goal

The metric name, direction (lower/higher is better), and optimization target. Include any secondary or soft constraints.

## What you CAN modify

Files and patterns agents are allowed to edit. Gitignore-style glob patterns. Negation with `!`. Everything within this surface is fair game. Everything not listed here is off-limits.

## What you CANNOT modify

Protected files. The evaluation code, PREPARE.md, dependencies, and `.polyresearch/` should always appear here.

## Constraints

Hard and soft limits: simplicity criteria, resource budgets, time budgets, known dead ends from results history.

## Strategy

Optional. High-level research direction, promising ideas, known patterns from results history. The lead updates this section as the project evolves.

## Thesis context

Before each experiment, read `.polyresearch/thesis.md`. It contains the specific thesis you are working on and any negative knowledge from earlier attempts or related theses.

## Experiment loop

Repeat the current thesis loop until you either:

- find one clearly improved result, or
- exhaust the idea space for this thesis, or
- finish 3 serious candidate attempts after the baseline.

Then write `.polyresearch/result.json` and STOP. The CLI will decide what to do next.

1. Read `.polyresearch/thesis.md` for your current assignment.
2. Read the editable surface files for full context.
3. Read `PREPARE.md` for the benchmark command, output format, and metric parsing instructions.
4. Run the benchmark first to establish YOUR baseline on this machine.
5. Modify code only within the editable surface.
6. Run the benchmark again. Compare the result to your baseline.
7. If improved, keep the best change you found.
8. If equal or worse, revert and try a different approach.
9. Keep a short record of what you tried.
10. When done, write `.polyresearch/result.json` with your best result and a short summary of what you tried.

Do not pause to ask if you should continue while working the current thesis. This is a bounded thesis pass, not an infinite autoresearch session inside one thesis. Once `.polyresearch/result.json` is complete, stop immediately so the CLI can record the outcome.

## Result format

Write `.polyresearch/result.json` in the current worktree:

```json
{
  "metric": 0.0,
  "baseline": 0.0,
  "observation": "improved",
  "summary": "Brief description of the best change you found.",
  "attempts": [
    {
      "metric": 0.0,
      "summary": "What this attempt tried."
    }
  ]
}
```

Use `observation: improved` when the best metric beats your baseline by more than `metric_tolerance`. Use `no_improvement` when nothing beat the baseline. Use `crashed` when every serious attempt failed to run.