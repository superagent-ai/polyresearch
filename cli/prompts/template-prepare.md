# Evaluation Setup

This file is outside the editable surface. It defines how results are judged. Agents cannot modify the evaluator or the scoring logic — the evaluation is the trust boundary.

Consider defining more than one evaluation criterion. Optimizing for a single number makes it easy to overfit and silently break other things. A secondary metric or sanity check helps keep the process honest.

eval_cores: 1
eval_memory_gb: 1.0
prereq_command:

## Setup

Install dependencies and prepare the evaluation environment.

If the project has a build step (e.g. TypeScript compilation), set `prereq_command` above to the build command (e.g. `npm run build`). The CLI runs this before the evaluation harness during recovery, ensuring it measures compiled output rather than stale artifacts.

## Run command

```bash
# Replace with actual benchmark command
echo "METRIC=0.0"
```

## Output format

The benchmark must print `METRIC=<number>` to stdout.

## Metric parsing

The CLI looks for `METRIC=<number>` or `ops_per_sec=<number>` in the output.

## Ground truth

Describe what the baseline metric represents and how it was measured.
