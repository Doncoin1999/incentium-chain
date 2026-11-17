import socket

from incentium_neuron.backend import MockBackend
from incentium_neuron.miner import serve
from incentium_neuron.scale import commit_hash, compact
from incentium_neuron.scoring import MinerStats, logprob_distance, passes_spot_check, to_weights
from incentium_neuron.validator import Validator


def test_commit_hash_matches_runtime():
    # same vector as pallet-weights `commit_hash_test_vector`
    h = commit_hash(0, [1, 2], [3, 1], bytes([7] * 32))
    assert h.hex() == "cd1d13b27f1aa27e8b3635619fdd1181413a77f391b43b65abacd699d1296b92"


def test_compact_encoding():
    assert compact(0) == b"\x00"
    assert compact(63) == b"\xfc"
    assert compact(64) == b"\x01\x01"
    assert compact(16384) == b"\x02\x00\x01\x00"


def test_spot_check_honest_noise_passes_cheat_fails():
    ref = MockBackend().complete("p", 16, 1)
    honest = MockBackend(noise=0.05).complete("p", 16, 1)
    cheat = MockBackend(cheat=True).complete("p", 16, 1)
    assert passes_spot_check(1, honest.top_logprobs, ref.top_logprobs)
    assert not passes_spot_check(1, cheat.top_logprobs, ref.top_logprobs)
    assert logprob_distance([], ref.top_logprobs) == float("inf")


def test_weights_scale_to_u16():
    a = MinerStats(uid=2, tokens=300, requests=10, answered=10)
    b = MinerStats(uid=3, tokens=100, requests=10, answered=5)
    uids, w = to_weights([a, b])
    assert uids == [2, 3]
    assert w[0] == 65535 and abs(w[1] - round(65535 / 6)) <= 1


def free_port():
    s = socket.socket()
    s.bind(("127.0.0.1", 0))
    p = s.getsockname()[1]
    s.close()
    return p


class FakeChain:
    def __init__(self, miners):
        self.epoch = 0
        self._miners = miners
        self.commits, self.reveals, self.reports = [], [], []

    def current_epoch(self):
        return self.epoch

    def miners(self, netuid):
        return self._miners

    def commit_weights(self, netuid, h):
        self.commits.append((self.epoch, h))

    def reveal_weights(self, netuid, uids, weights, salt):
        self.reveals.append((self.epoch, uids, weights, salt))

    def report_failed_check(self, epoch, uid, evidence):
        self.reports.append((epoch, uid))


def test_validator_loop_against_live_miners():
    ports = [free_port() for _ in range(3)]
    servers = [
        serve(MockBackend(noise=0.05), "honest-a", "127.0.0.1", ports[0], background=True),
        serve(MockBackend(noise=0.05), "honest-b", "127.0.0.1", ports[1], background=True),
        serve(MockBackend(cheat=True), "cheater", "127.0.0.1", ports[2], background=True),
    ]
    try:
        miners = [(0, "validator", ""), (1, "honest-a", f"http://127.0.0.1:{ports[0]}"),
                  (2, "honest-b", f"http://127.0.0.1:{ports[1]}"), (3, "cheater", f"http://127.0.0.1:{ports[2]}")]
        chain = FakeChain(miners)
        v = Validator(chain=chain, reference=MockBackend(), own_uid=0, spot_check_share=1.0, prompts_per_miner=4)
        v.step()  # epoch 0: measure + commit
        assert len(chain.commits) == 1
        assert (0, 3) in chain.reports  # the cheater is reported
        committed = v.pending
        assert 3 not in committed.uids  # and gets no weight
        assert committed.uids == [1, 2]
        # the hash committed is the one the reveal will match
        assert chain.commits[0][1] == commit_hash(0, committed.uids, committed.weights, committed.salt)
        chain.epoch = 1
        v.step()  # epoch 1: reveal epoch 0, commit epoch 1
        assert chain.reveals[0][1:3] == (committed.uids, committed.weights)
        assert len(chain.commits) == 2
    finally:
        for s in servers:
            s.shutdown()


def test_validator_uses_chain_tiers_and_skips_other_tiers():
    from incentium_neuron.validator import TierInfo

    port = free_port()
    srv = serve(MockBackend(noise=0.05), "honest", "127.0.0.1", port, background=True)
    try:
        url = f"http://127.0.0.1:{port}"
        chain = FakeChain([(0, "validator", "", 0), (1, "two-b", url, 0), (2, "nine-b", url, 1)])
        chain.tiers = lambda: {0: TierInfo(0.5, 0.35, "Qwen3.5-2B", "00" * 32, True)}
        v = Validator(chain=chain, reference=MockBackend(), own_uid=0, tier=0,
                      spot_check_share=1.0, prompts_per_miner=2)
        stats = v.measure(0)
        assert [s.uid for s in stats] == [1]  # the tier-1 miner is left to tier-1 validators
        assert stats[0].g == 0.5 and stats[0].tier == 0
        assert any("uid 2: tier 1" in line for line in v.log)
        assert stats[0].work_units() == stats[0].tokens * 0.5
    finally:
        srv.shutdown()
