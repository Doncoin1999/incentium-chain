import copy
import json

import pytest

import make_bootstrap_spec as m

# Bootstrap preset patch as produced by the runtime (genesis_config_presets::bootstrap).
PATCH = {
    "aura": {"authorities": sorted(m.PLACEHOLDER_SR)},
    "balances": {"balances": [["5GrwvaEF5zXb26Fz9rcQpDWS57CtERHpNehXCPcNoHGKutQY", 100000000000]]},
    "emission": {"preIssued": 100000000000},
    "grandpa": {"authorities": [[k, 1] for k in sorted(m.PLACEHOLDER_ED)]},
    "registry": {"bootstrapBlocks": 648000, "bootstrapOperator": "5GrwvaEF5zXb26Fz9rcQpDWS57CtERHpNehXCPcNoHGKutQY", "phase2Enabled": False},
    "sudo": {"key": "5GrwvaEF5zXb26Fz9rcQpDWS57CtERHpNehXCPcNoHGKutQY"},
}
KEYS = {"operator": "OP", "authorities": [{"aura": f"A{i}", "grandpa": f"G{i}"} for i in range(3)]}


def test_keys_replaced_and_allocation_kept():
    p = copy.deepcopy(PATCH)
    m.apply_keys(p, KEYS)
    assert p["aura"]["authorities"] == ["A0", "A1", "A2"]
    assert p["grandpa"]["authorities"] == [["G0", 1], ["G1", 1], ["G2", 1]]
    assert p["balances"]["balances"] == [["OP", 100000000000]]
    assert p["sudo"]["key"] == p["registry"]["bootstrapOperator"] == "OP"
    assert p["registry"]["bootstrapBlocks"] == 648000
    assert p["emission"]["preIssued"] == 100000000000
    for k in m.PLACEHOLDER_SR | m.PLACEHOLDER_ED:
        assert k not in json.dumps(p)


def test_dev_key_as_authority_is_rejected():
    p = copy.deepcopy(PATCH)
    bad = {"operator": "OP", "authorities": [{"aura": sorted(m.PLACEHOLDER_SR)[0], "grandpa": "G"}]}
    with pytest.raises(SystemExit):
        m.apply_keys(p, bad)


def test_testnet_faucet_allocation_counts_in_issuance():
    p = copy.deepcopy(PATCH)
    m.apply_keys(p, dict(KEYS, faucet={"address": "FAUCET", "ict": 100_000}))
    assert p["balances"]["balances"] == [["OP", 100000000000], ["FAUCET", 100_000 * m.UNIT]]
    assert p["emission"]["preIssued"] == 100000000000 + 100_000 * m.UNIT
