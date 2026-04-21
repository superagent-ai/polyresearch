# Polyresearch CLI v2 Specification

## What this document is

A product-level specification for the polyresearch CLI. It describes what the CLI does and the rules it must follow. It does not prescribe implementation details except in the architecture hints section at the end.

---

## What is being removed

The v2 CLI folds all coordination logic into the CLI commands (`bootstrap`, `lead`, `contribute`). This eliminates several files and concepts from the v1 system:

### POLYRESEARCH.md (618 lines, deleted)

The v1 protocol specification file. It described the full coordination protocol -- how agents find work, run experiments, submit candidates, and verify results -- as a markdown document that agents were expected to read and follow. Agents were bad at this: wrong commands, wrong order, wrong repo, bad GitHub API usage. The entire contents are now encoded as deterministic CLI behavior. The agent never needs to understand the protocol; it just reads `PROGRAM.md`, runs experiments, and writes `result.json`.

### skills/polyresearch/SKILL.md (253 lines, deleted)

The agent skill file that taught coding agents how to operate the polyresearch protocol by reading `POLYRESEARCH.md` and running CLI commands in the right order. With `bootstrap`, `lead`, and `contribute` handling all orchestration, the agent no longer needs a skill to operate polyresearch. It only needs to know how to read `PROGRAM.md` and run experiments -- which `PROGRAM.md` itself explains.

### What replaces them

- The **contributor loop** (`polyresearch contribute`) replaces the skill's contributor loop instructions. The CLI claims, sets up worktrees, spawns the agent, records results, submits PRs, and releases claims automatically.
- The **lead loop** (`polyresearch lead`) replaces the skill's lead loop instructions. The CLI syncs, policy-checks, decides, and generates theses automatically.
- The **bootstrap command** (`polyresearch bootstrap`) replaces the manual setup steps (clone, write templates, init node).
- `PROGRAM.md` remains as the agent-facing interface but now only describes the experiment loop and result format -- not the coordination protocol. The "Thesis context", "Experiment loop", and "Result format" sections are the agent's entire surface.

### What stays the same

- `PROGRAM.md` as the agent-facing config and experiment loop spec
- `PREPARE.md` as the evaluation setup
- `results.tsv` as the experiment ledger
- `.polyresearch/` as the runtime directory
- All low-level CLI commands (`claim`, `attempt`, `release`, `submit`, etc.) for manual use and debugging
- The structured GitHub comment protocol (comments on issues and PRs are still the canonical shared state)
- The node config file (`.polyresearch-node.toml`)

---

## Core concept

Polyresearch is distributed autoresearch with coordination. Autoresearch gives one agent a codebase, a metric, and a loop. Polyresearch keeps that same agent experience but wraps it in a CLI that adds shared queues, claims, review, results syncing, and multi-machine coordination.

The split of responsibilities is absolute:

- **Agent**: read PROGRAM.md, run experiments, write `.polyresearch/result.json`.
- **CLI**: everything else. Claims, submissions, releases, syncing, PR flow, review, pacing, queue management.

The agent never touches GitHub. The CLI never modifies code inside the editable surface.

---

## Repo layout

A polyresearch project is any git repo with these files:


| File                      | Owner       | Purpose                                                                                                          |
| ------------------------- | ----------- | ---------------------------------------------------------------------------------------------------------------- |
| `PROGRAM.md`              | Maintainer  | Agent-facing experiment loop, editable surface, constraints, config key-value pairs                              |
| `PREPARE.md`              | Maintainer  | Evaluation setup: benchmark command, output format, metric parsing, resource footprint                           |
| `results.tsv`             | CLI (lead)  | Experiment ledger. Every attempt ever recorded                                                                   |
| `.polyresearch/`          | CLI + Agent | Runtime directory. Contains `thesis.md` (CLI writes), `result.json` (agent writes), and optional harness scripts |
| `.polyresearch-node.toml` | CLI (init)  | Node identity, capacity, agent command. Gitignored                                                               |


### PROGRAM.md config keys

