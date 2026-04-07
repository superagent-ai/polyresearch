#!/usr/bin/env python3
"""
Generate the frozen duplicate-deal set for the pokeragent example.

This script is not part of the editable surface. It exists so the checked-in
deals.json can be audited or regenerated deterministically.
"""

from __future__ import annotations

import json
import random
from pathlib import Path


ROOT = Path(__file__).resolve().parent.parent
OUTPUT_PATH = ROOT / "deals.json"
SEED = 42
DEAL_COUNT = 200
RANKS = "23456789TJQKA"
SUITS = "cdhs"
CARDS = [rank + suit for rank in RANKS for suit in SUITS]


def generate_deal(rng: random.Random, deal_id: int) -> dict[str, object]:
    cards = rng.sample(CARDS, 9)
    return {
        "id": deal_id,
        "seat1_hole": cards[0:2],
        "seat2_hole": cards[2:4],
        "board": cards[4:9],
    }


def main() -> None:
    rng = random.Random(SEED)
    payload = {
        "seed": SEED,
        "deal_count": DEAL_COUNT,
        "deals": [generate_deal(rng, deal_id) for deal_id in range(1, DEAL_COUNT + 1)],
    }
    OUTPUT_PATH.write_text(json.dumps(payload, indent=2) + "\n")
    print(f"Wrote {DEAL_COUNT} deals to {OUTPUT_PATH}")


if __name__ == "__main__":
    main()
