#!/usr/bin/env python3
"""
Evaluation harness for the pokeragent example.

DO NOT MODIFY - this file is outside the editable surface.
It is the trust boundary described in PREPARE.md.
"""

from __future__ import annotations

import argparse
import importlib.util
import json
import os
import signal
import sys
import time
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any

from dotenv import load_dotenv

load_dotenv()

import litellm
import tiktoken
from pokerkit import Automation, NoLimitTexasHoldem

litellm.suppress_debug_info = True

ROOT = Path(__file__).resolve().parent.parent
SYSTEM_PROMPT_PATH = ROOT / "system_prompt.md"
TOOLS_DIR = ROOT / "tools"
DEALS_PATH = Path(__file__).resolve().parent / "deals.json"

MAX_SYSTEM_PROMPT_TOKENS = 4000
MAX_TOOL_DIR_BYTES = 50 * 1024
MAX_TOOL_CALLS_PER_DECISION = 3
TOOL_TIMEOUT_SECONDS = 2
DEFAULT_MODEL = "gpt-5.4-mini"
MOCK_MODEL = "mock-basic"
SEED = 42
SMALL_BLIND = 1
BIG_BLIND = 2
STARTING_STACK = 200

AUTOMATIONS = (
    Automation.ANTE_POSTING,
    Automation.BET_COLLECTION,
    Automation.BLIND_OR_STRADDLE_POSTING,
    Automation.HOLE_CARDS_SHOWING_OR_MUCKING,
    Automation.HAND_KILLING,
    Automation.CHIPS_PUSHING,
    Automation.CHIPS_PULLING,
)

BASELINE_SYSTEM_PROMPT = """You are playing heads-up no-limit Texas Hold'em poker.

You will receive the game state as a JSON object containing your hole cards,
community cards, pot size, your stack, opponent stack, and the action history.

Choose one action and respond with a JSON object:
- {"action": "fold"}
- {"action": "check_or_call"}
- {"action": "raise", "amount": <total_chips>}

The amount is your total bet for the current betting round. If tools are
available, you may call them before making your decision."""

STREET_NAMES = {
    0: "preflop",
    1: "flop",
    2: "turn",
    3: "river",
}

SEED_SUPPORTED = True


@dataclass
class LoadedTool:
    name: str
    schema: dict[str, Any]
    runner: Any
    source_path: Path


@dataclass
class AgentSpec:
    label: str
    system_prompt: str
    tools: list[LoadedTool] = field(default_factory=list)


@dataclass
class EvalTotals:
    total_bb_delta: float = 0.0
    deal_points: float = 0.0
    deals_evaluated: int = 0
    total_hands: int = 0
    llm_calls: int = 0
    total_cost_usd: float = 0.0


class ToolTimeoutError(RuntimeError):
    pass


def read_required_text(path: Path) -> str:
    if not path.exists():
        print(f"Error: {path} not found.", file=sys.stderr)
        sys.exit(1)
    return path.read_text().strip()


def count_tokens(text: str) -> int:
    try:
        enc = tiktoken.get_encoding("cl100k_base")
        return len(enc.encode(text))
    except Exception:
        return len(text.split())


def validate_system_prompt(text: str) -> int:
    token_count = count_tokens(text)
    if token_count > MAX_SYSTEM_PROMPT_TOKENS:
        print(
            f"Error: system prompt is {token_count} tokens, exceeds limit of "
            f"{MAX_SYSTEM_PROMPT_TOKENS}.",
            file=sys.stderr,
        )
        sys.exit(1)
    return token_count


def load_deals() -> list[dict[str, Any]]:
    if not DEALS_PATH.exists():
        print(f"Error: {DEALS_PATH} not found.", file=sys.stderr)
        sys.exit(1)

    payload = json.loads(DEALS_PATH.read_text())
    deals = payload.get("deals") if isinstance(payload, dict) else payload
    if not isinstance(deals, list) or not deals:
        print("Error: deals.json does not contain a valid deal list.", file=sys.stderr)
        sys.exit(1)

    seen_ids: set[int] = set()
    for deal in deals:
        if not isinstance(deal, dict):
            print("Error: invalid deal entry in deals.json.", file=sys.stderr)
            sys.exit(1)
        deal_id = deal.get("id")
        if not isinstance(deal_id, int) or deal_id in seen_ids:
            print("Error: deal ids must be unique integers.", file=sys.stderr)
            sys.exit(1)
        seen_ids.add(deal_id)
        cards = []
        for key, expected in (("seat1_hole", 2), ("seat2_hole", 2), ("board", 5)):
            value = deal.get(key)
            if not isinstance(value, list) or len(value) != expected:
                print(f"Error: deal {deal_id} has invalid {key}.", file=sys.stderr)
                sys.exit(1)
            cards.extend(value)
        if len(set(cards)) != 9:
            print(f"Error: deal {deal_id} reuses cards.", file=sys.stderr)
            sys.exit(1)

    return deals


