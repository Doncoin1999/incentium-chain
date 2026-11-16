from incentium_neuron.backend import parse_top_logprobs


def test_new_llama_cpp_format():
    lp = {"content": [{"id": 1, "token": " Oslo", "logprob": -0.45,
                       "top_logprobs": [{"id": 1, "token": " Oslo", "logprob": -0.45},
                                        {"id": 2, "token": "\n", "logprob": -3.2}]}]}
    assert parse_top_logprobs(lp) == [{" Oslo": -0.45, "\n": -3.2}]


def test_legacy_format():
    assert parse_top_logprobs({"top_logprobs": [{"a": -1.0}]}) == [{"a": -1.0}]


def test_decode_bytes():
    from incentium_neuron.validator import decode_bytes
    url = "http://127.0.0.1:8091"
    assert decode_bytes(url) == url
    assert decode_bytes("0x" + url.encode().hex()) == url
    assert decode_bytes(list(url.encode())) == url
    assert decode_bytes(None) == ""


def test_tail_swap_passes_but_other_model_fails():
    from incentium_neuron.scoring import passes_spot_check
    ref = [{"a": -0.2, "b": -2.0, "c": -4.0, "d": -5.0, "e": -5.1}]
    honest = [{"a": -0.21, "b": -2.02, "c": -4.01, "d": -5.05, "f": -5.08}]  # e/f near-tie swap
    other = [{"x": -0.5, "y": -1.5, "a": -3.0, "z": -3.5, "w": -4.0}]
    assert passes_spot_check(0, honest, ref)
    assert not passes_spot_check(0, other, ref)
