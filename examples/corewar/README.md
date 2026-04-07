# Core War

Evolve a [Redcode](https://corewar.co.uk/icws94.txt) warrior that beats a frozen gauntlet of eight opponents. The editable surface is a single file: `warrior.red`.

## Why this is a good polyresearch example

**Evaluation is free and fast.** The simulator is pure Python, runs on any machine with no API keys, and finishes 800 deterministic battles in one to four minutes. Peer review costs nothing. A reviewer checks out the candidate, runs the evaluator, and gets the same number every time. This makes it easy to onboard new contributors without worrying about cost.

**The search space is large but the verification is cheap.** Redcode warriors can combine bombing, scanning, replication, imp launching, decoys, and process management in ways that interact non-obviously. A small change to a scan step or bomb pattern can flip matchups against multiple opponents. No single contributor is likely to explore every combination, and each new idea is cheap to verify.

**Rock-paper-scissors dynamics reward breadth.** Bombers beat scanners, replicators survive bombers, scanners kill replicators. The gauntlet includes all three plus imps, imp gates, and hybrids. A contributor who only thinks about bombing will plateau. Having multiple contributors with different mental models of Core War -- or different LLMs with different biases -- means more of the strategy space gets covered.

**Parallel search has a clear payoff.** The optional `evolve.py` tool runs an LLM-driven generate-and-mutate loop, evaluating every candidate against the full gauntlet. On a single machine this takes hours. The whole point is that ten machines running different seeds and mutation strategies for one hour will find better warriors than one machine running for ten hours. Polyresearch coordinates the results.

## Getting started

```bash
.polyresearch/setup.sh            # one-time setup
.venv/bin/python .polyresearch/evaluate.py > run.log 2>&1
grep "^battle_score:" run.log     # parse the metric
```

See [PREPARE.md](PREPARE.md) for full evaluation details and [PROGRAM.md](PROGRAM.md) for the research playbook.