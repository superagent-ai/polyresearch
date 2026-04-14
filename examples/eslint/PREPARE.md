# Evaluation

This is the evaluation setup. It tells agents and reviewers how to set up, run experiments, and measure results. Both experimenters and reviewers follow the same instructions.

This file is the trust boundary. The evaluation code it references is outside the editable surface. Agents cannot change how they are judged.

The maintainer writes this file. It rarely changes.

## Setup

One-time setup from the repository root:

```bash
npm install
```

Verify the benchmark runs correctly:

```bash
node .polyresearch/bench.js
```

You should see output with `METRIC_A:`, `METRIC_B:`, and `METRIC:` lines. The composite `METRIC` should be around 350.

## Running an experiment

From the worktree root (which contains your modified `lib/` files):

```bash
node .polyresearch/bench.js > run.log 2>&1
```

The benchmark runs two workloads:

### Workload A: Single large file

1. Loads `tests/bench/large.js` (19 500 lines of JavaScript).
2. Creates a `Linter` instance and configures it with `@eslint/js` recommended rules (57 rules).
3. Runs 3 warmup iterations (discarded).
4. Runs 10 timed iterations and reports the **median** time in milliseconds as `METRIC_A`.
5. Computes a fingerprint of the lint messages to verify correctness.

### Workload B: Multi-file project lint

1. Creates an `ESLint` instance pointed at the repository root with caching disabled.
2. Lints the entire `lib/` directory (388 files, 280 rules including unicorn, jsdoc, regexp, n, and internal plugins).
3. Runs 1 warmup iteration (discarded).
4. Runs 5 timed iterations and reports the **median** time in milliseconds as `METRIC_B`.
5. Verifies 0 errors and 0 warnings across all files.

### Composite metric

The primary metric is: `METRIC = METRIC_A + METRIC_B / 25`

This puts both workloads on a comparable scale (~175 + ~170 ≈ 350 baseline). Both must improve (or at least not regress) to bring the composite down.

## Output format

A successful run prints this structure:

```
METRIC_A: 182.32
METRIC_A_MIN: 176.99
METRIC_A_MAX: 266.24
MESSAGES: 63
FINGERPRINT: 4944605e99e3

METRIC_B: 4238.38
METRIC_B_MIN: 4223.36
METRIC_B_MAX: 4581.17
FILES: 388
ERRORS: 0
WARNINGS: 0

METRIC: 351.86
```

- `METRIC_A` is the median single-file lint time in milliseconds.
- `METRIC_B` is the median multi-file lint time in milliseconds.
- `METRIC` is the composite (primary metric for acceptance).
- `MESSAGES` must remain 63. `FINGERPRINT` must remain `4944605e99e3`.
- `FILES` must remain 388. `ERRORS` must remain 0. `WARNINGS` must remain 0.

If any correctness check fails, the experiment is rejected regardless of speed improvement.

## Parsing the metric

```bash
grep '^METRIC:' run.log | awk '{print $2}'
```

This produces a single number on stdout: the composite metric.

To inspect sub-metrics:

```bash
grep '^METRIC_A:' run.log | awk '{print $2}'
grep '^METRIC_B:' run.log | awk '{print $2}'
```

## Ground truth

**Workload A:** Median wall-clock time of `Linter.verify()` over 10 runs on a fixed 19 500-line JavaScript file with 57 recommended rules. Correctness enforced by fingerprinting `(ruleId, line, column, severity)` tuples.

**Workload B:** Median wall-clock time of `ESLint.lintFiles(["lib/"])` over 5 runs on the project's own 388-file `lib/` directory with the project's full 280-rule config. Correctness enforced by verifying 0 errors and 0 warnings.

**Composite:** `METRIC_A + METRIC_B / 25`. Both workloads use `process.hrtime.bigint()` for nanosecond-resolution timing. The `/25` scaling factor puts both sub-metrics on a roughly equal footing (~175 ms each in the composite).

The evaluation cannot be gamed by modifying the benchmark, the test files, or the ESLint config -- all are outside the editable surface.

## Environment

- **Runtime:** Node.js v22+.
- **Hardware:** Results are relative. The benchmark uses medians to reduce noise. Improvements of less than 5 ms on the composite may be within measurement variance and will not be accepted.
- **Expected wall time:** A full benchmark run takes approximately 30-60 seconds.
- **Kill threshold:** If a run exceeds 180 seconds, kill it and record as `crashed`.
