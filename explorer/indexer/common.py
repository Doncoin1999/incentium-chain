"""Chain facts the explorer needs: units, protocol accounts, address helpers.

Values mirror runtime/src/configs.rs; keep them in step if the runtime changes.
"""
from __future__ import annotations

import re

UNIT = 10**9                  # 1 ICT = 10^9 base units
BLOCK_SECONDS = 12
DAY = 7_200                   # blocks per day
SS58_PREFIX = 42
LAUNCH_CLIFF = 60 * DAY       # LaunchCliff: locked part fully locked 2 months after the launch period
LAUNCH_STEP = 60 * DAY        # LaunchStep: then 10 % every 2 months
LAUNCH_STEP_PPM = 100_000
PPM = 1_000_000

# PalletId accounts ("modl" + 8-byte id, zero padded to 32 bytes), from runtime/src/configs.rs.
PROTOCOL_PALLETS = {
    b"ict/lnch": "Launch vault",
    b"ict/fund": "Development fund",
    b"ict/spot": "Subnet pot",
    b"ict/stak": "Validator stake (held for stakers)",
    b"ict/work": "Work bonds (held for miners and pools)",
    b"ict/insu": "Insurance fund",
    b"ict/regi": "Subnet registry deposits",
}


def ss58_encode(public_key: bytes) -> str:
    from substrateinterface.utils.ss58 import ss58_encode as enc
    return enc(public_key.hex(), SS58_PREFIX)


def pallet_account(pallet_id: bytes) -> str:
    return ss58_encode((b"modl" + pallet_id).ljust(32, b"\0"))


def protocol_accounts() -> dict[str, str]:
    """address -> label for every protocol (pallet) account."""
    return {pallet_account(pid): label for pid, label in PROTOCOL_PALLETS.items()}


_HEX32 = re.compile(r"^0x[0-9a-fA-F]{64}$")


def normalize_address(text: str) -> str | None:
    """Any SS58 address (any network prefix) or 0x public key -> Incentium address (prefix 42)."""
    from substrateinterface.utils.ss58 import ss58_decode
    text = (text or "").strip()
    if _HEX32.match(text):
        return ss58_encode(bytes.fromhex(text[2:]))
    try:
        return ss58_encode(bytes.fromhex(ss58_decode(text)))
    except Exception:
        return None


def launch_unlocked_ppm(elapsed: int) -> int:
    """Mirror of ict_economics::registry::launch_unlocked_ppm with the runtime's constants."""
    if elapsed < LAUNCH_CLIFF:
        return 0
    steps = (elapsed - LAUNCH_CLIFF) // LAUNCH_STEP + 1
    return min(steps * LAUNCH_STEP_PPM, PPM)


def vault_schedule(locked: int, claimed: int, launch_end: int, now: int) -> dict:
    """Release schedule for one account's launch-vault rewards (all in base units / blocks)."""
    claimable = 0
    if launch_end and now >= launch_end:
        claimable = max(0, locked * launch_unlocked_ppm(now - launch_end) // PPM - claimed)
    releases = []
    for i in range(1, PPM // LAUNCH_STEP_PPM + 1):
        block = launch_end + LAUNCH_CLIFF + (i - 1) * LAUNCH_STEP
        releases.append({"block": block, "pct": i * 10, "unlocked": locked * i * LAUNCH_STEP_PPM // PPM})
    return {"locked": locked, "claimed": claimed, "still_locked": locked - claimed,
            "claimable": claimable, "releases": releases}
