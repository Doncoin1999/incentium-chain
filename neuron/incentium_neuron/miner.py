"""SN0 reference miner (pull mode): fetches tasks from validators and posts signed answers.

The miner only makes outgoing requests, so it runs behind a home router with no open port,
no fixed IP and no endpoint on chain. It finds validators' task servers on chain (staked
neurons with an endpoint) or takes them from --validator.

Run:
  python -m incentium_neuron.miner --backend http://127.0.0.1:8080 --model qwen3.5-2b --key //Charlie
"""
from __future__ import annotations

import argparse
import json
import time
import urllib.parse
import urllib.request
from typing import Callable

from .tasks import result_message

POLL_SECONDS = 5.0
MAX_TOKENS = 512


def _get(url: str, timeout: float) -> dict:
    with urllib.request.urlopen(url, timeout=timeout) as r:
        return json.loads(r.read())


def _post(url: str, obj: dict, timeout: float) -> int:
    req = urllib.request.Request(url, data=json.dumps(obj).encode(), headers={"Content-Type": "application/json"})
    try:
        with urllib.request.urlopen(req, timeout=timeout) as r:
            return r.status
    except urllib.error.HTTPError as e:
        return e.code


class Miner:
    def __init__(self, backend, hotkey: str, sign: Callable[[bytes], str], timeout: float = 30.0):
        self.backend = backend
        self.hotkey = hotkey
        self.sign = sign  # bytes -> hex signature by the hotkey
        self.timeout = timeout
        self.done: dict[str, int] = {}  # task_id -> status of our post

    def work_once(self, validator_url: str) -> int:
        """Fetch open tasks from one validator and answer them. Returns tasks answered."""
        base = validator_url.rstrip("/")
        q = urllib.parse.urlencode({"hotkey": self.hotkey})
        try:
            board = _get(f"{base}/sn0/tasks?{q}", self.timeout)
        except Exception:
            return 0
        n = 0
        for t in board.get("tasks", []):
            tid = t["task_id"]
            if tid in self.done:
                continue
            if board.get("deadline") and time.time() > board["deadline"]:
                break
            c = self.backend.complete(str(t["prompt"]), min(int(t["max_tokens"]), MAX_TOKENS), int(t["seed"]))
            sig = self.sign(result_message(tid, c.text, c.tokens, c.top_logprobs))
            status = _post(f"{base}/sn0/result", {"task_id": tid, "hotkey": self.hotkey, "text": c.text,
                                                  "tokens": c.tokens, "top_logprobs": c.top_logprobs,
                                                  "signature": sig}, self.timeout)
            self.done[tid] = status
            n += status == 200
        if len(self.done) > 10_000:
            self.done.clear()
        return n


def main():
    from substrateinterface import Keypair

    from .backend import OpenAICompatibleBackend

    ap = argparse.ArgumentParser()
    ap.add_argument("--backend", required=True, help="OpenAI-compatible base URL of the local model")
    ap.add_argument("--model", required=True)
    ap.add_argument("--key", required=True, help="secret URI or mnemonic of this miner's hotkey")
    ap.add_argument("--url", default="ws://127.0.0.1:9944", help="chain RPC, to find validators")
    ap.add_argument("--validator", action="append", default=[],
                    help="validator task-server URL (repeatable); default: found on chain")
    a = ap.parse_args()
    kp = Keypair.create_from_uri(a.key)
    miner = Miner(OpenAICompatibleBackend(a.backend, a.model), kp.ss58_address, lambda m: kp.sign(m).hex())
    print(f"miner {kp.ss58_address} started", flush=True)
    chain, found, last_lookup = None, list(a.validator), 0.0
    while True:
        if not a.validator and time.time() - last_lookup > 300:
            try:
                from .validator import SN0, SubstrateChain

                chain = chain or SubstrateChain(a.url, None)
                found = chain.validator_endpoints(SN0)
                print(f"validators: {found}", flush=True)
            except Exception as e:
                print(f"could not read validators from chain: {e!r}", flush=True)
            last_lookup = time.time()
        for v in found:
            n = miner.work_once(v)
            if n:
                print(f"answered {n} tasks for {v}", flush=True)
        time.sleep(POLL_SECONDS)


if __name__ == "__main__":
    main()