Parsed as `key: value` lines where the key is a single `snake_case` word. Non-matching lines (headings, prose, blank lines) are ignored.


| Key                       | Type     | Default            | Description                                                 |
| ------------------------- | -------- | ------------------ | ----------------------------------------------------------- |
| `required_confirmations`  | u64      | 0                  | Reviews required before a PR can be decided                 |
| `metric_tolerance`        | f64      | (required)         | Minimum improvement to count as "improved"                  |
| `metric_direction`        | enum     | `higher_is_better` | `higher_is_better` or `lower_is_better`                     |
| `metric_bound`            | f64      | directional        | Theoretical limit of the metric (default 0.0 for `lower_is_better`, 1.0 for `higher_is_better`). Used by the metric-floor advisory |
| `lead_github_login`       | string   | (required)         | GitHub login of the lead                                    |
| `maintainer_github_login` | string   | (required)         | GitHub login of the maintainer                              |
| `default_branch`          | string   | auto-detected      | Branch thesis worktrees are created from                    |
| `auto_approve`            | bool     | true               | Whether theses are auto-approved or need `/approve`         |
| `assignment_timeout`      | duration | 24h                | How long before a claim expires                             |
| `min_queue_depth`         | usize    | 5                  | Lead generates theses when queue is below this              |
| `max_queue_depth`         | usize    | (none)             | Lead stops generating when queue reaches this               |
| `cli_version`             | string   | (none)             | Required CLI version. Checked on startup (except bootstrap) |


### PREPARE.md config keys


| Key              | Type  | Default | Description                            |
| ---------------- | ----- | ------- | -------------------------------------- |
| `eval_cores`     | usize | 1       | CPU cores needed per evaluation run    |
| `eval_memory_gb` | f64   | 1.0     | Memory in GB needed per evaluation run |


---

## Shared state model

All protocol state lives in GitHub Issues, PRs, and structured comments. The CLI never stores protocol state locally. It derives the current state by fetching issues, PRs, and their comments, then validating the comment trail chronologically.

### Thesis lifecycle

```
Created -> Approved -> Claimed -> (experiment) -> Submitted/Released
                                                        |
                                              Submitted -> InReview -> Decided (accepted/rejected)
                                              Released -> (back to Approved, claimable by others)
```

### Comment types (structured HTML comments)

Each protocol event is a GitHub issue or PR comment with a structured HTML comment block that the CLI parses. Types:

- **Approval**: lead approves a thesis for work
- **Claim**: contributor claims a thesis (includes node ID)
- **Attempt**: contributor records an experiment result (metric, baseline, observation, summary, branch)
- **Release**: contributor releases a claim (reason: no_improvement, timeout, infra_failure)
- **PolicyPass**: lead certifies a PR meets policy requirements
- **ReviewClaim**: reviewer claims a PR for review
- **Review**: reviewer records their independent evaluation
- **Decision**: lead decides a PR (outcome: accepted, non_improvement, disagreement, stale, policy_rejection, infra_failure)
- **SlashApprove / SlashReject**: maintainer `/approve` or `/reject` commands
- **AdminNote**: lead repair actions (release-claim, acknowledge-invalid, reopen-thesis)
- **Annotate**: free-form note attached to a thesis for future contributors

### Validation rules

Comments are validated chronologically. Invalid comments produce audit findings. Key rules:

- Only the lead can post approval, policy-pass, and decision comments
- Only the maintainer can post `/approve` and `/reject` (when `auto_approve` is false)
- Claims require a prior approval
- Attempts require an active claim by the same author
- Releases require the releaser to own the active claim (or be the lead)
- Decisions require a prior policy-pass
- A thesis can only have one active claim at a time
- Claims expire after `assignment_timeout`

---

## Hardware-aware parallelism

The CLI automatically determines how many thesis workers to run in parallel on each machine. This is the mechanism that lets polyresearch saturate available hardware without manual tuning.

### Inputs

Three sources feed the parallelism decision:

