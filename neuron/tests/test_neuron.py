import hashlib
import json
import socket
import time
import urllib.request

from incentium_neuron.backend import MockBackend
from incentium_neuron.miner import Miner
from incentium_neuron.scale import commit_hash, compact
from incentium_neuron.scoring import MinerStats, logprob_distance, passes_spot_check, to_weights
from incentium_neuron.tasks import Task, TaskBoard, result_message, serve_tasks
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


def fake_sign(hotkey):
    return lambda m: hashlib.sha256(hotkey.encode() + m).hexdigest()


def fake_verify(hotkey, m, sig):
    return fake_sign(hotkey)(m) == sig


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


def make_validator(chain, miners_and_backends, **kw):
    """Validator with a live task server; during the collect window the miners pull and answer."""
    board = TaskBoard(verify=fake_verify)
    port = free_port()
    srv = serve_tasks(board, "127.0.0.1", port)
    url = f"http://127.0.0.1:{port}"
    workers = [Miner(b, hk, fake_sign(hk)) for hk, b in miners_and_backends]
    v = Validator(chain=chain, reference=MockBackend(), own_uid=0, board=board, spot_check_share=1.0,
                  prompts_per_miner=4, collect_seconds=30,
                  sleep=lambda _s: [w.work_once(url) for w in workers], **kw)
    return v, srv, url


def test_validator_loop_with_pulling_miners():
    miners = [(0, "validator", ""), (1, "honest-a", ""), (2, "honest-b", ""), (3, "cheater", ""), (4, "offline", "")]
    chain = FakeChain(miners)
    v, srv, _ = make_validator(chain, [("honest-a", MockBackend(noise=0.05)), ("honest-b", MockBackend(noise=0.05)),
                                       ("cheater", MockBackend(cheat=True))])
    try:
        v.step()  # epoch 0: publish, miners pull and answer, score, commit
        assert len(chain.commits) == 1
        assert (0, 3) in chain.reports  # the cheater is reported
        committed = v.pending
        assert committed.uids == [1, 2]  # no weight for the cheater or the offline miner
        assert any("uid 4: answered 0/4" in line for line in v.log)
        assert chain.commits[0][1] == commit_hash(0, committed.uids, committed.weights, committed.salt)
        chain.epoch = 1
        v.step()  # epoch 1: reveal epoch 0, commit epoch 1
        assert chain.reveals[0][1:3] == (committed.uids, committed.weights)
        assert len(chain.commits) == 2
    finally:
        srv.shutdown()


def test_task_server_rejects_forged_duplicate_and_late_results():
    board = TaskBoard(verify=fake_verify)
    board.publish(5, time.time() + 60, [Task("t1", "alice", 1, "p", 8, 1)])
    port = free_port()
    srv = serve_tasks(board, "127.0.0.1", port)
    url = f"http://127.0.0.1:{port}"
    try:
        opened = json.loads(urllib.request.urlopen(f"{url}/sn0/tasks?hotkey=alice").read())
        assert [t["task_id"] for t in opened["tasks"]] == ["t1"]
        assert json.loads(urllib.request.urlopen(f"{url}/sn0/tasks?hotkey=bob").read())["tasks"] == []
        r = {"task_id": "t1", "hotkey": "alice", "text": "x", "tokens": 1, "top_logprobs": [{"x": -0.1}]}
        forged = dict(r, signature=fake_sign("mallory")(result_message("t1", "x", 1, [{"x": -0.1}])))
        assert board.submit(forged) == 403
        assert board.submit(dict(r, hotkey="bob", signature="00")) == 404
        good = dict(r, signature=fake_sign("alice")(result_message("t1", "x", 1, [{"x": -0.1}])))
        assert board.submit(good) == 200
        assert board.submit(good) == 409
        assert json.loads(urllib.request.urlopen(f"{url}/sn0/tasks?hotkey=alice").read())["tasks"] == []
        board.publish(6, time.time() - 1, [Task("t2", "alice", 1, "p", 8, 1)])
        late = dict(r, task_id="t2", signature=fake_sign("alice")(result_message("t2", "x", 1, [{"x": -0.1}])))
        assert board.submit(late) == 400
    finally:
        srv.shutdown()


def test_pending_reveal_survives_restart(tmp_path):
    state = str(tmp_path / "state.json")
    chain = FakeChain([(0, "validator", ""), (1, "honest", "")])
    v, srv, _ = make_validator(chain, [("honest", MockBackend(noise=0.05))], state_file=state)
    try:
        v.step()
        committed = v.pending
    finally:
        srv.shutdown()
    # The process restarts: a new validator picks the reveal up from disk.
    chain.epoch = 1
    v2, srv2, _ = make_validator(chain, [("honest", MockBackend(noise=0.05))], state_file=state)
    try:
        assert v2.pending.salt == committed.salt
        v2.step()
        assert chain.reveals[0][1:3] == (committed.uids, committed.weights)
    finally:
        srv2.shutdown()


def test_validator_uses_chain_tiers_and_skips_other_tiers():
    from incentium_neuron.validator import TierInfo

    chain = FakeChain([(0, "validator", "", 0), (1, "two-b", "", 0), (2, "nine-b", "", 1)])
    chain.tiers = lambda: {0: TierInfo(0.5, 0.35, "Qwen3.5-2B", "00" * 32, True)}
    v, srv, _ = make_validator(chain, [("two-b", MockBackend(noise=0.05)), ("nine-b", MockBackend(noise=0.05))])
    try:
        stats = v.measure(0)
        assert [s.uid for s in stats] == [1]  # the tier-1 miner is left to tier-1 validators
        assert stats[0].g == 0.5 and stats[0].tier == 0
        assert any("uid 2: tier 1" in line for line in v.log)
        assert stats[0].work_units() == stats[0].tokens * 0.5
    finally:
        srv.shutdown()


def test_read_key_from_phrase_file_or_json(tmp_path):
    import argparse

    from incentium_neuron.validator import read_key

    ap = argparse.ArgumentParser()
    (tmp_path / "plain").write_text("word " * 11 + "word\n")
    (tmp_path / "gen.json").write_text(json.dumps({"secretPhrase": "a b c", "ss58Address": "5x"}))
    assert read_key(argparse.Namespace(key=None, key_file=str(tmp_path / "plain")), ap) == ("word " * 11 + "word")
    assert read_key(argparse.Namespace(key=None, key_file=str(tmp_path / "gen.json")), ap) == "a b c"
    assert read_key(argparse.Namespace(key="//Alice", key_file=None), ap) == "//Alice"
