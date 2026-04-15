# Bookface Post

## Introducing polyresearch -- distributed autoresearch

We just open-sourced polyresearch. If you've been running AI agents for research and hit the ceiling, this is for you.

Karpathy's autoresearch proved the core loop: give an agent a language model and a metric, let it experiment, wake up to better results. But research has never been single-player. We extended the loop to any number of machines and contributors, with independent verification and full experiment history.

If you have something measurable you want to improve -- model accuracy, eval scores, prompt quality, agent performance -- you can throw a hundred machines at it and wake up to verified, reproducible progress. 

### How it works

Tell your agent to bootstrap polyresearch on any GitHub repo. It generates a `PROGRAM.md` (what to optimize, which files agents can edit), a `PREPARE.md` (the evaluation setup -- agents can't modify how they're scored), and optionally a `.polyresearch/` directory with setup scripts, frozen dependencies, and the evaluator itself. That directory is the reproducible environment -- every machine runs the same setup, the same eval, the same scoring. That's how you get results you can actually trust across a hundred different machines. Then point contributor agents at the repo from as many machines as you have.

Agents coordinate through GitHub Issues and PRs. No external services, no database. A Rust CLI handles claim-based work distribution, result syncing, and state validation so agents don't vibe the process.

Every experiment -- accepted, failed, crashed -- gets a row in `results.tsv` and stays as a branch. The lead agent reads the full history when generating new research directions.

Human-in-the-loop when you want it: set `auto_approve: false` and the lead waits for your `/approve` or `/reject` on each thesis and PR. Or run fully autonomous overnight.

Works with any coding agent, any model, any sandbox.

### Examples

- ESLint: single-file linting 24% faster over 75 experiments
- Core War: evolved a Redcode warrior with 218% improvement over 27 experiments
- PostCSS: ~20% performance improvement

### Join the fun

Star, fork, contribute:  
[https://www.github.com/superagent-ai/polyresearch](https://www.github.com/superagent-ai/polyresearch)