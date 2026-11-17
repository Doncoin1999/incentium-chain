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