1. **Machine probe** (`hardware::probe`): detects physical cores, logical cores, total memory, available memory, 1-minute load average, GPUs (via `nvidia-smi` on Linux, `system_profiler` on macOS), and platform.
2. **Capacity setting** (`.polyresearch-node.toml` `capacity` field, default 75): an integer 1-100 representing what percentage of the total machine this node is allowed to use.
3. **Eval footprint** (`PREPARE.md` keys `eval_cores` and `eval_memory_gb`): how many cores and how much memory a single evaluation run needs. Declared by the maintainer.

### Budget calculation

```
hardware_budget = probe the machine
budget.cores    = physical_cores * capacity% (floor, min 1)
budget.memory   = total_memory   * capacity%
budget.gpus     = gpu_count      * capacity% (floor, but min 1 if any GPU exists)
```

### Parallelism formula

```
effective_memory = min(budget.memory, live_available_memory)
by_cores  = max(budget.cores / eval_cores, 1)
by_memory = floor(effective_memory / eval_memory_gb)
target    = min(by_cores, max(by_memory, 1))
target    = min(target, --max-parallel)     # if flag set
target    = min(target, available_work)     # claimable + resumable theses
```

The `effective_memory` step is critical: it uses the **live free memory** (not total budget) so the CLI naturally backs off when other processes are using the machine.

### The `pace` command

`polyresearch pace` reports all three layers so the operator (or agent skill) can see the reasoning:

- **Machine line**: physical cores, logical cores, total RAM, GPUs, platform
- **Your max line**: the budget after applying `capacity%`
- **Live free line**: current load average and available memory
- **API budget**: GitHub rate limit status, cost per derive, commands remaining, near-limit flag
- **Throughput**: active claims for this node, attempts in last 1h and 4h, idle minutes, claimable theses waiting

If the GitHub quota is below the cost of a single derive operation, `pace` exits with code 75 (rate limited) and a retry message.

---

## Commands

### High-level orchestration commands

#### `polyresearch bootstrap <url>`

Sets up a new polyresearch project from a GitHub repo URL.

1. Clone or reuse existing checkout (optionally fork first with `--fork`)
2. Write template `PROGRAM.md`, `PREPARE.md`, `results.tsv` if they don't exist
3. Initialize `.polyresearch-node.toml`
4. Spawn the configured coding agent to fill in project-specific details
5. Normalize `PROGRAM.md` to ensure required sections exist

Flags: `--fork <owner>`, `--goal "<text>"`, `--pause-after-bootstrap`

Special init behavior: bootstrap must work on repos that don't have `PROGRAM.md` yet. The CLI must not require a valid `PROGRAM.md`, `cli_version` check, or `default_branch` resolution to start bootstrap.

#### `polyresearch lead`

Runs the lead management loop. The lead never claims theses or runs experiments.

Each iteration:

1. Sync `results.tsv` if stale
2. Policy-check any open PRs that lack a policy-pass
3. Decide any PRs that have enough reviews (and maintainer approval if `auto_approve` is false)
4. If queue is below `min_queue_depth` AND below `max_queue_depth` (if set) AND audit is clean: spawn agent to generate thesis proposals, truncate to the number actually needed, create issues for each

Flags: `--once` (single iteration then exit), `--sleep-secs` (interval between iterations)

Agent calls in the lead loop must not block the async runtime.

#### `polyresearch contribute [url]`

Runs the contributor experiment loop. Optionally clones from a URL first.

Each iteration:

1. Initialize node config if missing
2. Derive repo state from GitHub
3. Auto-submit any blocking submit duties (requires resuming the thesis worktree to get the right branch context)
4. Re-derive state if anything was submitted
5. Check for remaining blocking duties. If any exist, sleep and retry (or error in `--once` mode)
6. Determine parallelism using the hardware-aware formula (see "Hardware-aware parallelism" section above). Counts both claimable and resumable theses as available work
7. Resume any theses already claimed by this node
8. Claim new theses up to remaining slots
9. Dispatch experiment workers (one per thesis)
10. Collect results and handle each outcome

Flags: `--once`, `--max-parallel N`, `--sleep-secs`

