# Evaluation

## Setup

Run the setup script once. It creates a virtual environment and installs the
Python dependencies used by the evaluator.

```
.polyresearch/setup.sh
```

Requires one environment variable:

- `OPENAI_API_KEY` - used by litellm when `EVAL_MODEL` points at an OpenAI
  model such as the default `gpt-5.4-mini`

Optional environment variables:

- `EVAL_MODEL` - the model to evaluate. Default: `gpt-5.4-mini`. Accepts any
  litellm model string. The evaluator also accepts `mock-basic` for local smoke
  tests when you want to validate the full pipeline without making API calls.

The frozen duplicate-deal set is already checked in as
`.polyresearch/deals.json`. You do not need to regenerate it during normal
experiments.

## Running an experiment

```
python .polyresearch/evaluate.py > run.log 2>&1
```

Expected runtime: usually 20-35 minutes depending on API latency and model
speed. Expected cost: roughly low single-digit dollars or less per run. The
summary block includes a rough estimate based on litellm usage metadata when it
is available.

## Output format

The script prints a summary block when it finishes:

```
---
poker_score: 0.5000
avg_bb_per_deal: 0.0000
deal_win_rate: 0.5000
deals_evaluated: 200
total_hands: 400
model: gpt-5.4-mini
system_prompt_tokens: 61
tools_loaded: 0
llm_calls: 3200
cost_usd: 0.48
elapsed_seconds: 1420.7
```

Before the summary block, the evaluator prints progress lines so you can see
which deal is running and whether either side had parse or tool failures.

## Parsing the metric

```
grep "^poker_score:" run.log | awk '{print $2}'
```

## Ground truth

```
avg_bb_per_deal = total_candidate_bb_delta / deals_evaluated
poker_score = (clamp(avg_bb_per_deal, -100, 100) + 100) / 200
```

Where `total_candidate_bb_delta` is the candidate's net chip result across both
seatings for every frozen deal, converted to big blinds.

Each frozen deal is played twice:

- candidate in seat 1, baseline in seat 2
- baseline in seat 1, candidate in seat 2

That duplicate format cancels most card luck. The same hole cards and board are
reused in both seatings, so improvements should come from decisions, not from
better cards.

The evaluator, the baseline prompt embedded inside it, the deal set, and the
scoring logic are all frozen. Do not modify them.

## Environment

- Python 3.11+
- Internet access for LLM API calls
- No GPU required
- No Docker required
