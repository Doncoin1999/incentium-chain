"""Incentium addresses: SS58 format 122, so accounts read `ic...` (the same keys as `5...`)."""
from __future__ import annotations

SS58_FORMAT = 122


def pubkey(addr: str) -> str:
    """Hex public key of an address in any SS58 format, so `ic...` and `5...` of one key compare
    equal. Strings that are not SS58 addresses come back unchanged."""
    if addr.startswith("0x"):
        return addr.lower()
    try:
        from substrateinterface.utils.ss58 import ss58_decode

        return "0x" + ss58_decode(addr)
    except Exception:
        return addr


def to_incentium(addr: str) -> str:
    """The `ic...` form of an address given in any SS58 format."""
    try:
        from substrateinterface.utils.ss58 import ss58_encode

        return ss58_encode(pubkey(addr), SS58_FORMAT)
    except Exception:
        return addr