### Thesis worker lifecycle

For each thesis being worked (whether resumed or freshly claimed):

**Setup phase:**

1. Create or resume the thesis worktree
2. Sync `.polyresearch-node.toml` to the worktree (this file is gitignored, so it won't pollute git state)
3. Write `.polyresearch/thesis.md` with thesis context and prior attempt history

**Experiment phase:**
4. Spawn the configured coding agent with a prompt telling it to read PROGRAM.md, PREPARE.md, and thesis.md
5. If the agent fails, attempt recovery from run logs or by running the evaluation harness directly

**Recording phase:**
6. If the experiment produced a result:

- Record the attempt via a GitHub comment
- If `improved`: commit only files within the editable surface (NOT `.polyresearch/` runtime files, NOT `PROGRAM.md`, NOT `PREPARE.md`), push, create PR. Keep the worktree alive for potential revisions.
- If `no_improvement` or `crashed`: release the claim and remove the worktree

1. If the experiment failed entirely: release the claim as `infra_failure` and remove the worktree

**Invariants:**

- Every thesis that enters the worker pool MUST be either recorded (attempt + submit/release) or released as infra_failure. No thesis may be silently dropped.
- Worker tasks must always return enough information (issue number, worktree path) for the cleanup path to run, even on failure.
- The git commit for improved theses must only include files within the editable surface. Runtime files (`.polyresearch/result.json`, `.polyresearch/thesis.md`) and config files (`PROGRAM.md`, `PREPARE.md`) must not be committed.

### Claimability rules

A thesis is claimable by a node when ALL of these are true:

- Issue state is OPEN
- Thesis is approved (both `approved` flag and `Approved` phase)
- No active claims exist
- The node has not previously released the thesis with `no_improvement` (infra_failure releases do NOT permanently blacklist -- the node can retry after the infra issue is resolved)

### Low-level commands

These are the building blocks. The high-level commands compose them. They remain available for manual use.


| Command                     | What it does                                                                                                |
| --------------------------- | ----------------------------------------------------------------------------------------------------------- |
| `init`                      | Write `.polyresearch-node.toml`. Flags: `--node`, `--capacity`                                              |
| `claim <issue>`             | Claim a thesis. Checks blocking duties first                                                                |
| `batch-claim --count N`     | Claim up to N theses at once                                                                                |
| `attempt <issue>`           | Record an experiment result. Flags: `--metric`, `--baseline`, `--observation`, `--summary`, `--annotations` |
| `annotate <issue>`          | Attach a free-form note to a thesis                                                                         |
| `release <issue>`           | Release a claim. Flag: `--reason` (no_improvement, timeout, infra_failure)                                  |
| `submit <issue>`            | Push the thesis branch and create a PR. Must be run from the thesis worktree                                |
| `review-claim <pr>`         | Claim a PR for review                                                                                       |
| `review <pr>`               | Record a review result. Flags: `--metric`, `--baseline`, `--observation`                                    |
| `duties`                    | Show blocking and advisory duties for the current node                                                      |
| `audit`                     | Check for protocol violations across all issues and PRs                                                     |
| `sync`                      | Update `results.tsv` from the current issue/PR state                                                        |
| `generate`                  | Create a new thesis issue. Flags: `--title`, `--body`. Requires lead role, clean audit, and queue below max |
| `policy-check <pr>`         | Lead certifies a PR meets policy                                                                            |
| `decide <pr>`               | Lead decides a PR. Requires prior policy-pass                                                               |
| `pace`                      | Report hardware budget, API budget, and throughput                                                          |
| `status`                    | Report queue depth, thesis states, node metrics. Flag: `--tui` for dashboard                                |
| `prune`                     | Remove stale worktree directories                                                                           |
| `admin release-claim`       | Lead force-releases a stuck claim                                                                           |
| `admin acknowledge-invalid` | Lead acknowledges an invalid comment to suppress it from audit                                              |
| `admin reopen-thesis`       | Lead reopens a closed thesis                                                                                |
| `admin reconcile-ledger`    | Re-sync ledger via `sync`                                                                                   |


### Global flags

All commands accept:

- `--repo <owner/name>` -- explicit repo override
- `--json` -- JSON output instead of human-readable
- `--dry-run` -- show what would happen without making changes
- `--github-debug` -- log GitHub API calls

---

## Node identity

Each machine running polyresearch has a node identity stored in `.polyresearch-node.toml`:

```toml
node_id = "hostname-ab3f"
capacity = 75
api_budget = 5000
request_delay_ms = 100

[agent]
command = "claude -p --permission-mode bypassPermissions"
```

The `node_id` is generated automatically by `init` (hostname + random suffix). It identifies the machine in claim and release comments.

The `POLYRESEARCH_NODE_ID` environment variable overrides the file's `node_id` for the current session. This is required when multiple agents share one checkout or when one GitHub login runs several workers in parallel -- each needs a distinct node ID to avoid claim collisions.

---

## Agent runner interface

The CLI spawns the configured agent command as a subprocess. The interface is:

1. CLI writes context files to the worktree (`.polyresearch/thesis.md`, `.polyresearch-node.toml`)
2. CLI invokes the agent with a prompt string and the worktree as the working directory
3. Agent reads `PROGRAM.md`, `PREPARE.md`, `.polyresearch/thesis.md`
4. Agent modifies code within the editable surface, runs benchmarks, and writes `.polyresearch/result.json`
5. CLI reads `.polyresearch/result.json` for the experiment result

If the agent crashes or exits without writing `result.json`, the CLI attempts recovery:

- Parse benchmark run logs (`run-*.log`) for metric values using patterns `ops_per_sec=<number>` and `^METRIC=<number>$`
- As a last resort, run the evaluation harness directly (`.polyresearch/run.sh` or `bench.js/bench.mjs`) against both the candidate worktree and a detached baseline worktree, then compare metrics

For thesis generation (lead loop), the agent writes a JSON array of `{"title": "...", "body": "..."}` objects to `.polyresearch/thesis-proposals.json`.

---

## GitHub API throttling and retries

### Cross-process throttling

All GitHub API calls go through a throttle that enforces a minimum delay between requests (configured via `request_delay_ms`, default 100ms). The throttle uses a file-based timestamp (`.polyresearch-throttle` in a temp directory) so multiple CLI processes on the same machine coordinate without overshooting rate limits.

### Retry logic

Failed GitHub API calls are classified and retried:

- **Transient errors** (HTTP 502, 503, bad gateway, service unavailable): retried on idempotent requests with delays of 5s, 10s, 20s
- **Secondary rate limits** (abuse detection, "please wait", HTTP 429, retry-after): retried with server-provided `Retry-After` or fallback delays of 90s, 180s, 300s
- **Primary rate limits** (API rate limit exceeded): the CLI fetches the rate limit reset timestamp and waits until reset + 1s

All retry delays include +/-50% jitter so concurrent agents don't wake up at the same time.

---

## Worktree and branch naming

Thesis worktrees live under `.worktrees/` (gitignored) in the repo root:

- **Path**: `.worktrees/{issue_number}-{slug}`
- **Branch**: `thesis/{issue_number}-{slug}`

The slug is derived from the thesis title: lowercased, non-alphanumeric characters replaced with dashes, leading/trailing dashes stripped.

Worktrees are created from the default branch. They are removed after a thesis is released. For improved theses that are submitted as PRs, the worktree is preserved until the PR is decided and `polyresearch prune` cleans up.

---

## Duties system

The `duties` command reports what the current node should do next. Duties are divided into two categories:

### Blocking duties

Must be resolved before the node can claim new work. The `claim`, `batch-claim`, and `generate` commands refuse to proceed when blocking duties exist.

**Contributor blocking duties:**

- `submit`: an improved attempt was recorded but no PR was created yet

**Lead blocking duties:**

- `policy-check`: an open PR lacks a policy-pass
- `decide`: a PR has enough reviews and is ready for a decision
- `sync`: `results.tsv` is stale (has missing rows)

### Advisory duties

Informational. Do not block claims.

- `attempt`: a claim exists with 0 attempts posted
- `review`: a PR needs review (not authored by this node)
- `queue`: queue depth is below `min_queue_depth` (lead only)
- `idle`: no claimable work exists (contributor)
- `awaiting-approval`: theses are waiting for maintainer `/approve`
- `no-claimable-work`: all theses have been tried by this node, or metric floor reached
- `metric-floor` / `stale-queue`: best metric is already below tolerance (lead only)
- `maintainer-approval`: a PR or thesis needs maintainer `/approve` (lead only, when `auto_approve` is false)

---

## Ledger (results.tsv)

The experiment ledger is a TSV file maintained by the lead via `sync`. Format:

```
thesis	attempt	metric	baseline	status	summary
#12	thesis/12-rmsnorm-attempt-1	0.9934	0.9979	accepted	RMSNorm instead of LayerNorm
#15	thesis/15-attention-opt	—	0.5000	crashed	Attention optimization attempt
```

Columns: thesis issue reference, attempt branch name, metric value (or `—` for crashed/infra_failure), baseline metric, status, summary.

Status values come from the PR decision outcome (`accepted`, `non_improvement`, `disagreement`, `stale`, `policy_rejection`, `infra_failure`) or from the observation directly (`crashed`, `infra_failure`, `discarded`).

`sync` derives missing rows from the current GitHub state, appends them, commits `results.tsv`, and pushes.

`is_current` checks whether any rows are missing (used by `lead` to decide when to sync, and by `decide`/`generate` as a precondition guard).

---

## Policy check logic

`policy-check` verifies that a PR only touches files within the editable surface:

1. Fetch the list of changed files in the PR via GitHub API
2. For each file, check against the editable surface globs from PROGRAM.md ("What you CAN modify") and the protected list ("What you CANNOT modify")
3. A file passes if it matches at least one editable glob AND does not match any protected glob
4. If all files pass: post a `PolicyPass` structured comment
5. If any file violates: post a `Decision` with outcome `policy_rejection`, close the PR, and close the thesis issue

---

## Decision logic

`decide` evaluates a PR and posts the outcome:

**Without peer review** (`required_confirmations` = 0):

1. Find the attempt matching the PR branch
2. Check if the attempt's metric beats its baseline by `metric_tolerance`
3. Check if the metric meets or exceeds the best previously accepted metric in the ledger
4. If both pass: `accepted`. Otherwise: `non_improvement`

**With peer review** (`required_confirmations` > 0):

1. Verify enough reviews exist
2. Check if any reviewer's `base_sha` is stale (doesn't match current default branch HEAD) -> `stale`
3. Check if reviewers ran different eval environments (`env_sha` disagrees) -> `disagreement`
4. If majority of reviews are `crashed`/`infra_failure` -> `infra_failure`
5. If all reviewers agree `improved` and metrics are within tolerance -> `accepted`
6. If all reviewers agree `no_improvement` and metrics agree -> `non_improvement`
7. Otherwise -> `disagreement`

