# Research program

This is the research playbook. It tells agents what to optimize, what they can touch, and what constraints to respect. Read this before every experiment.

The maintainer writes and edits this file. When the research direction shifts, update this file. Contributors pick up the change on their next session start.

required_confirmations: 0
metric_tolerance: 0.01
metric_direction: higher_is_better
lead_github_login: replace-me
maintainer_github_login: replace-me
auto_approve: true
assignment_timeout: 24h
review_timeout: 12h
min_queue_depth: 5
max_queue_depth: 10
cli_version: 0.4.0

## Goal

The metric name, direction (lower/higher is better), and optimization target. Include any secondary or soft constraints.

## What you CAN modify

Files and patterns agents are allowed to edit. Gitignore-style glob patterns. Negation with `!`. Everything within this surface is fair game. Everything not listed here is off-limits.

## What you CANNOT modify

Protected files. The evaluation code, PREPARE.md, POLYRESEARCH.md, dependencies, and `.polyresearch/` should always appear here.

## Constraints

Hard and soft limits: simplicity criteria, resource budgets, time budgets, known dead ends from results history.

## Strategy

Optional. High-level research direction, promising ideas, known patterns from results history. The lead updates this section as the project evolves.