"""Explorer tests without a node: classification, vault schedule, snapshot + API on a temp database.

  cd explorer && python -m pytest -q
"""
import json
import threading
import urllib.request

import pytest

from indexer import api, common, db, indexer

M = 30 * common.DAY
PROTO = common.protocol_accounts()
BY = {v: k for k, v in PROTO.items()}
CTX = indexer.context(BY)
ALICE = "5GrwvaEF5zXb26Fz9rcQpDWS57CtERHpNehXCPcNoHGKutQY"
BOB = "5FHneW46xGXgs5mUiveU4sbTyGBzmstUspZC92UhjJM694ty"


def test_launch_unlock_matches_runtime():
    # same points as primitives/ict-economics/src/registry.rs launch_tests
    assert common.launch_unlocked_ppm(2 * M - 1) == 0
    assert common.launch_unlocked_ppm(2 * M) == 100_000      # day 150
    assert common.launch_unlocked_ppm(4 * M) == 200_000
    assert common.launch_unlocked_ppm(20 * M) == 1_000_000   # day 690
    assert common.launch_unlocked_ppm(40 * M) == 1_000_000


def test_vault_schedule():
    v = common.vault_schedule(1_000 * common.UNIT, 0, 648_000, 648_000 + 2 * M)
    assert v["claimable"] == 100 * common.UNIT
    assert v["releases"][0]["block"] == 648_000 + 2 * M
    assert v["releases"][-1]["block"] == 648_000 + 20 * M
    assert v["releases"][-1]["unlocked"] == 1_000 * common.UNIT
    assert common.vault_schedule(1_000, 0, 648_000, 100)["claimable"] == 0


def test_protocol_accounts_are_pallet_accounts():
    from substrateinterface.utils.ss58 import ss58_decode
    raw = bytes.fromhex(ss58_decode(BY["Launch vault"]))
    assert raw == b"modlict/lnch" + b"\0" * 20


def test_normalize_address_any_prefix():
    from substrateinterface.utils.ss58 import ss58_decode, ss58_encode
    polkadot_style = ss58_encode(ss58_decode(ALICE), 0)
    assert common.normalize_address(polkadot_style) == ALICE
    assert common.normalize_address("0x" + ss58_decode(ALICE)) == ALICE
    assert common.normalize_address("nonsense") is None


def T(frm, to, amt):
    return indexer.classify("Balances", "Transfer", {"from": frm, "to": to, "amount": amt}, PROTO, CTX)


def test_classify_transfers():
    assert T(ALICE, BOB, 5) == [(ALICE, "transfer", "Transfer out", BOB, -5, None), (BOB, "transfer", "Transfer in", ALICE, 5, None)]
    r = T(BY["Subnet pot"], ALICE, 7)
    assert r[0] == (ALICE, "reward", "Reward paid", BY["Subnet pot"], 7, None)
    assert T(BY["Subnet pot"], BY["Launch vault"], 9) == []
    assert T(ALICE, CTX["stake"], 100)[0][1:3] == ("stake", "Staked")
    assert T(BY["Launch vault"], ALICE, 3)[0][1:3] == ("vault", "Claimed from launch vault")
    locked = indexer.classify("Consensus", "LaunchRewardLocked", {"who": ALICE, "amount": 21}, PROTO, CTX)
    assert locked == [(ALICE, "reward", "Reward locked in launch vault", BY["Launch vault"], 21, "locked")]


def test_bond_return_vs_pool_payout():
    ctx = dict(CTX, work_withdrawn={ALICE})
    r = indexer.classify("Balances", "Transfer", {"from": CTX["work"], "to": ALICE, "amount": 4}, PROTO, ctx)
    assert r[0][2] == "Work bond returned"
    r = indexer.classify("Balances", "Transfer", {"from": CTX["work"], "to": BOB, "amount": 4}, PROTO, ctx)
    assert r[0][2] == "Pool payout"


