#!/usr/bin/env python3
"""
Frozen evaluation harness for the Core Wars polyresearch example.

DO NOT MODIFY - this file is outside the editable surface.
It is the trust boundary described in PREPARE.md.
"""

from __future__ import annotations

import argparse
from collections import defaultdict
from concurrent.futures import ProcessPoolExecutor
from dataclasses import dataclass
import os
from pathlib import Path
import random
import sys
import time

from corewar import Core, MARS, parse


ROOT = Path(__file__).resolve().parent
CANDIDATE_PATH = ROOT.parent / "warrior.red"
OPPONENTS_DIR = ROOT / "opponents"

CORESIZE = 8000
MAX_CYCLES = 80000
MAX_PROCESSES = 8000
MAX_LENGTH = 100
MIN_DISTANCE = 100
SEEDS_PER_OPPONENT = 50

OPPONENT_FILES = [
    "imp.red",
    "dwarf.red",
    "mice.red",
    "nonzeroscanner.red",
    "dwarfmice.red",
    "impgate.red",
    "rato.red",
    "stone.red",
]

ENVIRONMENT = {
    "CORESIZE": CORESIZE,
    "CYCLES": MAX_CYCLES,
    "ROUNDS": 1,
    "MAXPROCESSES": MAX_PROCESSES,
    "MAXLENGTH": MAX_LENGTH,
    "MINDISTANCE": MIN_DISTANCE,
}

POINTS = {"win": 3, "draw": 1, "loss": 0}


@dataclass(frozen=True)
class TrialTask:
    opponent_name: str
    candidate_path: str
    opponent_path: str
    seed: int
    candidate_first: bool


def load_warrior(path: Path):
    text = path.read_text(encoding="latin-1")
    warrior = parse(text.splitlines(), ENVIRONMENT)
    if len(warrior.instructions) > MAX_LENGTH:
        raise ValueError(
            f"{path.name} has {len(warrior.instructions)} instructions; "
            f"max allowed is {MAX_LENGTH}."
        )
    return warrior


def run_trial(task: TrialTask) -> tuple[str, str]:
    candidate = load_warrior(Path(task.candidate_path))
    opponent = load_warrior(Path(task.opponent_path))

    random.seed(task.seed)
    warriors = [candidate, opponent] if task.candidate_first else [opponent, candidate]

    simulation = MARS(
        core=Core(size=CORESIZE),
        warriors=warriors,
        minimum_separation=MIN_DISTANCE,
        max_processes=MAX_PROCESSES,
    )

    candidate_index = 0 if task.candidate_first else 1
    for _ in range(MAX_CYCLES):
        simulation.step()
        active = [bool(warrior.task_queue) for warrior in warriors]
        if sum(active) <= 1:
            if active[candidate_index]:
                return task.opponent_name, "win"
            return task.opponent_name, "loss"

    return task.opponent_name, "draw"


def build_tasks(candidate_path: Path) -> list[TrialTask]:
    tasks: list[TrialTask] = []
    for opponent_name in OPPONENT_FILES:
        opponent_path = OPPONENTS_DIR / opponent_name
        for seed in range(SEEDS_PER_OPPONENT):
            tasks.append(
                TrialTask(
                    opponent_name=opponent_name,
                    candidate_path=str(candidate_path),
                    opponent_path=str(opponent_path),
                    seed=seed,
                    candidate_first=True,
                )
            )
            tasks.append(
                TrialTask(
                    opponent_name=opponent_name,
                    candidate_path=str(candidate_path),
                    opponent_path=str(opponent_path),
                    seed=seed,
                    candidate_first=False,
                )
            )
    return tasks


def main() -> None:
    parser = argparse.ArgumentParser(description="Evaluate a Core War warrior")
    parser.add_argument(
        "--candidate",
        default=str(CANDIDATE_PATH),
        help="Path to the editable warrior file",
    )
    parser.add_argument(
        "--workers",
        type=int,
        default=int(os.environ.get("COREWAR_WORKERS", min(os.cpu_count() or 1, 8))),
        help="Worker processes for battle simulation",
    )
    args = parser.parse_args()

    candidate_path = Path(args.candidate).resolve()
    if not candidate_path.exists():
        print(f"Error: candidate file not found: {candidate_path}", file=sys.stderr)
        sys.exit(1)

    start_time = time.time()

    try:
        candidate = load_warrior(candidate_path)
    except Exception as exc:
        print(f"Error parsing candidate warrior: {exc}", file=sys.stderr)
        sys.exit(1)

    # Preflight all frozen opponents once so parse failures show up clearly.
    for opponent_name in OPPONENT_FILES:
        opponent_path = OPPONENTS_DIR / opponent_name
        if not opponent_path.exists():
            print(f"Error: opponent file not found: {opponent_path}", file=sys.stderr)
            sys.exit(1)
        try:
            load_warrior(opponent_path)
        except Exception as exc:
            print(f"Error parsing opponent {opponent_name}: {exc}", file=sys.stderr)
            sys.exit(1)

    tasks = build_tasks(candidate_path)
    print(
        f"Running {len(tasks)} battles across {len(OPPONENT_FILES)} opponents "
        f"with {args.workers} worker(s)...",
        file=sys.stderr,
    )

    counts: dict[str, dict[str, int]] = defaultdict(lambda: {"win": 0, "draw": 0, "loss": 0})

    if args.workers <= 1:
        for task in tasks:
            opponent_name, result = run_trial(task)
            counts[opponent_name][result] += 1
    else:
        with ProcessPoolExecutor(max_workers=args.workers) as executor:
            for opponent_name, result in executor.map(run_trial, tasks):
                counts[opponent_name][result] += 1

    total_wins = total_draws = total_losses = 0
    opponents_beaten = 0

    print("Per-opponent results:", file=sys.stderr)
    for opponent_name in OPPONENT_FILES:
        result_counts = counts[opponent_name]
        wins = result_counts["win"]
        draws = result_counts["draw"]
        losses = result_counts["loss"]
        total_wins += wins
        total_draws += draws
        total_losses += losses

        battles = wins + draws + losses
        score = (wins * 3 + draws) / (3 * battles) if battles else 0.0
        if score > 0.5:
            opponents_beaten += 1

        print(
            f"  {Path(opponent_name).stem}: "
            f"score={score:.4f} wins={wins} draws={draws} losses={losses}",
            file=sys.stderr,
        )

    total_battles = total_wins + total_draws + total_losses
    battle_score = (
        (total_wins * POINTS["win"] + total_draws * POINTS["draw"])
        / (3 * total_battles)
        if total_battles
        else 0.0
    )
    elapsed = time.time() - start_time

    print("---")
    print(f"battle_score: {battle_score:.4f}")
    print(f"wins: {total_wins}")
    print(f"draws: {total_draws}")
    print(f"losses: {total_losses}")
    print(f"opponents_beaten: {opponents_beaten}/{len(OPPONENT_FILES)}")
    print(f"battles_per_opponent: {SEEDS_PER_OPPONENT * 2}")
    print(f"total_battles: {total_battles}")
    print(f"warrior_length: {len(candidate.instructions)}")
    print(f"workers: {args.workers}")
    print(f"elapsed_seconds: {elapsed:.1f}")


if __name__ == "__main__":
    main()
