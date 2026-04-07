# Poker agent

Build a heads-up no-limit Texas Hold'em agent by writing a system prompt and optional tool implementations. The agent plays against a frozen baseline on a frozen duplicate-deal set. The editable surface is `system_prompt.md` and `tools/`.

## Why this is a good polyresearch example

**Two-dimensional search space.** Unlike the other examples where the editable surface is a single file, this one has two independent axes: the prompt and the tool suite. One contributor might focus entirely on prompt engineering -- better bet-sizing rules, clearer position awareness, tighter preflop discipline. Another might ignore the prompt and build a hand equity calculator or pot odds tool. A third might do both. These are different research directions that benefit from parallel exploration.

**Game-theoretic depth resists simple hill-climbing.** Poker is not a metric you can improve monotonically by tweaking one parameter. A strategy that exploits the baseline's weaknesses might itself be exploitable. Bet sizing, bluff frequency, and value extraction interact in ways that are hard to reason about from a prompt alone. Contributors with different intuitions about poker strategy will explore different parts of this space.

**Duplicate deals cancel luck.** Each deal is played twice with seats swapped, so the metric reflects decision quality rather than card distribution. Two reviewers evaluating the same candidate will see nearly identical results. This is what makes peer review meaningful -- you are comparing strategies, not comparing who got dealt pocket aces more often.

**The tool surface is open-ended.** The evaluator loads any `.json` + `.py` pair it finds in `tools/`. Contributors can add hand equity calculators, board texture classifiers, preflop lookup tables, pot odds helpers, or anything else that fits in pure Python under 50 KB. The constraint is lightweight but the design space is wide. Different contributors will build different toolsets and the experiment history will show which tools actually help.

## Getting started

```bash
.polyresearch/setup.sh
python .polyresearch/evaluate.py > run.log 2>&1
grep "^poker_score:" run.log
```

See [PREPARE.md](PREPARE.md) for full evaluation details and [PROGRAM.md](PROGRAM.md) for the research playbook.