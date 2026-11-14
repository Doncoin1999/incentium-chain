"""Inference backends. Production: an OpenAI-compatible server (llama.cpp `llama-server`
for tier 1, vLLM for tiers 2-3) serving the pinned Qwen3.5 weights. Tests: MockBackend."""
from __future__ import annotations

import json
import random
import urllib.request
from dataclasses import dataclass


@dataclass
class Completion:
    text: str
    tokens: int
    top_logprobs: list[dict[str, float]]  # per output position: {token: logprob}


class OpenAICompatibleBackend:
    def __init__(self, base_url: str, model: str, top_k: int = 5, timeout: float = 60.0):
        self.base_url = base_url.rstrip("/")
        self.model = model
        self.top_k = top_k
        self.timeout = timeout

    def complete(self, prompt: str, max_tokens: int, seed: int) -> Completion:
        body = {
            "model": self.model,
            "prompt": prompt,
            "max_tokens": max_tokens,
            "temperature": 0,
            "seed": seed,
            "logprobs": self.top_k,
        }
        req = urllib.request.Request(
            self.base_url + "/v1/completions",
            data=json.dumps(body).encode(),
            headers={"Content-Type": "application/json"},
        )
        with urllib.request.urlopen(req, timeout=self.timeout) as r:
            data = json.loads(r.read())
        choice = data["choices"][0]
        lp = (choice.get("logprobs") or {}).get("top_logprobs") or []
        return Completion(
            text=choice["text"],
            tokens=data.get("usage", {}).get("completion_tokens", len(lp)),
            top_logprobs=[dict(x) for x in lp],
        )


class MockBackend:
    """Deterministic fake model: output depends only on (prompt, seed).
    `noise` perturbs log-probabilities (honest GPU variation); `cheat` swaps in a
    different 'model' (what a dishonest miner running a smaller model would return)."""

    def __init__(self, noise: float = 0.0, cheat: bool = False):
        self.noise = noise
        self.cheat = cheat

    def complete(self, prompt: str, max_tokens: int, seed: int) -> Completion:
        rng = random.Random(f"{prompt}|{seed}|{'cheat' if self.cheat else 'ref'}")
        jitter = random.Random()
        positions = []
        words = []
        for _ in range(max_tokens):
            toks = [f"t{rng.randrange(1000)}" for _ in range(5)]
            lps = sorted((-rng.random() * 5 for _ in toks), reverse=True)
            positions.append({t: lp + jitter.uniform(-self.noise, self.noise) for t, lp in zip(toks, lps)})
            words.append(toks[0])
        return Completion(text=" ".join(words), tokens=max_tokens, top_logprobs=positions)