def normalize_tool_schema(schema: dict[str, Any], default_name: str) -> dict[str, Any]:
    if schema.get("type") == "function" and isinstance(schema.get("function"), dict):
        function_schema = dict(schema["function"])
    else:
        function_schema = {
            "name": schema.get("name", default_name),
            "description": schema.get("description", ""),
            "parameters": schema.get(
                "parameters",
                {"type": "object", "properties": {}, "additionalProperties": False},
            ),
        }

    if not function_schema.get("name"):
        function_schema["name"] = default_name

    parameters = function_schema.get("parameters")
    if not isinstance(parameters, dict):
        print(
            f"Error: tool schema for {default_name} must define JSON parameters.",
            file=sys.stderr,
        )
        sys.exit(1)

    return {"type": "function", "function": function_schema}


def load_tool_module(path: Path) -> Any:
    module_name = "pokeragent_tool_" + "_".join(path.with_suffix("").parts[-3:])
    spec = importlib.util.spec_from_file_location(module_name, path)
    if spec is None or spec.loader is None:
        print(f"Error: could not load tool module {path}.", file=sys.stderr)
        sys.exit(1)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    runner = getattr(module, "run", None)
    if not callable(runner):
        print(f"Error: tool module {path} must define run(**kwargs).", file=sys.stderr)
        sys.exit(1)
    return runner


def load_tools() -> list[LoadedTool]:
    if not TOOLS_DIR.exists():
        return []

    json_files = sorted(path for path in TOOLS_DIR.rglob("*.json") if path.is_file())
    py_files = sorted(
        path
        for path in TOOLS_DIR.rglob("*.py")
        if path.is_file() and path.name != "__init__.py"
    )
    size_bytes = sum(
        path.stat().st_size
        for path in TOOLS_DIR.rglob("*")
        if path.is_file() and "__pycache__" not in path.parts and path.name != ".gitkeep"
    )
    if size_bytes > MAX_TOOL_DIR_BYTES:
        print(
            f"Error: tools directory is {size_bytes} bytes, exceeds limit of "
            f"{MAX_TOOL_DIR_BYTES}.",
            file=sys.stderr,
        )
        sys.exit(1)

    json_keys = {path.relative_to(TOOLS_DIR).with_suffix("").as_posix() for path in json_files}
    py_keys = {path.relative_to(TOOLS_DIR).with_suffix("").as_posix() for path in py_files}
    missing_py = sorted(json_keys - py_keys)
    missing_json = sorted(py_keys - json_keys)
    if missing_py:
        print(
            "Error: missing tool implementations for schemas: "
            + ", ".join(missing_py),
            file=sys.stderr,
        )
        sys.exit(1)
    if missing_json:
        print(
            "Error: missing tool schemas for implementations: "
            + ", ".join(missing_json),
            file=sys.stderr,
        )
        sys.exit(1)

    loaded_tools: list[LoadedTool] = []
    names: set[str] = set()
    for json_path in json_files:
        if json_path.name == ".gitkeep":
            continue
        raw_schema = json.loads(json_path.read_text())
        tool_schema = normalize_tool_schema(raw_schema, json_path.stem)
        tool_name = tool_schema["function"]["name"]
        if tool_name in names:
            print(f"Error: duplicate tool name {tool_name}.", file=sys.stderr)
            sys.exit(1)
        names.add(tool_name)
        runner = load_tool_module(json_path.with_suffix(".py"))
        loaded_tools.append(
            LoadedTool(
                name=tool_name,
                schema=tool_schema,
                runner=runner,
                source_path=json_path.with_suffix(".py"),
            )
        )

    return loaded_tools


def create_state() -> Any:
    return NoLimitTexasHoldem.create_state(
        AUTOMATIONS,
        True,
        0,
        (SMALL_BLIND, BIG_BLIND),
        BIG_BLIND,
        (STARTING_STACK, STARTING_STACK),
        2,
    )


