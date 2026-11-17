"""Helpers for the one-command miner install (scripts/install-miner.sh).

  python -m incentium_neuron.setup_miner keygen ~/.incentium-miner/miner-key.json
  python -m incentium_neuron.setup_miner register --key-file ... --service https://register.incentium.org
  python -m incentium_neuron.setup_miner tier0 --url wss://rpc.incentium.org   # model name and locked hash
"""
from __future__ import annotations

import argparse
import json
import os
import sys
import urllib.error
import urllib.request


def registration_message(genesis_hash: str, hotkey: str) -> bytes:
    """What a miner signs to ask the launch operator to register its hotkey. The genesis hash
    ties the request to one network, so it cannot be replayed on another."""
    return f"incentium-register:{genesis_hash.lower()}:{hotkey}".encode()


def keygen(path: str) -> str:
    from substrateinterface import Keypair

    path = os.path.expanduser(path)
    if os.path.exists(path):
        from .addr import to_incentium

        return to_incentium(json.load(open(path))["ss58Address"])
    os.makedirs(os.path.dirname(path), exist_ok=True)
    from .addr import SS58_FORMAT

    phrase = Keypair.generate_mnemonic()
    kp = Keypair.create_from_mnemonic(phrase, ss58_format=SS58_FORMAT)
    with os.fdopen(os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600), "w") as f:
        json.dump({"secretPhrase": phrase, "ss58Address": kp.ss58_address}, f)
    return kp.ss58_address


def register(key_file: str, service: str, timeout: float = 60) -> tuple[int, dict]:
    from substrateinterface import Keypair

    kp = Keypair.create_from_mnemonic(json.load(open(os.path.expanduser(key_file)))["secretPhrase"])
    base = service.rstrip("/")
    with urllib.request.urlopen(base + "/", timeout=timeout) as r:
        info = json.loads(r.read())
    msg = registration_message(info["genesis"], kp.ss58_address)
    body = json.dumps({"hotkey": kp.ss58_address, "signature": kp.sign(msg).hex()}).encode()
    req = urllib.request.Request(base + "/register", data=body, headers={"Content-Type": "application/json"})
    try:
        with urllib.request.urlopen(req, timeout=timeout) as r:
            return r.status, json.loads(r.read())
    except urllib.error.HTTPError as e:
        return e.code, json.loads(e.read() or b"{}")


def tier0(url: str) -> dict:
    from .validator import SubstrateChain

    t = SubstrateChain(url, None).tiers()[0]
    return {"model": t.model, "model_hash": t.model_hash.strip("0") and t.model_hash, "open": t.open}


def main():
    ap = argparse.ArgumentParser()
    sub = ap.add_subparsers(dest="cmd", required=True)
    k = sub.add_parser("keygen")
    k.add_argument("path")
    r = sub.add_parser("register")
    r.add_argument("--key-file", required=True)
    r.add_argument("--service", default="https://register.incentium.org")
    t = sub.add_parser("tier0")
    t.add_argument("--url", default="wss://rpc.incentium.org")
    a = ap.parse_args()
    if a.cmd == "keygen":
        print(keygen(a.path))
    elif a.cmd == "register":
        code, body = register(a.key_file, a.service)
        print(json.dumps(body))
        sys.exit(0 if code == 200 else 1)
    else:
        print(json.dumps(tier0(a.url)))


if __name__ == "__main__":
    main()
