"""The endpoint picker (spec 19.2): register entries first by speed, seeds last, cache, fallback."""
import json

from incentium_neuron import endpoints as E

SEEDS = E.SEEDS["mainnet"]


def test_no_register_gives_reachable_seeds_first():
    def reg(url):
        raise RuntimeError("no such pallet")

    speed = {SEEDS[0]: float("inf"), SEEDS[1]: 0.05}
    assert E.pick("mainnet", register=reg, measure=speed.get) == [SEEDS[1], SEEDS[0]]


def test_register_entries_ordered_by_speed_and_seeds_kept():
    speed = {"wss://a": 0.01, "wss://b": 0.03, "wss://down": float("inf"), SEEDS[0]: 0.02, SEEDS[1]: 0.04}
    out = E.pick("mainnet", register=lambda u: ["wss://b", "wss://down", "wss://a"], measure=speed.get)
    assert out == ["wss://a", SEEDS[0], "wss://b", SEEDS[1]]


def test_the_first_seed_that_answers_is_used_to_read_the_register():
    asked = []

    def reg(url):
        asked.append(url)
        if url == SEEDS[0]:
            raise OSError("down")
        return ["wss://x"]

    out = E.pick("mainnet", register=reg, measure=lambda u: 0.1)
    assert asked == SEEDS
    assert out[0] == "wss://x"


def test_cache_for_an_hour(tmp_path):
    calls = []
    reg = lambda u: calls.append(u) or ["wss://x"]
    t = [1000.0]
    first = E.pick("testnet", home=str(tmp_path), register=reg, measure=lambda u: 0.1, now=lambda: t[0])
    t[0] += 1800
    assert E.pick("testnet", home=str(tmp_path), register=reg, measure=lambda u: 0.1, now=lambda: t[0]) == first
    assert len(calls) == 1
    t[0] += 3600
    E.pick("testnet", home=str(tmp_path), register=reg, measure=lambda u: 0.1, now=lambda: t[0])
    assert len(calls) == 2
    assert json.load(open(tmp_path / "rpc-testnet.json"))["list"] == first
