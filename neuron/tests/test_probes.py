"""Endpoint probes: pass and fail rules, daily scores, one report per day."""
from incentium_neuron.probes import (BLOCKS_PER_DAY, SYSTEM_NUMBER_KEY, Endpoint, ProbeState, Prober,
                                     Reference, http_url, probe)

REF = Reference(number=5000, hash="0xaa", old_number=1, old_hash="0x01")


def fake_rpc(answers):
    """answers: url -> {method: result or Exception}."""
    def rpc(url, method, params, timeout):
        r = answers[url][method]
        if isinstance(r, Exception):
            raise r
        return r
    return rpc


def num(n):
    return "0x" + n.to_bytes(4, "little").hex()


GOOD = {"chain_getBlockHash": "0xaa", "state_getStorage": num(1)}
PRUNED = {"chain_getBlockHash": "0xaa", "state_getStorage": RuntimeError("State already discarded")}
FORK = {"chain_getBlockHash": "0xbb", "state_getStorage": num(1)}
DOWN = {"chain_getBlockHash": OSError("refused"), "state_getStorage": OSError("refused")}


def test_probe_rules():
    rpc = fake_rpc({"a": GOOD, "p": PRUNED, "f": FORK, "d": DOWN})
    assert probe(Endpoint(1, "Rpc", "a"), REF, rpc)
    assert probe(Endpoint(2, "Archive", "a"), REF, rpc)
    assert probe(Endpoint(3, "Rpc", "p"), REF, rpc)           # a pruned node is a fine RPC node
    assert not probe(Endpoint(4, "Archive", "p"), REF, rpc)   # but not an archive
    assert not probe(Endpoint(5, "Rpc", "f"), REF, rpc)       # another chain or fork
    assert not probe(Endpoint(6, "Rpc", "d"), REF, rpc)
    wrong_state = {"chain_getBlockHash": "0xaa", "state_getStorage": num(2)}
    assert not probe(Endpoint(7, "Archive", "w"), REF, fake_rpc({"w": wrong_state}))
    assert SYSTEM_NUMBER_KEY.startswith("0x26aa394eea5630e07c48ae0c9558cef7")


def test_http_url():
    assert http_url("wss://rpc.example.org") == "https://rpc.example.org"
    assert http_url("ws://1.2.3.4:9944") == "http://1.2.3.4:9944"
    assert http_url("https://x") == "https://x"


class FakeChain:
    def __init__(self):
        self.n = 10 * BLOCKS_PER_DAY + 5
        self.eps = [Endpoint(1, "Rpc", "a"), Endpoint(2, "Archive", "p"), Endpoint(3, "Bootnode", "/dns/x")]
        self.reports = []
        self.fail = None

    def block_number(self):
        return self.n

    def endpoints(self):
        return self.eps

    def reference(self):
        return REF

    def report_endpoints(self, day, scores):
        if self.fail:
            raise RuntimeError(self.fail)
        self.reports.append((day, scores))


def test_scores_are_reported_once_the_next_day(tmp_path):
    chain = FakeChain()
    rpc = fake_rpc({"a": GOOD, "p": PRUNED})
    path = str(tmp_path / "s.json")
    p = Prober(chain, ProbeState.load(path), rpc=rpc, log=lambda m: None)
    for _ in range(4):
        p.step()
    assert chain.reports == []                          # nothing from day 9 to report
    # A restart keeps the counts.
    p = Prober(chain, ProbeState.load(path), rpc=rpc, log=lambda m: None)
    assert p.state.days[10] == {1: [4, 4], 2: [0, 4]}  # the bootnode is not probed
    chain.n += BLOCKS_PER_DAY
    p.step()
    p.step()
    assert chain.reports == [(10, [(1, 100), (2, 0)])]  # one report for day 10


def test_report_errors(tmp_path):
    chain = FakeChain()
    p = Prober(chain, ProbeState.load(None), rpc=fake_rpc({"a": GOOD, "p": GOOD}), log=lambda m: None)
    p.step()
    chain.n += BLOCKS_PER_DAY
    chain.fail = "Endpoints.report failed: {'name': 'WrongDay'}"
    p.report_due()
    assert 10 not in p.state.reported                   # retried next round
    chain.fail = None
    p.report_due()
    assert chain.reports == [(10, [(1, 100), (2, 100)])]
    chain.n += BLOCKS_PER_DAY
    p.step()
    chain.n += BLOCKS_PER_DAY
    chain.fail = "NotReporter"
    p.report_due()
    assert 12 in p.state.reported                       # no stake: not retried
