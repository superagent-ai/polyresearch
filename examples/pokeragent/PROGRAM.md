# Research program

## Goal

Maximize `poker_score` (higher is better). The evaluator runs a heads-up
no-limit Texas Hold'em agent on a frozen duplicate-deal set against a frozen
baseline agent using the same model.

For each deal, the evaluator plays two hands:

- candidate in seat 1 vs baseline in seat 2
- baseline in seat 1 vs candidate in seat 2

This cancels most card luck. The metric reflects decision quality more than
run-good on the deal set.

The final metric is:

`poker_score = (avg_bb_per_deal + 100) / 200`

Where `avg_bb_per_deal` is the candidate's average chip advantage per deal,
measured in big blinds and clamped to `[-100, 100]`.

- `0.50` means the candidate played roughly even with the baseline
- above `0.50` means the candidate beat the baseline
- below `0.50` means the candidate lost to the baseline

## Configuration

These are the project-specific coordination values referenced by
`POLYRESEARCH.md`.

| Parameter                | Value              |
| ------------------------ | ------------------ |
| `required_confirmations` | `0`                |
| `metric_tolerance`       | `0.01`             |
| `metric_direction`       | `higher_is_better` |
| `assignment_timeout`     | `24h`              |
| `review_timeout`         | `12h`              |
| `min_queue_depth`        | `5`                |

## What you CAN modify

- `system_prompt.md` - the poker agent's system prompt. You can change its
  strategy, reasoning style, decision rules, formatting instructions, and how
  it decides when to use tools.
- `tools/**/*.json` - tool schemas exposed to the model through litellm
  function calling.
- `tools/**/*.py` - tool implementations. Each tool must expose a
  `run(**kwargs)` entry point. The evaluator loads matching `.json` and `.py`
  files by basename.

The tools surface is intentionally open-ended. A valid thesis could add a hand
equity calculator, pot odds helper, board texture classifier, preflop lookup,
or something else entirely. Another valid thesis could ignore tools and only
improve the prompt.

## What you CANNOT modify

- `.polyresearch/` - the reproducible environment: evaluator, deal set,
  dependencies, and setup scripts
- `PREPARE.md` - the trust boundary
- `POLYRESEARCH.md` - the coordination protocol
- `results.tsv` - maintained by the lead on `main`

## Constraints

- The system prompt must be under 4,000 tokens. The evaluator enforces this.
- Tools must be pure Python. No network calls, subprocesses, or writing outside
  the `tools/` directory.
- Tools may use the standard library and the packages in
  `.polyresearch/requirements.txt`.
- Each tool needs both a schema file and a Python file with the same basename.
- Tool execution is limited to 2 seconds per call.
- The agent may make at most 3 tool calls at a single decision point.
- The total size of the `tools/` directory must stay under 50 KB.
- The model still has to return a legal poker action. Illegal or malformed
  actions fall back to a safe default in the evaluator, which usually hurts the
  score.
- Simpler is better. If two candidates score the same, prefer the shorter
  prompt and the smaller, clearer toolset.

## Strategy

The baseline is deliberately basic. It plays legal poker, but it has no real
view of hand strength, board texture, pot odds, bluff frequency, or exploit
adjustments. That leaves room for several kinds of improvements:

- Better prompt structure for preflop discipline, bet sizing, and showdown
  value extraction
- Clearer instructions about stack depth, position, and how to respond to
  aggression
- Better use of tools when the situation is close and raw intuition is weak
- Better refusal to overuse tools when the decision is obvious
- More consistent action formatting so the evaluator gets legal actions on the
  first try

Read `results.tsv` before you start. The most useful theses usually come from
patterns in earlier attempts, not from random new ideas.
