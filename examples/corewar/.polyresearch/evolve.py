#!/usr/bin/env python3
"""
Frozen LLM evolution helper for the Core Wars polyresearch example.

This tool is intentionally outside the editable surface. Contributors can use
it to search for stronger `warrior.red` candidates, but accepted improvements
are still determined by the frozen full benchmark in `evaluate.py`.
"""

from __future__ import annotations

import argparse
from collections import defaultdict
from concurrent.futures import ProcessPoolExecutor
from dataclasses import dataclass
import os
from pathlib import Path
import random
import re
import sys
import tempfile
import time

import litellm
from dotenv import find_dotenv, load_dotenv

import evaluate as frozen_eval


ROOT = Path(__file__).resolve().parent
PROMPTS_DIR = ROOT / "prompts"
DEFAULT_MODEL = "gpt-5.4-mini"
DEFAULT_ROUNDS = 10
DEFAULT_CANDIDATES = 8
MODEL_PRICING = {
    "gpt-5.4-mini": {"input": 0.75, "output": 4.50},
}

CODE_BLOCK_RE = re.compile(r"```(?:redcode)?\s*(.*?)```", re.IGNORECASE | re.DOTALL)

litellm.suppress_debug_info = True
load_dotenv(find_dotenv(usecwd=True))


@dataclass
class EvaluationSummary:
    score: float
    wins: int
    draws: int
    losses: int
    opponents_beaten: int
    warrior_length: int
    breakdown: dict[str, dict[str, float | int]]


def load_prompt(name: str) -> str:
    path = PROMPTS_DIR / name
    if not path.exists():
        raise FileNotFoundError(f"Prompt file not found: {path}")
    return path.read_text()


def extract_redcode(response_text: str) -> str:
    match = CODE_BLOCK_RE.search(response_text)
    if match:
        return match.group(1).strip()
    return response_text.strip()


def format_breakdown(summary: EvaluationSummary) -> str:
    lines = []
    for opponent_name in frozen_eval.OPPONENT_FILES:
        stats = summary.breakdown[opponent_name]
        label = Path(opponent_name).stem
        lines.append(
            f"- {label}: score={stats['score']:.4f}, "
            f"wins={stats['wins']}, draws={stats['draws']}, losses={stats['losses']}"
        )
    return "\n".join(lines)


def call_model(
    *,
    model: str,
    system_prompt: str,
    user_prompt: str,
    temperature: float,
    max_tokens: int,
    seed: int,
) -> tuple[str, int, int]:
    response = litellm.completion(
        model=model,
        messages=[
            {"role": "system", "content": system_prompt},
            {"role": "user", "content": user_prompt},
        ],
        temperature=temperature,
        max_tokens=max_tokens,
        seed=seed,
    )
    text = response.choices[0].message.content or ""
    usage = getattr(response, "usage", None)
    prompt_tokens = getattr(usage, "prompt_tokens", 0) if usage else 0
    completion_tokens = getattr(usage, "completion_tokens", 0) if usage else 0
    return text, prompt_tokens or 0, completion_tokens or 0


def evaluate_candidate(candidate_path: Path, workers: int) -> EvaluationSummary:
    candidate = frozen_eval.load_warrior(candidate_path)
    tasks = frozen_eval.build_tasks(candidate_path)

    counts: dict[str, dict[str, int]] = defaultdict(
        lambda: {"win": 0, "draw": 0, "loss": 0}
    )

    if workers <= 1:
        for task in tasks:
            opponent_name, result = frozen_eval.run_trial(task)
            counts[opponent_name][result] += 1
    else:
        with ProcessPoolExecutor(max_workers=workers) as executor:
            for opponent_name, result in executor.map(frozen_eval.run_trial, tasks):
                counts[opponent_name][result] += 1

    total_wins = total_draws = total_losses = 0
    opponents_beaten = 0
    breakdown: dict[str, dict[str, float | int]] = {}

    for opponent_name in frozen_eval.OPPONENT_FILES:
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

        breakdown[opponent_name] = {
            "wins": wins,
            "draws": draws,
            "losses": losses,
            "score": score,
        }

    total_battles = total_wins + total_draws + total_losses
    battle_score = (
        (total_wins * frozen_eval.POINTS["win"] + total_draws * frozen_eval.POINTS["draw"])
        / (3 * total_battles)
        if total_battles
        else 0.0
    )

    return EvaluationSummary(
        score=battle_score,
        wins=total_wins,
        draws=total_draws,
        losses=total_losses,
        opponents_beaten=opponents_beaten,
        warrior_length=len(candidate.instructions),
        breakdown=breakdown,
    )