def stringify_content(content: Any) -> str:
    if content is None:
        return ""
    if isinstance(content, str):
        return content
    if isinstance(content, list):
        parts = []
        for item in content:
            if isinstance(item, dict) and item.get("type") == "text":
                parts.append(item.get("text", ""))
            else:
                parts.append(str(item))
        return "\n".join(parts)
    return str(content)


def extract_first_json_object(text: str) -> str | None:
    start = text.find("{")
    if start == -1:
        return None

    depth = 0
    in_string = False
    escape = False
    for index, char in enumerate(text[start:], start=start):
        if in_string:
            if escape:
                escape = False
            elif char == "\\":
                escape = True
            elif char == '"':
                in_string = False
            continue
        if char == '"':
            in_string = True
        elif char == "{":
            depth += 1
        elif char == "}":
            depth -= 1
            if depth == 0:
                return text[start : index + 1]
    return None


def parse_action_payload(content: str) -> dict[str, Any] | None:
    snippet = extract_first_json_object(content)
    if snippet is None:
        return None

    try:
        parsed = json.loads(snippet)
    except json.JSONDecodeError:
        return None

    if not isinstance(parsed, dict):
        return None

    action = str(parsed.get("action", "")).strip().lower().replace("-", "_")
    if action in {"check", "call", "check_or_call", "check/call"}:
        return {"action": "check_or_call"}
    if action == "fold":
        return {"action": "fold"}
    if action in {"raise", "raise_to", "bet"}:
        try:
            amount = int(float(parsed["amount"]))
        except Exception:
            return None
        return {"action": "raise", "amount": amount}
    return None


def safe_fallback_action(state: Any) -> dict[str, Any]:
    if state.can_check_or_call():
        return {"action": "check_or_call"}
    if state.can_fold():
        return {"action": "fold"}
    if state.can_complete_bet_or_raise_to():
        return {"action": "raise", "amount": state.min_completion_betting_or_raising_to_amount}
    raise RuntimeError("No legal fallback action available.")


def serialize_tool_result(value: Any) -> str:
    try:
        return json.dumps(value, default=str)
    except Exception:
        return json.dumps({"result": str(value)})


def tool_timeout_handler(signum: int, frame: Any) -> None:
    raise ToolTimeoutError("tool timed out")


def execute_tool(tool: LoadedTool, arguments: dict[str, Any]) -> dict[str, Any]:
    previous_handler = signal.getsignal(signal.SIGALRM)
    signal.signal(signal.SIGALRM, tool_timeout_handler)
    signal.setitimer(signal.ITIMER_REAL, TOOL_TIMEOUT_SECONDS)
    try:
        result = tool.runner(**arguments)
        return {"ok": True, "result": result}
    except Exception as exc:
        return {"ok": False, "error": f"{type(exc).__name__}: {exc}"}
    finally:
        signal.setitimer(signal.ITIMER_REAL, 0)
        signal.signal(signal.SIGALRM, previous_handler)


def tool_call_to_dict(tool_call: Any) -> dict[str, Any]:
    function = getattr(tool_call, "function", None)
    if function is None and isinstance(tool_call, dict):
        function = tool_call.get("function", {})
    return {
        "id": getattr(tool_call, "id", None) or tool_call.get("id"),
        "type": "function",
        "function": {
            "name": getattr(function, "name", None)
            or (function or {}).get("name"),
            "arguments": getattr(function, "arguments", None)
            or (function or {}).get("arguments")
            or "{}",
        },
    }


def extract_response_cost(response: Any) -> float:
    hidden = getattr(response, "_hidden_params", None)
    if isinstance(hidden, dict):
        cost = hidden.get("response_cost")
        if cost is not None:
            try:
                return float(cost)
            except Exception:
                return 0.0
    return 0.0


def classify_hole_cards(cards: list[str]) -> tuple[bool, bool, int, int]:
    rank_order = {rank: index for index, rank in enumerate("23456789TJQKA", start=2)}
    ranks = sorted((rank_order[card[0]] for card in cards), reverse=True)
    suited = cards[0][1] == cards[1][1]
    pair = ranks[0] == ranks[1]
    return pair, suited, ranks[0], ranks[1]


