# Closed devnet runbook

The devnet runs three validator nodes (Alice, Bob, Charlie) on one machine or three
machines, with SN0 bootstrapped from dev accounts. Sudo exists on the devnet only.

## 1. Build and start the chain

Requirements: Rust stable with the `wasm32-unknown-unknown` target and `rust-src`, `protoc`, `clang`.

```
scripts/devnet.sh --fast      # 10-block epochs: a full commit -> reveal -> payout cycle in ~4 minutes
scripts/devnet.sh             # real 360-block epochs (72 minutes)
```

## 2. Start inference backends

Tier 1 (consumer hardware): `llama-server -m Qwen3.5-9B-Q4_K_M.gguf --port 8080`
Tiers 2-3: vLLM serving Qwen3.5-27B / Qwen3.5-122B-A10B with an OpenAI-compatible API.
Validators need the same model as a reference backend for spot checks.

## 3. Bootstrap SN0

```
pip install substrate-interface
python3 scripts/bootstrap_sn0.py --url ws://127.0.0.1:9944 \
  --miner-endpoint http://127.0.0.1:8091 --pool-endpoint http://127.0.0.1:8092
```

## 4. Run miners and validators

```
cd neuron
python -m incentium_neuron.miner --backend http://127.0.0.1:8080 --model qwen3.5-9b-q4 --hotkey <Charlie SS58> --port 8091
python -m incentium_neuron.miner --backend http://127.0.0.1:8080 --model qwen3.5-9b-q4 --hotkey <Dave SS58> --port 8092
python -m incentium_neuron.validator --key //Alice --uid 0 --reference http://127.0.0.1:8080 --model qwen3.5-9b-q4 --epoch-length 10
python -m incentium_neuron.validator --key //Bob   --uid 1 --reference http://127.0.0.1:8080 --model qwen3.5-9b-q4 --epoch-length 10
```

## 5. What to check

- `Emission.TotalIssued` grows by 1 ICT per block; the fund account gets 10 %.
- After two epochs, `Consensus.EpochSettled` events pay Charlie and Dave's pool.
- Dave calls `WorkVerify.distribute` to pay Eve and Ferdie.
- Point a miner at a different model: validators report it and it is penalised.

## Known devnet limitations

- Commit-reveal is hash-based with manual reveal; drand time-lock comes before testnet.
- Consensus runs on-chain for SN0 (Yuma-lite, no bonds); the proof-based version comes later.
- Extrinsic weights are estimates, not benchmarked.
