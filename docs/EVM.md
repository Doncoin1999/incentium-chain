# EVM on Incentium (pallet-revive)

Branch `feat/evm-revive`. Adds an Ethereum-compatible EVM to the runtime so that MetaMask,
Foundry/Hardhat and Hyperlane's standard Solidity contracts work on the Incentium chain
(technical specification, Section 14).

## Why pallet-revive and not Frontier

Section 14.1 names Frontier. The runtime is on `polkadot-sdk` 2610.0.0, while Frontier's newest
branch is `stable2603`, so Frontier cannot be added without porting it. Frontier would also need
node changes (Ethereum RPC, a mapping database, block import).

`pallet-revive` ships in the same `polkadot-sdk` release (0.22.0, already in `Cargo.lock`) and is
Parity's maintained Ethereum layer, used on Polkadot Hub. Its REVM backend runs unmodified EVM
bytecode compiled with `solc`. Ethereum JSON-RPC comes from a separate `eth-rpc` server, so the
node binary does not change.

## What changed

| File | Change |
| --- | --- |
| `runtime/Cargo.toml` | `pallet-revive` feature on `polkadot-sdk` |
| `runtime/src/lib.rs` | `Revive` at pallet index 30; `EthSetOrigin` transaction extension; `EthExtraImpl`; revive's `UncheckedExtrinsic` (accepts Ethereum transactions); `spec_version` 9, `transaction_version` 2 |
| `runtime/src/configs.rs` | `pallet_revive::Config`; `IctWeightToFee` becomes revive's `BlockRatioFee<1, 100>` (same fee as before: ref_time / 100) |
| `runtime/src/apis.rs` | `impl_runtime_apis_plus_revive_traits!` adds `ReviveApi` (blocks, receipts, `eth_call`, gas estimates, tracing) |
| `runtime/src/lib.rs` | Migration `CreateReviveAccount`: on the upgrade, creates the revive pallet account with 0.001 ICT moved from the development fund (no minting) |
| `runtime/src/genesis_config_presets.rs` | `BOOTSTRAP_PRE_ISSUED` counts one more existential deposit: on a new chain, revive's genesis creates its account by minting it |
| `runtime/src/tests.rs` | Four EVM tests: integrity test, fee unchanged, account mapping and 18-decimal balance, upgrade migration |
| `site/docs/technical-specification.md` | Section 14.1 and the delivery table name pallet-revive instead of Frontier |
| `Cargo.lock` | `alloy-trie` pinned to 0.9.8: 0.9.9 added a struct field that pallet-revive 0.22.0 does not set |

Key settings (`configs.rs`):

| Setting | Value | Note |
| --- | --- | --- |
| `EVM_CHAIN_ID` | 20261118 | **Check it is free on chainid.network before mainnet.** Cannot change later. |
| `NativeToEthRatio` | 10^9 | ICT has 9 decimals, EVM shows 18 |
| `AddressMapper` | `AccountId32Mapper` | Existing `5…` accounts unchanged |
| `AutoMap` | false | Talisman accounts call `revive.map_account` once to get an EVM address |
| `UploadOrigin`, `InstantiateOrigin` | any signed account | Restrict during the launch period if wanted |
| Storage deposits | 0.01 ICT per item, 0.0001 ICT per contract slot, 0.00001 ICT per byte | Refundable; safe to change on a live chain |
| `GasScale` | 100 | Puts gas numbers near Ethereum's; does not change cost |
| `FindAuthor` | none | `block.coinbase` is the zero address (no session pallet) |

Accounts: a MetaMask account (H160) has a native account of its own (the address padded with
`0xEE`) and can receive ICT from Substrate accounts. A Talisman account must be mapped once
(small refundable deposit); the bridge page should do this automatically.

## Build and test

```bash
cargo test -p incentium-runtime
cargo build --release -p incentium-node   # also builds the new runtime Wasm
```

The Wasm for a runtime upgrade is
`target/release/wbuild/incentium-runtime/incentium_runtime.compact.compressed.wasm`.

## Merging with the `swap` branch

`swap` uses pallet index 15 and `spec_version` 8. This branch uses index 30 and `spec_version` 9,
so they do not collide. After merging, keep `spec_version` above what the testnet runs.

## Rollout (testnet first)

1. **Upgrade the testnet runtime** with sudo:
   `sudo.sudoUncheckedWeight(system.setCode(<wasm>), {refTime: 0, proofSize: 0})`
   (Polkadot.js Apps → Developer → Sudo). No storage migration is needed: the pallet is new.
   `transaction_version` goes from 1 to 2, so wallets and scripts must fetch fresh metadata
   (Talisman, SubWallet, Polkadot.js and `substrate-interface` do this automatically).
   Check afterwards that the revive pallet account
   `5EYCAe5ijiYfhaAUBd6H9WGRTsvwFFc7GnhQkiHvBYxdvpbV` (`PalletId(*b"py/reviv")`) exists; the
   migration funds it from the development fund. If the fund
   could not pay, send it 0.001 ICT by hand.
2. **Run `eth-rpc`** next to an archive node (server B):
   ```bash
   cargo install pallet-revive-eth-rpc --version 0.25.0 --locked
   eth-rpc --node-rpc-url ws://127.0.0.1:9944 --rpc-port 8545 --rpc-cors all
   ```
   Put it behind TLS, e.g. `https://evm-testnet.incentium.org`. The default `--eth-pruning archive`
   needs an archive node (`--state-pruning archive`).
3. **MetaMask**: add network with the RPC URL, chain ID 20261118, symbol ICT, 18 decimals,
   explorer optional.
4. **Smoke test**: `cast chain-id`, `cast balance`, send ICT between two MetaMask accounts, deploy a
   small contract with `forge create`, map a Talisman account and transfer to/from it.
5. **Hyperlane**: `hyperlane core deploy` on the Incentium EVM, then a native-collateral Warp Route
   against Base Sepolia (`hyperlane warp deploy`). Check that the validator and relayer work with
   `eth-rpc` (log queries, finality, gas estimates).
6. **Mainnet** only after the external audit covers the runtime change and the Hyperlane setup.

## Open points

- Confirm the EVM chain ID is unused.
- Decide whether contract deployment is open to everyone during the launch period.
- Weights: the pallet uses Parity's reference weights (`SubstrateWeight`). Run the pallet-revive
  benchmarks on the mainnet hardware before mainnet.