def mock_action_from_payload(payload_text: str) -> str:
    payload = json.loads(payload_text)
    legal_actions = {item["action"]: item for item in payload.get("legal_actions", [])}
    hole_cards = payload.get("hole_cards", [])
    pair, suited, high, low = classify_hole_cards(hole_cards)

    should_raise = False
    if "raise" in legal_actions:
        if pair and high >= 10:
            should_raise = True
        elif high == 14 and low >= 11:
            should_raise = True
        elif suited and high >= 13 and low >= 10:
            should_raise = True

    if should_raise:
        return json.dumps(
            {"action": "raise", "amount": legal_actions["raise"]["min_amount"]}
        )

    if "check_or_call" in legal_actions:
        to_call = int(legal_actions["check_or_call"].get("to_call", 0))
        if to_call <= BIG_BLIND or pair or high >= 13:
            return json.dumps({"action": "check_or_call"})

    if "fold" in legal_actions:
        return json.dumps({"action": "fold"})

    return json.dumps({"action": "check_or_call"})


def call_model(model: str, messages: list[dict[str, Any]], tools: list[LoadedTool]) -> tuple[Any, float]:
    global SEED_SUPPORTED

    if model == MOCK_MODEL:
        response = litellm.mock_completion(
            model=DEFAULT_MODEL,
            messages=messages,
            mock_response=mock_action_from_payload(messages[-1]["content"]),
        )
        return response, 0.0

    kwargs: dict[str, Any] = {
        "model": model,
        "messages": messages,
        "temperature": 0,
        "max_tokens": 256,
    }
    if tools:
        kwargs["tools"] = [tool.schema for tool in tools]
        kwargs["tool_choice"] = "auto"
    if SEED_SUPPORTED:
        kwargs["seed"] = SEED

    try:
        response = litellm.completion(**kwargs)
    except Exception as exc:
        if SEED_SUPPORTED and "seed" in str(exc).lower():
            SEED_SUPPORTED = False
            kwargs.pop("seed", None)
            response = litellm.completion(**kwargs)
        else:
            raise
    return response, extract_response_cost(response)


def seat_name(player_index: int) -> str:
    return "big_blind" if player_index == 0 else "small_blind_button"


def build_decision_payload(
    state: Any,
    actor_index: int,
    board_cards: list[str],
    hole_cards: list[list[str]],
    action_history: list[dict[str, Any]],
    tool_names: list[str],
) -> str:
    opponent_index = 1 - actor_index
    legal_actions: list[dict[str, Any]] = []

    if state.can_fold():
        legal_actions.append({"action": "fold"})
    if state.can_check_or_call():
        legal_actions.append(
            {
                "action": "check_or_call",
                "to_call": int(state.checking_or_calling_amount),
            }
        )
    if state.can_complete_bet_or_raise_to():
        legal_actions.append(
            {
                "action": "raise",
                "min_amount": int(state.min_completion_betting_or_raising_to_amount),
                "max_amount": int(state.max_completion_betting_or_raising_to_amount),
            }
        )

    payload = {
        "street": STREET_NAMES.get(state.street_index, "showdown"),
        "seat": seat_name(actor_index),
        "hole_cards": hole_cards[actor_index],
        "community_cards": board_cards,
        "pot_size": int(state.total_pot_amount),
        "player_stack": int(state.stacks[actor_index]),
        "opponent_stack": int(state.stacks[opponent_index]),
        "player_bet_this_round": int(state.bets[actor_index]),
        "opponent_bet_this_round": int(state.bets[opponent_index]),
        "legal_actions": legal_actions,
        "available_tools": tool_names,
        "action_history": action_history,
        "response_contract": {
            "must_return_json_only": True,
            "allowed_shapes": [
                {"action": "fold"},
                {"action": "check_or_call"},
                {"action": "raise", "amount": "integer total bet for this round"},
            ],
        },
    }
    return json.dumps(payload, indent=2)


