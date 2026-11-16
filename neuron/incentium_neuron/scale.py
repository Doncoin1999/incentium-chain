"""Minimal SCALE encoding and the weights commit hash, matching pallet-weights.

commit_hash = blake2_256(SCALE((netuid: u16, uids: Vec<u16>, weights: Vec<u16>, salt: [u8; 32])))
"""
from __future__ import annotations

import hashlib
import struct


def compact(n: int) -> bytes:
    """SCALE compact integer encoding."""
    if n < 1 << 6:
        return bytes([n << 2])
    if n < 1 << 14:
        return struct.pack("<H", (n << 2) | 0b01)
    if n < 1 << 30:
        return struct.pack("<I", (n << 2) | 0b10)
    raw = n.to_bytes((n.bit_length() + 7) // 8, "little")
    return bytes([((len(raw) - 4) << 2) | 0b11]) + raw


def u16(n: int) -> bytes:
    return struct.pack("<H", n)


def vec_u16(xs: list[int]) -> bytes:
    return compact(len(xs)) + b"".join(u16(x) for x in xs)


def commit_hash(netuid: int, uids: list[int], weights: list[int], salt: bytes) -> bytes:
    if len(salt) != 32:
        raise ValueError("salt must be 32 bytes")
    payload = u16(netuid) + vec_u16(uids) + vec_u16(weights) + salt
    return hashlib.blake2b(payload, digest_size=32).digest()
