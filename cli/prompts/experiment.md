You are running a single experiment for a polyresearch thesis. Your working directory is a git worktree checked out to the thesis branch.

Read these files first:
1. PROGRAM.md — the research playbook: goal, editable surface, strategy, constraints.
2. PREPARE.md — the evaluation setup: how to run the benchmark and parse the metric.
3. .polyresearch/thesis.md — the thesis idea and prior attempt history. Do not repeat approaches that already failed.

## Rules

**Metric measurement.** Use ONLY the evaluation command defined in PREPARE.md to measure your metric. Do not write your own benchmarks. Do not measure with different units or methods. The metric from PREPARE.md is the only number that counts.

**Editable surface.** Only modify files listed under "What you CAN modify" in PROGRAM.md. Do not modify PROGRAM.md, PREPARE.md, or anything under .polyresearch/.

**Leave changes in place.** Never revert, undo, or clean up your changes, even if the result is underwhelming. The CLI handles commits and decisions independently. Your job is to leave the best version of your code in place.

## Experiment loop

1. Study the thesis idea and prior attempts. Pick an approach that differs from what was already tried.
2. Implement the change. Favor simplicity. A small improvement with clean code beats a large improvement with ugly complexity.
3. If PREPARE.md lists a prereq_command (like `npm run build`), run it before the benchmark.
4. Run the evaluation exactly as PREPARE.md specifies. Capture the output.
5. Parse the metric from the output as PREPARE.md describes.
6. If the evaluation crashes, read the error. If it is a trivial bug you introduced (typo, missing import, wrong variable name), fix it and re-run. If the thesis idea is fundamentally broken, stop and report crashed.
7. If the metric did not improve, you may try a different angle on the same thesis. Do not try more than 3 approaches total.
8. When you are done — whether you found an improvement or not — write your result.

## Result format

Write `.polyresearch/result.json` with these fields:

```json
{
  "metric": 0.95,
  "baseline": 0.90,
  "observation": "improved",
  "summary": "Replaced linear scan with hash lookup in hot path"
}
```

- **metric** (f64): the number you measured from the PREPARE.md evaluation.
- **baseline** (f64): the metric before your changes, measured with the same evaluation.
- **observation**: one of `improved`, `no_improvement`, `crashed`, `infra_failure`.
- **summary** (string): one sentence describing what you did and what happened.
