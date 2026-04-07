# Evaluation

## Setup

Run the setup script once. It verifies that Python 3.9+ is available, creates a
local virtual environment, and installs Python dependencies.

```
.polyresearch/setup.sh
```

No GPU or Docker setup is required.

Optional environment variables:

- `COREWAR_WORKERS` - number of worker processes for battle simulation.
  Default: `min(cpu_count, 8)`.
- `OPENAI_API_KEY` - required only for `.polyresearch/evolve.py`, which uses OpenAI via
  `litellm`. The default model is `gpt-5.4-mini`.

## Running an experiment

```
.venv/bin/python .polyresearch/evaluate.py > run.log 2>&1
```

Expected runtime: usually 1-4 minutes depending on CPU core count. The
evaluator runs 100 battles per opponent (50 seeds x both warrior load orders)
against a frozen gauntlet of eight opponents.

## Optional automated search

You can also ask the frozen helper tool to generate and mutate warriors:

```
.venv/bin/python .polyresearch/evolve.py > evolve.log 2>&1
```

Default `.polyresearch/evolve.py` settings:

- Model: `gpt-5.4-mini`
- 10 rounds
- 8 candidates per round
- full 800-battle evaluation for every candidate

Rough LLM cost per default run: about `$0.41`.

Expected runtime on one 8-core machine: multiple hours. The dominant cost is
not the model calls - it is the repeated full battle simulation. This is
intentional. The example is meant to demonstrate why polyresearch helps:
different contributor machines can run different seeds and search trajectories
in parallel, then share results through the common protocol and experiment log.

## Output format

The script prints a summary block when it finishes:

```
---
battle_score: 0.7250
wins: 487
draws: 152
losses: 161
opponents_beaten: 6/8
battles_per_opponent: 100
total_battles: 800
warrior_length: 12
workers: 8
elapsed_seconds: 41.2
```

Before the summary block, the evaluator prints per-opponent results so you can
see where the warrior is strong or weak.

`.polyresearch/evolve.py` prints a similar final summary for the best discovered
warrior, plus the model name, round count, candidate count, token usage,
estimated cost, and elapsed time.

## Parsing the metric

```
grep "^battle_score:" run.log | awk '{print $2}'
```

## Ground truth

```
battle_score = total_points / (total_battles * 3)
```

Per battle:

- Win = 3 points
- Draw = 1 point
- Loss = 0 points

Each opponent is evaluated across 100 deterministic battles:

- 50 fixed random seeds
- candidate loaded first and second for every seed

The simulator, the parser, the gauntlet, the prompt templates, the helper
scripts, and the scoring logic all live under `.polyresearch/` and are frozen.
Do not modify them.

## Environment

- Python 3.9+
- Network access required only for `evolve.py`
- No GPU required
- No Docker required