@pytest.fixture
def database(tmp_path):
    path = str(tmp_path / "x.db")
    con = db.connect(path)
    U = common.UNIT
    db.set_meta(con, "genesis_ts", 1_795_000_000)
    for n in (1, 2):
        indexer.store_block(con, {"n": n, "hash": f"0x{n:064x}", "ts": 1_795_000_000 + 12 * n, "author": None, "n_extrinsics": 1,
                                  "events": [{"idx": 0, "extrinsic": None, "module": "Balances", "name": "Transfer",
                                              "attributes": {"from": BY["Subnet pot"], "to": ALICE, "amount": 25 * U}},
                                             {"idx": 1, "extrinsic": None, "module": "Consensus", "name": "LaunchRewardLocked",
                                              "attributes": {"who": ALICE, "amount": 75 * U}}]}, PROTO, CTX)
    snap = {"accounts": {ALICE: (50 * U, 0), BOB: (3 * U, 0), BY["Launch vault"]: (150 * U, 0), BY["Development fund"]: (20 * U, 0),
                         CTX["stake"]: (10 * U, 0)},
            "stake": {ALICE: 10 * U}, "unstaking": {}, "bond": {}, "unbonding": {}, "vault_locked": {ALICE: 150 * U},
            "vault_claimed": {}, "neurons": {ALICE: 0, BOB: 1}, "tiers": {}, "issued": 233 * U,
            "bootstrap_until": 648_000, "operator": BOB}
    indexer.store_snapshot(con, 2, snap, PROTO, {})
    db.set_meta(con, "heads", {"best": 3, "finalized": 2, "at": 0})
    db.set_meta(con, "next_block", 3)
    con.commit()
    api.DB_PATH = path
    return path


def test_snapshot_counts_held_ict_with_owner(database):
    c = api.con()
    res = api.accounts(c, "holders", 0, 100)
    top = res["rows"][0]
    assert top["addr"] == ALICE and top["role"] == "validator"
    assert int(top["total"]) == (50 + 10 + 150) * common.UNIT   # free + stake + vault
    assert res["rows"][1]["label"] == "Launch operator"
    proto = api.accounts(c, "protocol", 0, 100)["rows"]
    assert {r["label"] for r in proto} >= {"Launch vault", "Development fund"}
    s = api.stats(c)
    assert s["vault"] == str(150 * common.UNIT) and s["validators"] == 1 and s["miners"] == 1
    assert s["launch"]["in_launch"] and s["launch"]["days"] == 90


def test_account_and_events(database):
    c = api.con()
    a = api.account(c, ALICE)
    assert a["rank"] == 1 and a["vault_schedule"]["claimable"] == "0" and a["vault_schedule"]["growing"]
    assert a["vault_schedule"]["releases"][0]["at"] == 1_795_000_000 + (648_000 + 2 * M - 1) * 12
    ev = api.account_events(c, ALICE, "reward", 0, 10)
    assert ev["count"] == 4 and ev["rows"][0]["block"] == 2
    assert api.account(c, "5CiPPseXPECbkjWCa6MnjNokrgYjMqmKndv2rSnekmSK2DjL")["known"] is False
    assert api.search(c, "#2") == {"type": "block", "id": "2"}
    assert api.search(c, ALICE)["type"] == "account"
    assert "25.000000000" in api.to_csv(ev["rows"], ["block", "amount"])


def test_http_server(database):
    from http.server import ThreadingHTTPServer
    srv = ThreadingHTTPServer(("127.0.0.1", 0), api.Handler)
    threading.Thread(target=srv.serve_forever, daemon=True).start()
    base = f"http://127.0.0.1:{srv.server_address[1]}"
    try:
        acc = json.load(urllib.request.urlopen(f"{base}/api/accounts?limit=5"))
        assert acc["rows"][0]["addr"] == ALICE
        blk = json.load(urllib.request.urlopen(f"{base}/api/block/1"))
        assert len(blk["events"]) == 2
        csv_text = urllib.request.urlopen(f"{base}/api/account/{ALICE}/events?format=csv").read().decode()
        assert csv_text.startswith("block,ts,kind")
    finally:
        srv.shutdown()


def test_classify_swap_events():
    r = indexer.classify("Balances", "TransferOnHold", {"reason": {"Swap": "Swap"}, "source": ALICE, "dest": BOB, "amount": 5}, PROTO, CTX)
    assert r == [(ALICE, "swap", "Swap: ICT delivered", BOB, -5, None), (BOB, "swap", "Swap: ICT received", ALICE, 5, None)]
    held = indexer.classify("Balances", "Held", {"reason": {"Swap": "Swap"}, "who": ALICE, "amount": 2 * common.UNIT}, PROTO, CTX)
    assert held[0][2] == "ICT held for a swap" and "2.00 ICT" in held[0][5]
    assert indexer.classify("Balances", "Held", {"reason": {"Other": "x"}, "who": ALICE, "amount": 1}, PROTO, CTX) == []