**Post-decision actions:**

- `accepted`: merge the PR, post decision comment, close the thesis issue
- `non_improvement` (with peer review) / `disagreement` / `policy_rejection`: close PR, close thesis issue
- `non_improvement` (without peer review): close PR only (thesis stays open for another attempt)
- `stale` / `infra_failure`: close PR only

---

## Structured comment wire format

Protocol comments are embedded in GitHub issue/PR comments as HTML comment blocks:

```
Visible summary text for humans.

<!-- polyresearch:comment-type
key: value
key: value
-->
```

The regex `<!--\s*polyresearch:([a-z-]+)\s*\n(.*?)-->` extracts the type and key-value payload. Fields are parsed as `key: value` lines within the block.

Comment types in the wire format: `approval`, `claim`, `release`, `attempt`, `annotation`, `policy-pass`, `review-claim`, `review`, `decision`, `admin-note`.

Slash commands (`/approve`, `/reject`) are parsed from the raw comment body (no HTML block needed). Email-quoted lines (starting with `>`) are skipped to avoid parsing forwarded protocol comments.

---

## Protocol data types

### Observation

`improved`, `no_improvement`, `crashed`, `infra_failure`

### ReleaseReason

`no_improvement`, `timeout`, `infra_failure`

### Outcome (PR decision)