def choose_action(
    model: str,
    agent: AgentSpec,
    state: Any,
    actor_index: int,
    board_cards: list[str],
    hole_cards: list[list[str]],
    action_history: list[dict[str, Any]],
    totals: EvalTotals,
    deal_id: int,
) -> tuple[dict[str, Any], str | None]:
    messages: list[dict[str, Any]] = [{"role": "system", "content": agent.system_prompt}]
    messages.append(
        {
            "role": "user",
            "content": build_decision_payload(
                state,
                actor_index,
                board_cards,
                hole_cards,
                action_history,
                [tool.name for tool in agent.tools],
            ),
        }
    )
    tool_lookup = {tool.name: tool for tool in agent.tools}
    tool_calls_used = 0

    for _ in range(MAX_TOOL_CALLS_PER_DECISION + 2):
        response, response_cost = call_model(model, messages, agent.tools)
        totals.llm_calls += 1
        totals.total_cost_usd += response_cost

        message = response.choices[0].message
        tool_calls = getattr(message, "tool_calls", None) or []

        if tool_calls:
            tool_call_dicts = [tool_call_to_dict(tool_call) for tool_call in tool_calls]
            messages.append(
                {
                    "role": "assistant",
                    "content": stringify_content(getattr(message, "content", "")),
                    "tool_calls": tool_call_dicts,
                }
            )
            for tool_call in tool_call_dicts:
                tool_name = tool_call["function"]["name"]
                raw_arguments = tool_call["function"]["arguments"] or "{}"
                try:
                    arguments = json.loads(raw_arguments)
                    if not isinstance(arguments, dict):
                        raise TypeError("arguments must decode to an object")
                except Exception as exc:
                    tool_result = {"ok": False, "error": f"Invalid tool arguments: {exc}"}
                else:
                    if tool_calls_used >= MAX_TOOL_CALLS_PER_DECISION:
                        tool_result = {
                            "ok": False,
                            "error": "tool call limit exceeded for this decision",
                        }
                    elif tool_name not in tool_lookup:
                        tool_result = {"ok": False, "error": f"Unknown tool: {tool_name}"}
                    else:
                        tool_result = execute_tool(tool_lookup[tool_name], arguments)
                        tool_calls_used += 1

                messages.append(
                    {
                        "role": "tool",
                        "tool_call_id": tool_call["id"],
                        "name": tool_name,
                        "content": serialize_tool_result(tool_result),
                    }
                )
            continue

        content = stringify_content(getattr(message, "content", ""))
        action = parse_action_payload(content)
        if action is not None:
            return action, None

        warning = (
            f"deal {deal_id} {agent.label} on {STREET_NAMES.get(state.street_index)} "
            "returned invalid JSON action, using fallback"
        )
        return safe_fallback_action(state), warning

    warning = (
        f"deal {deal_id} {agent.label} exceeded tool loop limit, using fallback"
    )
    return safe_fallback_action(state), warning


def apply_action(state: Any, action: dict[str, Any]) -> dict[str, Any]:
    chosen = action["action"]
    if chosen == "fold" and state.can_fold():
        state.fold()
        return {"action": "fold"}

    if chosen == "check_or_call" and state.can_check_or_call():
        state.check_or_call()
        return {"action": "check_or_call"}

    if chosen == "raise" and state.can_complete_bet_or_raise_to():
        min_amount = int(state.min_completion_betting_or_raising_to_amount)
        max_amount = int(state.max_completion_betting_or_raising_to_amount)
        amount = int(action["amount"])
        amount = max(min_amount, min(max_amount, amount))
        state.complete_bet_or_raise_to(amount)
        return {"action": "raise", "amount": amount}

    fallback = safe_fallback_action(state)
    if fallback["action"] == "fold":
        state.fold()
    elif fallback["action"] == "check_or_call":
        state.check_or_call()
    else:
        state.complete_bet_or_raise_to(fallback["amount"])
    fallback["fallback"] = True
    return fallback


def run_hand(
    model: str,
    deal: dict[str, Any],
    seat_agents: list[AgentSpec],
    totals: EvalTotals,
) -> float:
    state = create_state()
    hole_cards = [deal["seat1_hole"], deal["seat2_hole"]]
    state.deal_hole("".join(hole_cards[0]))
    state.deal_hole("".join(hole_cards[1]))

    action_history: list[dict[str, Any]] = []
    board_cards: list[str] = []
    board_offset = 0

    while state.status:
        if state.actor_index is not None:
            actor_index = int(state.actor_index)
            agent = seat_agents[actor_index]
            action, warning = choose_action(
                model,
                agent,
                state,
                actor_index,
                board_cards,
                hole_cards,
                action_history,
                totals,
                int(deal["id"]),
            )
            if warning:
                print(warning, file=sys.stderr)
            applied = apply_action(state, action)
            action_history.append(
                {
                    "street": STREET_NAMES.get(state.street_index, "showdown"),
                    "agent": agent.label,
                    "seat": seat_name(actor_index),
                    **applied,
                }
            )
            continue

        if state.can_burn_card():
            state.burn_card("??")
            continue

        if state.can_deal_board():
            if board_offset == 0:
                chunk = deal["board"][0:3]
                board_offset = 3
            else:
                chunk = [deal["board"][board_offset]]
                board_offset += 1
            board_cards.extend(chunk)
            state.deal_board("".join(chunk))
            continue

        if state.can_no_operate():
            state.no_operate()
            continue

        raise RuntimeError(
            f"Stuck while running deal {deal['id']} on street {state.street_index}."
        )

    candidate_index = 0 if seat_agents[0].label == "candidate" else 1
    return float(state.payoffs[candidate_index]) / BIG_BLIND


