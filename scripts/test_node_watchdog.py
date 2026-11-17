from node_watchdog import UNIT, Snapshot, problems


def healthy(**kw):
    s = Snapshot(now=1000.0, last_block_time=995.0, best=500, finalized=498, epoch=50,
                 commits={"5Alice": 49}, faucet_balance=20_000 * UNIT, public_rpc_ok=True, disk_used_share=0.1)
    for k, v in kw.items():
        setattr(s, k, v)
    return s


def test_healthy_chain_has_no_problems():
    assert problems(healthy()) == []


def test_each_failure_is_reported():
    assert "no new block" in problems(healthy(last_block_time=800.0))[0]
    assert "finality" in problems(healthy(finalized=400))[0]
    assert "last committed in epoch 45" in problems(healthy(commits={"5Alice": 45}))[0]
    assert "never committed" in problems(healthy(commits={"5Alice": None}))[0]
    assert "faucet balance is low" in problems(healthy(faucet_balance=5 * UNIT))[0]
    assert problems(healthy(public_rpc_ok=False)) == ["public RPC does not answer"]
    assert "disk 95% full" in problems(healthy(disk_used_share=0.95))[0]
    assert len(problems(healthy(last_block_time=0.0, public_rpc_ok=False))) == 2


def test_optional_checks_can_be_off():
    assert problems(healthy(faucet_balance=None, public_rpc_ok=None, commits={})) == []


class FakeStorage:
    def __init__(self, commits, revealed):
        self.commits, self.revealed = commits, revealed

    def query(self, module, name, params):
        class V:
            pass

        v = V()
        if name == "Commits":
            v.value = self.commits.get(params[1])
        else:
            v.value = self.revealed.get((params[0][1], params[1]))
        return v


def test_revealed_weights_count_as_recent():
    from node_watchdog import last_weights_epoch

    # Just after the reveal and before the next commit, the commit sits in Revealed.
    s = FakeStorage({}, {(49, "5Alice"): [[2, 65535]]})
    assert last_weights_epoch(s, "5Alice", 50) == 49
    assert last_weights_epoch(FakeStorage({"5Alice": ["0xab", 50]}, {}), "5Alice", 50) == 50
    assert last_weights_epoch(FakeStorage({}, {}), "5Alice", 50) is None