def make_candidate_prompt(
    *,
    current_warrior: str,
    current_summary: EvaluationSummary,
    generate_prompt: str,
    mutate_prompt: str,
    fresh: bool,
) -> str:
    if fresh:
        return generate_prompt
    return mutate_prompt.format(
        current_warrior=current_warrior,
        current_score=current_summary.score,
        breakdown=format_breakdown(current_summary),
    )


def save_text(path: Path, text: str) -> None:
    path.write_text(text.rstrip() + "\n")


def main() -> None:
    parser = argparse.ArgumentParser(description="Evolve a Core War warrior with litellm")
    parser.add_argument("--model", default=DEFAULT_MODEL, help="litellm model string")
    parser.add_argument(
        "--rounds", type=int, default=DEFAULT_ROUNDS, help="Evolution rounds"
    )
    parser.add_argument(
        "--candidates",
        type=int,
        default=DEFAULT_CANDIDATES,
        help="Candidates generated per round",
    )
    parser.add_argument(
        "--workers",
        type=int,
        default=int(os.environ.get("COREWAR_WORKERS", min(os.cpu_count() or 1, 8))),
        help="Worker processes for full battle evaluation",
    )
    parser.add_argument(
        "--temperature", type=float, default=1.0, help="Sampling temperature"
    )
    parser.add_argument(
        "--seed", type=int, default=0, help="Seed for deterministic candidate labels"
    )
    parser.add_argument(
        "--max-output-tokens",
        type=int,
        default=800,
        help="Max output tokens per model call",
    )
    parser.add_argument(
        "--new",
        action="store_true",
        help="Start round 1 from freshly generated warriors instead of mutating warrior.red",
    )
    args = parser.parse_args()

    if not os.environ.get("OPENAI_API_KEY"):
        print("Error: OPENAI_API_KEY is required for evolve.py.", file=sys.stderr)
        sys.exit(1)

    system_prompt = load_prompt("system.txt")
    mutate_prompt = load_prompt("mutate.txt")
    generate_prompt = load_prompt("generate.txt")

    warrior_path = frozen_eval.CANDIDATE_PATH
    current_warrior = warrior_path.read_text(encoding="latin-1").strip()

    print("Evaluating current seed warrior...", file=sys.stderr)
    overall_start = time.time()
    current_summary = evaluate_candidate(warrior_path, args.workers)
    best_warrior = current_warrior
    best_summary = current_summary

    print(
        f"Initial score: {best_summary.score:.4f} "
        f"(wins={best_summary.wins}, draws={best_summary.draws}, losses={best_summary.losses})",
        file=sys.stderr,
    )

    total_prompt_tokens = 0
    total_completion_tokens = 0
    rng = random.Random(args.seed)

    with tempfile.TemporaryDirectory(prefix="corewar-evolve-") as temp_dir_name:
        temp_dir = Path(temp_dir_name)

        for round_index in range(1, args.rounds + 1):
            print(
                f"Round {round_index}/{args.rounds}: generating {args.candidates} candidate(s)...",
                file=sys.stderr,
            )

            candidate_summaries: list[tuple[EvaluationSummary, str]] = []

            for candidate_index in range(1, args.candidates + 1):
                prompt_text = make_candidate_prompt(
                    current_warrior=best_warrior,
                    current_summary=best_summary,
                    generate_prompt=generate_prompt,
                    mutate_prompt=mutate_prompt,
                    fresh=args.new and round_index == 1,
                )

                try:
                    response_text, prompt_tokens, completion_tokens = call_model(
                        model=args.model,
                        system_prompt=system_prompt,
                        user_prompt=prompt_text,
                        temperature=args.temperature,
                        max_tokens=args.max_output_tokens,
                        seed=args.seed + round_index * 1000 + candidate_index,
                    )
                except Exception as exc:
                    print(
                        f"  candidate {candidate_index}: model call failed: {exc}",
                        file=sys.stderr,
                    )
                    continue

                total_prompt_tokens += prompt_tokens
                total_completion_tokens += completion_tokens

                candidate_text = extract_redcode(response_text)
                candidate_path = temp_dir / (
                    f"round-{round_index:02d}-candidate-{candidate_index:02d}-{rng.randrange(1_000_000):06d}.red"
                )
                save_text(candidate_path, candidate_text)

                try:
                    summary = evaluate_candidate(candidate_path, args.workers)
                except Exception as exc:
                    print(
                        f"  candidate {candidate_index}: invalid or failed evaluation: {exc}",
                        file=sys.stderr,
                    )
                    continue

                candidate_summaries.append((summary, candidate_text))
                print(
                    f"  candidate {candidate_index}: "
                    f"score={summary.score:.4f} "
                    f"length={summary.warrior_length} "
                    f"opponents_beaten={summary.opponents_beaten}/{len(frozen_eval.OPPONENT_FILES)}",
                    file=sys.stderr,
                )

            if not candidate_summaries:
                print("  no valid candidates this round; keeping incumbent", file=sys.stderr)
                continue

            round_best_summary, round_best_warrior = max(
                candidate_summaries,
                key=lambda item: (item[0].score, -item[0].warrior_length),
            )

            delta = round_best_summary.score - best_summary.score
            if (
                round_best_summary.score > best_summary.score
                or (
                    abs(round_best_summary.score - best_summary.score) < 1e-9
                    and round_best_summary.warrior_length < best_summary.warrior_length
                )
            ):
                best_summary = round_best_summary
                best_warrior = round_best_warrior
                save_text(warrior_path, best_warrior)
                print(
                    f"  round winner accepted: score={best_summary.score:.4f} "
                    f"delta={delta:+.4f}",
                    file=sys.stderr,
                )
            else:
                print(
                    f"  no improvement this round: best candidate score={round_best_summary.score:.4f} "
                    f"delta={delta:+.4f}",
                    file=sys.stderr,
                )

        elapsed = time.time() - overall_start

    pricing = MODEL_PRICING.get(args.model)
    if pricing:
        input_cost = (total_prompt_tokens / 1_000_000) * pricing["input"]
        output_cost = (total_completion_tokens / 1_000_000) * pricing["output"]
        total_cost_text = f"{input_cost + output_cost:.2f}"
    else:
        total_cost_text = "unknown"

    print("---")
    print(f"battle_score: {best_summary.score:.4f}")
    print(f"wins: {best_summary.wins}")
    print(f"draws: {best_summary.draws}")
    print(f"losses: {best_summary.losses}")
    print(f"opponents_beaten: {best_summary.opponents_beaten}/{len(frozen_eval.OPPONENT_FILES)}")
    print(f"warrior_length: {best_summary.warrior_length}")
    print(f"model: {args.model}")
    print(f"rounds: {args.rounds}")
    print(f"candidates_per_round: {args.candidates}")
    print(f"workers: {args.workers}")
    print(f"input_tokens: {total_prompt_tokens}")
    print(f"output_tokens: {total_completion_tokens}")
    print(f"estimated_cost_usd: {total_cost_text}")
    print(f"elapsed_seconds: {elapsed:.1f}")


if __name__ == "__main__":
    main()
