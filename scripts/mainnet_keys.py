#!/usr/bin/env python3
"""Key bookkeeping for the mainnet launch (used by mainnet-node.sh and mainnet-genesis.sh).

Standard library only, so it runs on a fresh server.

  mainnet_keys.py public  ROLE SECRETS IP PEER_V1 PEER_V2 PEER_RPC OUT
      Write the public half of one validator server (no secrets): its two Aura/Grandpa
      authorities and its p2p addresses.
  mainnet_keys.py genesis SECRETS PUBLIC_A PUBLIC_B OUT
      Combine both servers into keys.json for make_bootstrap_spec.py: four authorities, the
      operator account and the curator, a 2-of-3 multisig of curator1..3 (whitepaper 5.1).
"""
import hashlib
import json
import sys

ALPHABET = "123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz"
SS58_PREFIX = 42


def b58encode(data: bytes) -> str:
    n = int.from_bytes(data, "big")
    out = ""
    while n:
        n, r = divmod(n, 58)
        out = ALPHABET[r] + out
    return "1" * (len(data) - len(data.lstrip(b"\0"))) + out


def b58decode(s: str) -> bytes:
    n = 0
    for c in s:
        n = n * 58 + ALPHABET.index(c)
    body = n.to_bytes((n.bit_length() + 7) // 8, "big") if n else b""
    return b"\0" * (len(s) - len(s.lstrip("1"))) + body


def _checksum(data: bytes) -> bytes:
    return hashlib.blake2b(b"SS58PRE" + data, digest_size=64).digest()[:2]


def ss58_encode(pubkey: bytes, prefix: int = SS58_PREFIX) -> str:
    assert len(pubkey) == 32 and prefix < 64
    data = bytes([prefix]) + pubkey
    return b58encode(data + _checksum(data))


def ss58_decode(address: str) -> bytes:
    raw = b58decode(address)
    if len(raw) != 35 or raw[0] >= 64:
        raise ValueError(f"not a 32-byte SS58 address: {address}")
    if _checksum(raw[:33]) != raw[33:]:
        raise ValueError(f"bad SS58 checksum: {address}")
    return raw[1:33]


def multisig(addresses: list, threshold: int) -> str:
    """pallet_multisig::multi_account_id: blake2_256(("modlpy/utilisuba", sorted, threshold))."""
    keys = sorted(ss58_decode(a) for a in addresses)
    assert 0 < threshold <= len(keys) < 64
    data = b"modlpy/utilisuba" + bytes([len(keys) << 2]) + b"".join(keys) + threshold.to_bytes(2, "little")
    return ss58_encode(hashlib.blake2b(data, digest_size=32).digest())


def address(secrets: str, name: str) -> str:
    return json.load(open(f"{secrets}/{name}.json"))["ss58Address"]


def public(role, secrets, ip, peer_v1, peer_v2, peer_rpc) -> dict:
    return {
        "role": role,
        "ip": ip,
        "authorities": [
            {"aura": address(secrets, f"aura{i}"), "grandpa": address(secrets, f"gran{i}")} for i in (1, 2)
        ],
        "bootnodes": [
            f"/ip4/{ip}/tcp/30333/p2p/{peer_v1}",
            f"/ip4/{ip}/tcp/30334/p2p/{peer_v2}",
            f"/ip4/{ip}/tcp/30335/p2p/{peer_rpc}",
        ],
    }


def genesis(secrets, a: dict, b: dict) -> dict:
    curators = [address(secrets, f"curator{i}") for i in (1, 2, 3)]
    auths = a["authorities"] + b["authorities"]
    seen = [x["aura"] for x in auths] + [x["grandpa"] for x in auths]
    if len(set(seen)) != len(seen):
        raise SystemExit("the same authority key appears twice; were both servers set up with one secrets folder?")
    return {
        "operator": address(secrets, "operator"),
        "authorities": auths,
        "curator": multisig(curators, 2),
        "curator_signatories": curators,
        "bootnodes": a["bootnodes"][:1] + b["bootnodes"][:1],
    }


def main(argv):
    if argv[:1] == ["public"] and len(argv) == 8:
        _, role, secrets, ip, p1, p2, pr, out = argv
        json.dump(public(role, secrets, ip, p1, p2, pr), open(out, "w"), indent=2)
    elif argv[:1] == ["genesis"] and len(argv) == 5:
        _, secrets, pa, pb, out = argv
        keys = genesis(secrets, json.load(open(pa)), json.load(open(pb)))
        json.dump(keys, open(out, "w"), indent=2)
        print(f"curator (2-of-3 multisig): {keys['curator']}")
    else:
        sys.exit(__doc__)


if __name__ == "__main__":
    main(sys.argv[1:])