`accepted`, `non_improvement`, `disagreement`, `stale`, `policy_rejection`, `infra_failure`

### MetricDirection

`higher_is_better`, `lower_is_better`

---

## Architecture hints

These are lessons learned from the v1 implementation. They are suggestions for the rebuild, not requirements.

### 1. Split orchestration loops into lifecycle phases

The v1 `contribute.rs` grew to 700+ lines because setup, experiment, recording, and cleanup were all inlined in one function. Every edge-case fix exposed the next one because concerns were tangled.

Recommended structure: a `ThesisWorker` (or similar) that owns `(issue_number, worktree_path)` and exposes discrete phases:

```
setup()    -> Result<()>        // create/resume worktree, sync config, write thesis.md
run()      -> Result<ExperimentResult>  // spawn agent, recover on failure
record()   -> Result<()>        // post attempt, submit or release
cleanup()  -> Result<()>        // remove worktree if appropriate
```

Each phase has its own error handling. The main loop dispatches to workers and guarantees that `cleanup()` runs regardless of which phase failed. This is the most important architectural change.

### 2. Worker tasks must be infallible containers

When spawning tasks into a JoinSet, the task closure should never propagate errors via `?`. It should always return a result struct that includes the issue number and worktree path, so the caller can always clean up. The only unrecoverable failure mode should be a panic.

