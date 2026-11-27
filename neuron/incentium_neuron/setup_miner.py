"""Helpers for the one-command miner install (scripts/install-miner.sh).

  python -m incentium_neuron.setup_miner keygen ~/.incentium-miner/miner-key.json
  python -m incentium_neuron.setup_miner register --key-file ... --service https://register.incentium.org
  python -m incentium_neuron.setup_miner tier0 --url wss://testnet-rpc.incentium.org   # model name and locked hash
  python -m incentium_neuron.setup_miner choose --url wss://testnet-rpc.incentium.org  # tier and model for this computer
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


def register(key_file: str, service: str, timeout: float = 60, contact: dict | None = None) -> tuple[int, dict]:
    """Ask the registration service to register this hotkey. `contact` (name, email, note) goes
    along for services that approve requests by hand; others ignore it."""
    from substrateinterface import Keypair

    kp = Keypair.create_from_mnemonic(json.load(open(os.path.expanduser(key_file)))["secretPhrase"])
    base = service.rstrip("/")
    with urllib.request.urlopen(base + "/", timeout=timeout) as r:
        info = json.loads(r.read())
    msg = registration_message(info["genesis"], kp.ss58_address)
    body = json.dumps({"hotkey": kp.ss58_address, "signature": kp.sign(msg).hex(), **({"contact": contact} if contact else {})}).encode()
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


def choose(url: str, cpu_only: bool = False) -> dict:
    """The tier a Linux miner should run: the largest open on chain that this computer runs well
    with the model taking at most a quarter of its memory. `cpu_only` when no GPU build of
    llama.cpp could be installed."""
    from . import hw as HW
    from .validator import SubstrateChain

    h = HW.detect()
    if cpu_only:
        h.vram_gb, h.gpu = 0.0, ""
    ok, why = h.can_run_at_all()
    if not ok:
        return {"ok": False, "reason": why}
    tiers = SubstrateChain(url, None).tiers()
    open_tiers = {t for t, i in tiers.items() if i.open and t in HW.CATALOG}
    tier = HW.choose_tier(h, open_tiers or {0})
    m = HW.CATALOG[tier]
    info = tiers.get(tier)
    return {
        "ok": True,
        "tier": tier,
        "model": m["model"],
        "url": m["url"],
        "file": m["url"].rsplit("/", 1)[1],
        "size_gb": m["size_gb"],
        "model_hash": (info.model_hash if info and info.model_hash.strip("0") else ""),
        "gpu_layers": h.gpu_layers(tier),
        "max_tier": h.max_tier(),
        "open_tiers": sorted(open_tiers),
        "hardware": f"{h.cpu}, {h.ram_gb:.0f} GB memory" + (f", {h.gpu} {h.vram_gb:.0f} GB" if h.gpu else ""),
    }


def main():
    ap = argparse.ArgumentParser()
    sub = ap.add_subparsers(dest="cmd", required=True)
    k = sub.add_parser("keygen")
    k.add_argument("path")
    r = sub.add_parser("register")
    r.add_argument("--key-file", required=True)
    r.add_argument("--service", default="https://register.incentium.org")
    t = sub.add_parser("tier0")
    t.add_argument("--url", default="wss://testnet-rpc.incentium.org")
    c = sub.add_parser("choose")
    c.add_argument("--url", default="wss://testnet-rpc.incentium.org")
    c.add_argument("--cpu-only", action="store_true")
    a = ap.parse_args()
    if a.cmd == "keygen":
        print(keygen(a.path))
    elif a.cmd == "register":
        code, body = register(a.key_file, a.service)
        print(json.dumps(body))
        sys.exit(0 if code == 200 else 1)
    elif a.cmd == "choose":
        out = choose(a.url, a.cpu_only)
        print(json.dumps(out))
        sys.exit(0 if out["ok"] else 1)
    else:
        print(json.dumps(tier0(a.url)))


if __name__ == "__main__":
    main()
