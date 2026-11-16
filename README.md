# incentium-chain

Reference implementation of **Incentium**, built on the Polkadot SDK
(stable2610). See the whitepaper and "Incentium Technical Specification v0.1" for the rules.

Status: step 2 in progress. Open source, Apache-2.0. No company or foundation operates this software.

## Layout

| Path | What | Status |
|---|---|---|
| `primitives/ict-economics` | Pure `no_std` rules: issuance and halving, AMM, buyback and price-aware injection, allocation, deposit price, Yuma-lite | Done, tested |
| `pallets/ict-emission` | Per-block issuance, 10 % to the on-chain fund, rest to the subnet pot | Done, tested |
| `pallets/subnet-registry` | SN0 at genesis, Phase 2 gate, doubling deposit, neuron fee burn, 90/10 deregistration, on-chain endpoints | Done, tested |
| `pallets/weights` | Commit-reveal of validator weights per epoch (hash-based; drand time-lock before testnet) | Done, tested |
| `pallets/work-verify` | Bonds, spot-check quorum slashing, three-strike ban, pools with pro-rata payouts | Done, tested |
| `pallets/ict-consensus` | Validator staking, Yuma-lite per epoch, pot payout to miners/pools and validators | Done, tested |
| `runtime` | Aura + Grandpa solochain runtime with all Incentium pallets, devnet genesis presets | Done; native tests incl. end-to-end Phase 1 loop |
| `node` | Reference node from the Polkadot SDK solochain template | Compiles; needs the wasm runtime to run |
| `neuron` | Python reference SN0 miner and validator (OpenAI-compatible backends, spot checks, commit-reveal) | Done, tested against mock models |
| `scripts`, `docs/DEVNET.md` | 3-validator devnet launcher, SN0 bootstrap, runbook | Ready |
| `pallets/amm`, `buyback`, `alpha-assets`, `utility-metrics`, `payments` | Phase 2-3 | Later |

## Build and test

```
SKIP_WASM_BUILD=1 cargo test --workspace             # native tests, no wasm target needed
cargo build --release -p incentium-node --features fast-epochs   # real node with wasm runtime
cd neuron && python -m pytest -q
```

Requires Rust stable (tested with 1.97) and, for the node later, `protoc` and the
`wasm32-unknown-unknown` target.

## Cross-check with the simulation

`ict-economics` reproduces the Python simulation (`incentium-sim`): 10,506,000 ICT issued after
4 × 365 days with the reward halved to 0.5 ICT; first halving at block 10,500,000.

## Third-party code

`node/` and `runtime/src/apis.rs` are adapted from the Polkadot SDK solochain template
(MIT-0 / Unlicense).
