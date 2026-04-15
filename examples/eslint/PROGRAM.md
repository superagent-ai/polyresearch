# Research program

This is the research playbook. It tells agents what to optimize, what they can touch, and what constraints to respect. Read this before every experiment.

The maintainer writes and edits this file. When the research direction shifts, update this file. Contributors pick up the change on their next session start.

required_confirmations: 0
metric_tolerance: 5
metric_direction: lower_is_better
lead_github_login: replace-me
maintainer_github_login: replace-me
auto_approve: true
assignment_timeout: 24h
review_timeout: 12h
min_queue_depth: 5
max_queue_depth: 10

## Goal

Reduce ESLint's wall-clock linting time on **two workloads simultaneously**:

- **Workload A (single large file):** `Linter.verify()` on `tests/bench/large.js` (19 500 lines) with the `@eslint/js` recommended ruleset (57 rules).
- **Workload B (multi-file project):** ESLint linting its own `lib/` directory (388 files, 98 000 lines) with its full project config (280 rules across 7 plugin groups: core, unicorn, jsdoc, regexp, n, @eslint-community, internal-rules).

The benchmark reports a **composite metric**: `METRIC = METRIC_A + METRIC_B / 25`. Both sub-metrics are median wall-clock milliseconds. Lower is better. The baseline composite on `main` is approximately **350 ms**.

An improvement must reduce the composite by at least **5 ms**. Neither sub-metric may regress by more than **5 ms** compared to its baseline.

Secondary constraint: correctness. Workload A must produce 63 messages with fingerprint `4944605e99e3`. Workload B must produce 0 errors and 0 warnings on 388 files.

## What you CAN modify

- `lib/**/*.js` - all JavaScript source files under `lib/`, including the core linter, AST traversal, rule implementations, config loading, scope analysis integration, selector matching, source code representation, and shared utilities.

## What you CANNOT modify

- `.polyresearch/` - the reproducible environment
- `POLYRESEARCH.md` - the coordination protocol
- `PROGRAM.md` - the research playbook
- `PREPARE.md` - the evaluation setup
- `results.tsv` - maintained by the lead on `main`
- `tests/` - the test suite
- `packages/` - published sub-packages
- `docs/` - documentation
- `tools/` - build scripts
- `conf/` - configuration defaults
- `bin/` - CLI entry point
- `templates/` - release templates
- `messages/` - localized messages
- `node_modules/` - dependencies
- `package.json` - dependency manifest
- `package-lock.json` - lockfile
- `.github/` - CI and GitHub config
- `Makefile.js` - build system
- `eslint.config.js` - the project's own ESLint config (used by Workload B)

## Constraints

1. **Correctness is non-negotiable.** The benchmark verifies Workload A via a SHA-256 fingerprint (must be `4944605e99e3`, 63 messages). Workload B must produce 0 errors and 0 warnings across all 388 files. If either check fails, the experiment is rejected.
2. **No new dependencies.** Do not add, remove, or upgrade entries in `package.json`. Optimize using the existing dependency set.
3. **Tests must pass.** Run `npm test` to verify. A change that breaks existing tests is rejected regardless of performance gain.
4. **No caching tricks.** Both workloads run with caching disabled. File-level caching, memoization across `verify()` calls, or other stateful shortcuts are not valid.
5. **Both workloads matter.** An optimization that helps Workload A but regresses Workload B (or vice versa) by more than 5 ms will not be accepted. The composite metric captures this, but reviewers also check sub-metrics independently.
6. **Expected run time.** A single benchmark invocation (`.polyresearch/bench.js`) takes approximately 30-60 seconds. Kill and record as `crashed` if it exceeds 180 seconds.

## Strategy

The hot path for `Linter.verify()` on a single file runs through:

1. **Parsing** (`lib/languages/js/index.js`) -- espree parses source into AST.
2. **Scope analysis** -- `eslint-scope` builds the scope tree.
3. **SourceCode construction** (`lib/languages/js/source-code/source-code.js`) -- merges tokens, comments, builds lookup structures.
4. **Rule setup** (`lib/linter/linter.js` `runRules()`) -- iterates configured rules, creates listeners.
5. **AST traversal and selector matching** (`lib/linter/source-code-traverser.js`, `lib/linter/esquery.js`) -- walks the tree, matches CSS-like selectors to nodes, dispatches rule listeners.
6. **Disable directive processing** (`lib/linter/apply-disable-directives.js`).

For multi-file linting (Workload B), per-file overhead is repeated 388 times: config resolution, parser instantiation, SourceCode construction, and rule setup. Optimizations that reduce per-file setup cost compound across all files.

### Known findings from prior experiments

Read `results.tsv` carefully before starting. Key empirical findings:

**What worked (accepted patches):**

- Pre-merging selector lists by node type in ESQueryHelper constructor (~5.5 ms).
- `Number.isSafeInteger()` early return in `no-loss-of-precision` rule (~29 ms).
- Combined CPA bypass, positional emit args, and dispatch fast paths (~33 ms).
- Skip eager `createIndexMap` for large token arrays with lazy `sortedMerge` (~4.6 ms).

**What regressed on multi-file (Workload B) -- avoid these patterns:**

- Pre-allocating large typed arrays per file. A 200K-entry `Uint8Array` costs more to allocate than it saves for files under ~1000 lines.
- Removing `createIndexMap` entirely. The hash map gives O(1) token lookups that multi-file configs with 280 rules depend on.
- Polymorphic Map values (Function|Array). Causes V8 megamorphic IC deoptimization.

**What has no remaining room for improvement:**

- Code path analysis: 11.4 ms total across 90K nodes, no single optimization reaches 5 ms.
- Scope analysis: 1.73 ms total with near-100% WeakMap cache hit rate.
- Config normalization: under 1 ms for all rules.
- Parser overhead: ~67 ms in `node_modules`, outside the editable surface.
- Individual rules after `no-loss-of-precision`: no remaining rule costs >= 10 ms.
- Selector dispatch after compiled fast-checks: only ~4K calls remain at ~0.2 ms total.

### Promising directions

- Lazy initialization patterns that skip work for small files but do it for large files.
- Reducing per-file setup cost in `runRules()`, ESQueryHelper construction, or SourceCode construction -- compounds 388x in Workload B.
- Adaptive allocation -- size data structures based on file size or AST node count.
- Rule listener registration caching across files that share the same config.