### 3. Git operations must respect the editable surface

When committing experiment results, never use `git add -A`. Instead, use the editable surface globs from `PROGRAM.md` to selectively stage only files the agent was allowed to modify. Runtime files (`.polyresearch/*`, `PROGRAM.md`, `PREPARE.md`, `.polyresearch-node.toml`) must never be committed to thesis branches.

### 4. Defer setup for commands that create their own context

`bootstrap` and `contribute <url>` operate on repos that may not exist or may not be fully configured yet. The CLI entrypoint (`main.rs`) should not unconditionally require `PROGRAM.md`, `cli_version`, or `default_branch` for these commands. Use lazy initialization or command-specific setup paths.

### 5. Context passing should be explicit

Avoid the pattern of cloning a shared `AppContext` and replacing fields (like `repo_root`) to create worktree-specific contexts. This creates confusion about which operations use the main repo root vs. the worktree. Instead, pass `worktree_path` as an explicit parameter where needed, or use a dedicated worktree context type.

### 6. Release reason should affect reclaimability

When a thesis is released with `infra_failure`, the same node should be allowed to reclaim it later (the infra issue may be resolved). Only `no_improvement` releases should permanently prevent the same node from reclaiming. This is a filter predicate change, not a structural one.

### 7. Queue depth bounds must be enforced consistently

Both `min_queue_depth` and `max_queue_depth` must be checked before generating new theses. The number of proposals generated should be capped to not exceed `max_queue_depth`. The agent may return more proposals than requested, so truncation after generation is necessary.

### 8. Async discipline

Any function that shells out to a long-running process (agent invocation, benchmark harness) must run in `spawn_blocking` or equivalent. The lead and contribute loops are async, and blocking the tokio runtime thread causes all concurrent work to stall.

### 9. State derivation should happen once per phase

Avoid re-deriving `RepositoryState` multiple times within the same loop iteration unless a mutation (like auto-submit) invalidates it. When a mutation does happen, re-derive exactly once afterward. The v1 code had both unnecessary re-derivations and missing re-derivations.

### 10. Test the lifecycle, not just the happy path

The v1 e2e tests covered the happy path well but missed failure/cleanup paths. The rebuild should include tests for:

- Worker failure -> release + cleanup
- Claim failure -> orphaned worktree cleanup
- Auto-submit with stale state -> re-derivation
- Bootstrap on a repo without PROGRAM.md
- Contribute with blocking non-submit duties

