import json

import pytest

import mainnet_keys as m

ALICE = "5GrwvaEF5zXb26Fz9rcQpDWS57CtERHpNehXCPcNoHGKutQY"
BOB = "5FHneW46xGXgs5mUiveU4sbTyGBzmstUspZC92UhjJM694ty"
CHARLIE = "5FLSigC9HGRKVhB9FiEo4Y3koPsNmBmLJbpXg2mp1hXcS59Y"


def test_ss58_round_trip():
    assert m.ss58_encode(m.ss58_decode(ALICE)) == ALICE


def test_bad_checksum_rejected():
    with pytest.raises(ValueError):
        m.ss58_decode(ALICE[:-1] + ("Z" if ALICE[-1] != "Z" else "Y"))


def test_multisig_matches_pallet_multisig():
    # Well-known 2-of-3 multisig of the dev accounts; order of signatories does not matter.
    assert m.multisig([ALICE, BOB, CHARLIE], 2) == "5DjYJStmdZ2rcqXbXGX7TW85JsrW6uG4y9MUcLq2BoPMpRA7"
    assert m.multisig([CHARLIE, ALICE, BOB], 2) == m.multisig([ALICE, BOB, CHARLIE], 2)


def _secrets(tmp_path, names):
    for name, addr in names.items():
        (tmp_path / f"{name}.json").write_text(json.dumps({"ss58Address": addr}))
    return str(tmp_path)


def test_genesis_combines_both_servers(tmp_path):
    s = _secrets(tmp_path, {"operator": ALICE, "curator1": ALICE, "curator2": BOB, "curator3": CHARLIE})
    a = {"authorities": [{"aura": "A1", "grandpa": "G1"}, {"aura": "A2", "grandpa": "G2"}], "bootnodes": ["a1", "a2"]}
    b = {"authorities": [{"aura": "B1", "grandpa": "H1"}, {"aura": "B2", "grandpa": "H2"}], "bootnodes": ["b1", "b2"]}
    k = m.genesis(s, a, b)
    assert [x["aura"] for x in k["authorities"]] == ["A1", "A2", "B1", "B2"]
    assert k["operator"] == ALICE
    assert k["curator"] == "5DjYJStmdZ2rcqXbXGX7TW85JsrW6uG4y9MUcLq2BoPMpRA7"
    assert k["bootnodes"] == ["a1", "b1"]


def test_genesis_rejects_duplicate_authorities(tmp_path):
    s = _secrets(tmp_path, {"operator": ALICE, "curator1": ALICE, "curator2": BOB, "curator3": CHARLIE})
    a = {"authorities": [{"aura": "A1", "grandpa": "G1"}, {"aura": "A2", "grandpa": "G2"}], "bootnodes": ["a1"]}
    with pytest.raises(SystemExit):
        m.genesis(s, a, a)
