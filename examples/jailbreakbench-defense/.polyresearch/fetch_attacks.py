#!/usr/bin/env python3
"""Fetch and curate adversarial jailbreak prompts for the example.

This script downloads attack artifacts from the public JailbreakBench artifacts
repository and writes a compact `jbb_attacks.json` file used by `evaluate.py`.

Selection policy:
- 50 prompts from PAIR black-box attacks on `gpt-4-0125-preview`
- 50 prompts from prompt_with_random_search black-box attacks on
  `gpt-4-0125-preview`
- successful prompts on the original target are prioritized first

The resulting file is deterministic and safe to vendor into the example.
"""

from __future__ import annotations

import json
import urllib.request
from pathlib import Path

OUTPUT_PATH = Path(__file__).resolve().parent.parent / "jbb_attacks.json"
USER_AGENT = "polyresearch-example/1.0"

SOURCES = [
    {
        "method": "PAIR",
        "target_model": "gpt-4-0125-preview",
        "count": 50,
        "url": (
            "https://raw.githubusercontent.com/JailbreakBench/artifacts/main/"
            "attack-artifacts/PAIR/black_box/gpt-4-0125-preview.json"
        ),
    },
    {
        "method": "prompt_with_random_search",
        "target_model": "gpt-4-0125-preview",
        "count": 50,
        "url": (
            "https://raw.githubusercontent.com/JailbreakBench/artifacts/main/"
            "attack-artifacts/prompt_with_random_search/black_box/"
            "gpt-4-0125-preview.json"
        ),
    },
]


def fetch_json(url: str) -> dict:
    request = urllib.request.Request(url, headers={"User-Agent": USER_AGENT})
    with urllib.request.urlopen(request) as response:
        return json.loads(response.read())


def select_prompts(source: dict) -> tuple[list[dict], dict]:
    payload = fetch_json(source["url"])
    jailbreaks = payload["jailbreaks"]

    successful = [j for j in jailbreaks if j.get("jailbroken")]
    unsuccessful = [j for j in jailbreaks if not j.get("jailbroken")]
    ordered = successful + unsuccessful
    selected = ordered[: source["count"]]

    attacks = []
    for item in selected:
        attacks.append(
            {
                "goal": item["goal"],
                "prompt": item["prompt"],
                "behavior": item["behavior"],
                "category": item["category"],
                "method": source["method"],
                "target_model": source["target_model"],
                "jailbroken_on_original_target": bool(item.get("jailbroken")),
                "queries_to_jailbreak": item.get("queries_to_jailbreak"),
                "prompt_tokens": item.get("prompt_tokens"),
            }
        )

    stats = {
        "requested": source["count"],
        "selected": len(attacks),
        "successful_available": len(successful),
        "successful_selected": sum(
            1 for attack in attacks if attack["jailbroken_on_original_target"]
        ),
        "source_url": source["url"],
    }
    return attacks, stats


def main() -> None:
    all_attacks: list[dict] = []
    selection_stats: dict[str, dict] = {}

    for source in SOURCES:
        attacks, stats = select_prompts(source)
        all_attacks.extend(attacks)
        selection_stats[source["method"]] = stats

    output = {
        "meta": {
            "description": (
                "Curated adversarial jailbreak prompts from JailbreakBench "
                "artifacts. Successful prompts on the original target are "
                "prioritized first."
            ),
            "selection_stats": selection_stats,
        },
        "attacks": all_attacks,
    }

    OUTPUT_PATH.write_text(json.dumps(output, indent=2) + "\n")

    print(f"Wrote {len(all_attacks)} attacks to {OUTPUT_PATH}")
    for method, stats in selection_stats.items():
        print(
            f"- {method}: {stats['selected']} selected "
            f"({stats['successful_selected']} originally successful)"
        )


if __name__ == "__main__":
    main()
