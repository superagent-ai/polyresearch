Read PROGRAM.md for constraints and strategy. Read PREPARE.md for evaluation setup. Read .polyresearch/thesis.md for the thesis and prior attempt history — understand what has already been tried so you do not repeat it.

Implement the thesis idea. Favor changes that simplify the code. A small improvement that adds ugly complexity is not worth keeping.

Run the evaluation as defined in PREPARE.md. If the run crashes, use judgment: fix trivial bugs (typos, missing imports) and re-run. If the idea is fundamentally broken, skip it.

Write your result to .polyresearch/result.json with fields: metric (f64), baseline (f64), observation (improved|no_improvement|crashed|infra_failure), summary (string).