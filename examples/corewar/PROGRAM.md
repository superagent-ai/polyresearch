# Research program

## Goal

Maximize `battle_score` (higher is better). The evaluator loads `warrior.red`
and runs it against a frozen gauntlet of eight opponent warriors across 100
deterministic battles per opponent.

Each battle is scored:

- Win = 3 points
- Draw = 1 point
- Loss = 0 points

The final metric is:

`battle_score = total_points / max_possible_points`

A pure draw machine scores about `0.3333`. A specialist that crushes one
opponent type but loses badly to the rest will plateau. The goal is to build a
general-purpose warrior that performs well across the whole gauntlet.

## Configuration

These are the project-specific coordination values referenced by
`POLYRESEARCH.md`.

| Parameter                | Value              |
| ------------------------ | ------------------ |
| `required_confirmations` | `0`                |
| `metric_tolerance`       | `0.00`             |
| `metric_direction`       | `higher_is_better` |
| `lead_github_login`      | `replace-me`       |
| `assignment_timeout`     | `24h`              |
| `review_timeout`         | `12h`              |
| `min_queue_depth`        | `5`                |
| `max_queue_depth`        | `10`               |

## What you CAN modify

- `warrior.red` - the candidate warrior. Anything inside this file is fair
game: instruction sequence, labels, constants, bombing pattern, scan logic,
replication logic, imp launchers, decoys, process management.

## What you CANNOT modify

- `.polyresearch/` - the trusted environment: setup, evaluator, LLM helper,
prompts, simulator, gauntlet, vendored source notes, and dependencies
- `PREPARE.md` - the trust boundary
- `POLYRESEARCH.md` - the coordination protocol
- `results.tsv` - maintained by the lead on `main`

## Constraints

- `warrior.red` must parse as valid Redcode.
- Maximum warrior length is 100 instructions. The evaluator enforces this.
- Do not depend on external files, network access, or anything outside
`warrior.red`.
- Simpler is better. If two warriors score the same, the shorter and clearer
one is a better result.

## Redcode quick reference

Useful opcodes:

- `DAT` - kill the current process
- `MOV` - copy data or whole instructions
- `ADD`, `SUB`, `MUL`, `DIV`, `MOD` - arithmetic on target fields
- `JMP`, `JMZ`, `JMN`, `DJN` - control flow
- `SPL` - spawn another process
- `SEQ`/`CMP`, `SNE`, `SLT` - comparisons and scan loops
- `NOP` - no operation

Common addressing modes:

- `#` immediate
- `$` direct
- `@` B-field indirect
- `<` B-field predecrement
- `>` B-field postincrement
- `*` A-field indirect
- `{` A-field predecrement
- `}` A-field postincrement

Useful directives:

- `ORG <label>` - set the start instruction
- `END [label]` - stop assembly, optionally overriding the start label
- `EQU` - define constants with arithmetic expressions

## Strategy

Core War has strong rock-paper-scissors dynamics, so the best warriors are
usually hybrids or carefully tuned specialists with good matchups:

- Bombers ("rock") fill memory with `DAT` or `SPL` bombs and do well against
many scanners.
- Papers/replicators ("paper") survive damage by spreading copies across core.
- Scanners ("scissors") probe memory and then clear or stun detected targets.
- Imps and imp-based endgames are tiny and durable, but gates and scanners can
farm them.
- Hybrids combine launchers, bombing, scanning, or replication to avoid obvious
blind spots.

The frozen gauntlet intentionally includes bombers, papers, scanners, an imp,
an imp gate, and hybrids. Read `results.tsv` to see which ideas previous
contributors tried and where they failed.

There is also an optional frozen helper tool:
`.polyresearch/evolve.py`. It uses `litellm` with OpenAI's `gpt-5.4-mini` to
generate and mutate warriors, then evaluates every candidate with the same full
800-battle gauntlet used by `.polyresearch/evaluate.py`. This is intentionally
slow on a single machine. That is a feature, not a bug: polyresearch shines
when many contributors run different search seeds and ideas across many
machines while sharing one verified experiment history.