def evaluate_deals(
    model: str,
    candidate: AgentSpec,
    baseline: AgentSpec,
    deals: list[dict[str, Any]],
) -> EvalTotals:
    totals = EvalTotals()

    for index, deal in enumerate(deals, start=1):
        first = run_hand(model, deal, [candidate, baseline], totals)
        second = run_hand(model, deal, [baseline, candidate], totals)
        deal_delta = first + second

        totals.total_bb_delta += deal_delta
        totals.deals_evaluated += 1
        totals.total_hands += 2
        if deal_delta > 0:
            totals.deal_points += 1.0
        elif deal_delta == 0:
            totals.deal_points += 0.5

        print(
            f"[{index}/{len(deals)}] deal {deal['id']} avg_bb_delta={deal_delta:.4f}",
            file=sys.stderr,
        )

    return totals


def main() -> None:
    parser = argparse.ArgumentParser(description="Pokeragent duplicate-deal evaluator")
    parser.add_argument(
        "--model",
        default=os.environ.get("EVAL_MODEL", DEFAULT_MODEL),
        help="Model to evaluate (litellm model string)",
    )
    parser.add_argument(
        "--max-deals",
        type=int,
        default=None,
        help="Optional cap for debugging smaller runs",
    )
    args = parser.parse_args()

    candidate_prompt = read_required_text(SYSTEM_PROMPT_PATH)
    system_prompt_tokens = validate_system_prompt(candidate_prompt)
    tools = load_tools()
    deals = load_deals()
    if args.max_deals is not None:
        deals = deals[: args.max_deals]

    candidate = AgentSpec(label="candidate", system_prompt=candidate_prompt, tools=tools)
    baseline = AgentSpec(label="baseline", system_prompt=BASELINE_SYSTEM_PROMPT, tools=[])

    print(f"Loaded {len(deals)} frozen deals.", file=sys.stderr)
    print(f"Model: {args.model}", file=sys.stderr)
    print(f"Candidate prompt: {system_prompt_tokens} tokens", file=sys.stderr)
    print(f"Tools loaded: {len(tools)}", file=sys.stderr)
    print(file=sys.stderr)

    start_time = time.time()
    try:
        totals = evaluate_deals(args.model, candidate, baseline, deals)
    except Exception as exc:
        print(f"Evaluation failed: {type(exc).__name__}: {exc}", file=sys.stderr)
        raise

    elapsed = time.time() - start_time
    avg_bb_per_deal = (
        totals.total_bb_delta / totals.deals_evaluated if totals.deals_evaluated else 0.0
    )
    avg_bb_per_deal = max(-100.0, min(100.0, avg_bb_per_deal))
    poker_score = (avg_bb_per_deal + 100.0) / 200.0
    deal_win_rate = (
        totals.deal_points / totals.deals_evaluated if totals.deals_evaluated else 0.0
    )

    print("---")
    print(f"poker_score: {poker_score:.4f}")
    print(f"avg_bb_per_deal: {avg_bb_per_deal:.4f}")
    print(f"deal_win_rate: {deal_win_rate:.4f}")
    print(f"deals_evaluated: {totals.deals_evaluated}")
    print(f"total_hands: {totals.total_hands}")
    print(f"model: {args.model}")
    print(f"system_prompt_tokens: {system_prompt_tokens}")
    print(f"tools_loaded: {len(tools)}")
    print(f"llm_calls: {totals.llm_calls}")
    print(f"cost_usd: {totals.total_cost_usd:.2f}")
    print(f"elapsed_seconds: {elapsed:.1f}")


if __name__ == "__main__":
    main()
