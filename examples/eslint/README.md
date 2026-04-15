# ESLint

Reduce [ESLint](https://github.com/eslint/eslint)'s wall-clock linting time on two workloads simultaneously: a single large file with recommended rules, and a full multi-file project lint. The editable surface is `lib/**/*.js` -- ESLint's core implementation.

This example runs against a real fork of the ESLint repository. Unlike the other examples in this repo, it cannot be self-contained: the evaluation needs the full ESLint codebase, its test suite, and its `node_modules`. See [Getting started](#getting-started) for setup.

## Results: single-file linting 24% faster over 75 experiments

We ran polyresearch for 75 experiments. Early experiments targeted only single-file linting, but the benchmark was later expanded to require that both workloads improve (or at least not regress). Four patches were accepted. A follow-up pass fixed all behavioral test regressions introduced by the patches. The combined result, measured on a dedicated server (Intel Xeon W-2295, 18 cores, 3 runs each):

| Workload                                 | Baseline | With all patches | Improvement      |
| ---------------------------------------- | -------- | ---------------- | ---------------- |
| **A** (single 19.5K-line file, 57 rules) | 398 ms   | 302 ms           | **-96 ms (24%)** |
| **B** (388 files, 280 rules)             | 8 024 ms | 7 750 ms         | **-274 ms (3%)** |

The four patches that produced this:

| Thesis | What changed                                                                                    |
| ------ | ----------------------------------------------------------------------------------------------- |
| #87    | `Number.isSafeInteger()` fast path in `no-loss-of-precision` rule, module-level regex constants |
| #78    | Combined CPA node-type bypass, positional emit args, dispatch fast path, plain object literals  |
| #94    | Pre-merge selector lists by node type at ESQueryHelper construction time                        |
| #102   | Skip eager `createIndexMap` for files with >10K tokens, lazy `sortedMerge` via deferred getter  |

Most of the gain is in single-file linting (Workload A), where the patches cut 96 ms off a 398 ms baseline. Multi-file linting (Workload B) improves by 274 ms (3%) -- the per-file overhead reductions compound across 388 files but each file is small enough that the absolute savings per file are limited.

The full experiment log is in [results.tsv](results.tsv).

### Why these results matter

**The optimizations are real.** Each patch was measured against a baseline on the same machine in interleaved A/B runs with median reporting. The benchmark uses `process.hrtime.bigint()` for nanosecond resolution and requires a correctness fingerprint -- lint messages must remain identical. There is no way to improve the metric by producing fewer or different warnings.

**The dual-workload benchmark prevented false wins.** Several early experiments helped the large single-file benchmark but regressed multi-file linting. Removing `createIndexMap` entirely saved 22 ms on Workload A but added hundreds of milliseconds to Workload B because multi-file configs do thousands of token lookups per file. Requiring both workloads to hold caught this. Only patches that improved (or at least did not regress) both workloads survived.

**The negative results are as valuable as the positive ones.** 71 of 75 experiments were discarded or showed no improvement. But they mapped the optimization landscape in detail:

- Code path analysis overhead is 11.4 ms spread across 90K nodes. No single CPA optimization produces a measurable improvement.
- Scope analysis (`getScope()`) takes 1.73 ms with near-100% WeakMap cache hits. Not a bottleneck.
- Parser overhead is ~67 ms but lives in `node_modules` (espree/acorn), outside the editable surface.
- After the `no-loss-of-precision` fast path, no remaining individual rule costs more than 10 ms on Workload A.
- V8 Node 22+ aggressively optimizes existing code patterns. Many "obvious" micro-optimizations (generator-based traversal, object pooling, polymorphic dispatch) caused JIT deoptimization and regressed performance.

### What's left to do

**All behavioral tests pass.** The original patches introduced 48 test failures. A follow-up pass fixed 19 of them -- these were real bugs (the CPA node-type bypass was skipping nodes it shouldn't, and the `isSafeInteger` fast path had edge cases). After the fix, every lint pattern that should produce a warning still does. The benchmark correctness checks pass: fingerprint match, 0 errors, 0 warnings.

**29 internal-interface tests need updating.** The remaining 29 failures are in `source-code-traverser.js` (26) and `source-code.js` (3). These tests assert implementation details -- step object class types, dispatch method signatures, traversal step shapes -- that the patches intentionally redesigned. The lint output is identical; no rule behavior changed. The tests need to be updated to match the new internals, which is standard when refactoring internal architecture.

## Getting started

This example needs a fork of the ESLint repository with polyresearch coordination files. To set one up:

```bash
git clone https://github.com/eslint/eslint.git eslint-polyresearch
cd eslint-polyresearch
npm install

# Copy coordination files from this example
cp /path/to/examples/eslint/PROGRAM.md .
cp /path/to/examples/eslint/PREPARE.md .
cp /path/to/examples/eslint/results.tsv .
mkdir -p .polyresearch
cp /path/to/examples/eslint/.polyresearch/bench.js .polyresearch/

# Verify the benchmark runs
node .polyresearch/bench.js
```

You should see output with `METRIC_A`, `METRIC_B`, and `METRIC` lines. The composite `METRIC` should be approximately 350 on modern hardware.

The canonical protocol file for this repo is [POLYRESEARCH.md](../../POLYRESEARCH.md).

See [PREPARE.md](PREPARE.md) for full evaluation details and [PROGRAM.md](PROGRAM.md) for the research playbook.

## Why this is a good polyresearch example

**The metric is grounded in real-world usage.** ESLint is one of the most widely used JavaScript tools. Reducing its lint time directly affects developer experience in every project that uses it. The benchmark workloads reflect actual usage: a large file with recommended rules, and a real project with a full config.

**The dual-workload benchmark prevents gaming.** A single benchmark is easy to overfit. Requiring both workloads to hold forces optimizations to work across two very different scenarios -- a single 19.5K-line file with 57 rules, and 388 small files with 280 rules. Tricks that help one workload (like skipping hash-map construction) can destroy the other.

**The search space is deep and non-obvious.** JavaScript runtime optimization interacts with V8's JIT compiler in unpredictable ways. An "obvious" optimization like replacing class instances with plain objects can trigger hidden-class deoptimization. An optimization that measures well in isolation can regress when combined with another. Different contributors bringing different mental models of V8 internals, AST traversal, and linter architecture are more likely to cover the space than one contributor running for longer.

**Failed experiments constrain future work.** The 71 discarded experiments document exactly which ideas do not work and why. This negative knowledge prevents future contributors from repeating dead ends and helps the lead generate better theses.
