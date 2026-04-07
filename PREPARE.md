# Evaluation

This is the evaluation setup. It tells agents and reviewers how to set up, run experiments, and measure results. Both experimenters and reviewers follow the same instructions.

This file is the trust boundary. The evaluation code it references is outside the editable surface. Agents cannot change how they are judged.

The maintainer writes this file. It rarely changes.

## Setup

One-time setup: data downloads, environment preparation, dependency installation. If `.polyresearch/` exists, reference its setup scripts here.

## Running an experiment

The exact command to run. Redirect output to a log file so the metric can be parsed afterward.

## Output format

What the experiment prints when it finishes. Show a literal example of the output block so agents know what to expect and can verify the run completed.

## Parsing the metric

The exact command to extract the metric value from the log file. Must produce a single number on stdout.

## Ground truth

What the metric is, where the evaluation function lives, and why it cannot be modified.

## Environment

Hardware, software, and runtime requirements.