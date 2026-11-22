"""Operator switch for single SN0 miners during the launch period.

The mining control board (scripts/mining_control.py) writes a small JSON file next to the
validator's state file; the validator reads it every epoch. A miner that is switched off gets
no tasks and no weight from this validator, so it earns nothing through it. The switch only
works inside the launch window (90 days from the network's first block): after `until` the
validator ignores the file, and the board can no longer change it.

  {"start": 1764000000, "until": 1771776000,
   "blocked": {"5F...hotkey": {"since": 1764100000, "note": "..."}}}
"""
from __future__ import annotations

import json
import os
import time

WINDOW_DAYS = 90


def blocklist_path(state_file: str | None) -> str | None:
    """validator-testnet-state.json -> validator-testnet-blocked.json (same folder)."""
    if not state_file:
        return None
    base = state_file[:-len("-state.json")] if state_file.endswith("-state.json") else os.path.splitext(state_file)[0]
    return base + "-blocked.json"


def stats_path(state_file: str | None) -> str | None:
    if not state_file:
        return None
    base = state_file[:-len("-state.json")] if state_file.endswith("-state.json") else os.path.splitext(state_file)[0]
    return base + "-stats.json"


def load(path: str | None) -> dict:
    """The blocklist file, or an empty one. Never raises."""
    empty = {"start": None, "until": None, "blocked": {}}
    if not path or not os.path.exists(path):
        return empty
    try:
        with open(path) as f:
            d = json.load(f)
    except (OSError, ValueError):
        return empty
    if not isinstance(d, dict) or not isinstance(d.get("blocked", {}), dict):
        return empty
    return {"start": d.get("start"), "until": d.get("until"), "blocked": d.get("blocked", {})}


def window_open(d: dict, now: float | None = None) -> bool:
    until = d.get("until")
    return isinstance(until, (int, float)) and (now if now is not None else time.time()) < until


def blocked_now(path: str | None, now: float | None = None) -> set[str]:
    """Hotkeys switched off right now: empty once the launch window has closed."""
    d = load(path)
    return set(d["blocked"]) if window_open(d, now) else set()


def write_json(path: str, d: dict) -> None:
    """Atomic write, readable by the owner only."""
    os.makedirs(os.path.dirname(os.path.abspath(path)), exist_ok=True)
    tmp = f"{path}.{os.getpid()}.tmp"
    with os.fdopen(os.open(tmp, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600), "w") as f:
        json.dump(d, f, indent=1, sort_keys=True)
    os.replace(tmp, path)
