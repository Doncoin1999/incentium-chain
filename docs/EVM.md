# EVM on Incentium (pallet-revive) and the Hyperlane bridge

Branch `feat/evm-revive`. Adds an Ethereum-compatible EVM to the runtime so that MetaMask,
Foundry/Hardhat and Hyperlane's standard Solidity contracts work on the Incentium chain
(technical specification, Section 14).

**Status (23 Nov 2026):** running on the testnet since 22 Nov (spec 8 → 9 at about block 37 990),
with a public EVM RPC, MetaMask, a test contract, Hyperlane core contracts and an ICT warp route to
Base Sepolia that works in both directions with a relayer running as a service. Not on mainnet.
The testnet runs branch `testnet-evm` (= `swap` + this branch).

Contents: [why pallet-revive](#why-pallet-revive-and-not-frontier) ·
[what changed](#what-changed) · [build](#build) · [upgrade a chain](#upgrade-a-chain) ·
[eth-rpc](#eth-rpc) · [using the EVM](#using-the-evm) · [Hyperlane](#hyperlane) ·
[testnet record](#testnet-record) · [before mainnet](#before-mainnet)

## Why pallet-revive and not Frontier

Section 14.1 named Frontier. The runtime is on `polkadot-sdk` 2610.0.0, while Frontier's newest
branch is `stable2603`, so Frontier cannot be added without porting it. Frontier would also need
node changes (Ethereum RPC, a mapping database, block import).

`pallet-revive` ships in the same `polkadot-sdk` release (0.22.0) and is Parity's maintained
Ethereum layer, used on Polkadot Hub. Its REVM backend runs unmodified EVM bytecode compiled with
`solc`. Ethereum JSON-RPC comes from a separate `eth-rpc` server, so the node binary does not
change. On the testnet, all of Hyperlane's contracts deployed and work unmodified.

## What changed

| File | Change |
| --- | --- |
| `runtime/Cargo.toml` | `pallet-revive` feature on `polkadot-sdk` |
| `runtime/src/lib.rs` | `Revive` at pallet index 30; `EthSetOrigin` transaction extension; `EthExtraImpl`; revive's `UncheckedExtrinsic` (accepts Ethereum transactions); `spec_version` 9, `transaction_version` 2 |
| `runtime/src/lib.rs` | Migration `CreateReviveAccount`: on the upgrade, creates the revive pallet account with 0.001 ICT moved from the development fund (no minting) |
| `runtime/src/configs.rs` | `pallet_revive::Config`; `IctWeightToFee` becomes revive's `BlockRatioFee<1, 100>` (same fee as before: ref_time / 100) |
| `runtime/src/apis.rs` | `impl_runtime_apis_plus_revive_traits!` adds `ReviveApi` (blocks, receipts, `eth_call`, gas estimates, tracing); `use alloc::{vec, vec::Vec}` for the no_std build |
| `runtime/src/genesis_config_presets.rs` | `BOOTSTRAP_PRE_ISSUED` counts one more existential deposit: on a new chain, revive's genesis creates its account by minting it |
| `runtime/src/tests.rs` | Four EVM tests: integrity test, fee unchanged, account mapping and 18-decimal balance, upgrade migration |
| `scripts/upgrade_runtime_wasm.py` | Installs a runtime wasm with `sudo(set_code)` and checks the result |
| `site/docs/technical-specification.md` | Section 14.1 and the delivery table name pallet-revive instead of Frontier |
| `Cargo.lock` | `alloy-trie` pinned to 0.9.8: 0.9.9 added a struct field that pallet-revive 0.22.0 does not set |

Key settings (`configs.rs`):

| Setting | Value | Note |
| --- | --- | --- |
| `EVM_CHAIN_ID` | 20261118 (`0x13528fe`) | **The testnet's ID**, being registered in `ethereum-lists/chains` as "Incentium Testnet" (`ict-testnet`). Mainnet must get a different, free ID before the mainnet upgrade: sharing one lets a transaction signed on one chain be replayed on the other. |
| `NativeToEthRatio` | 10^9 | ICT has 9 decimals, the EVM shows 18 |
| `AddressMapper` | `AccountId32Mapper` | Existing `5…` accounts unchanged |
| `AutoMap` | false | Talisman accounts call `revive.map_account` once to get an EVM address |
| `UploadOrigin`, `InstantiateOrigin` | any signed account | Restrict during the launch period if wanted |
| Storage deposits | 0.01 ICT per item, 0.0001 ICT per contract slot, 0.00001 ICT per byte | Refundable; safe to change on a live chain |
| `GasScale` | 100 | Gas price shows as 100 gwei; does not change cost |
| `FindAuthor` | none | `block.coinbase` is the zero address (no session pallet) |

### Accounts

- A **MetaMask account** (20 bytes, `0x…`) has a native account of its own: the address followed
  by 24 `e` characters (`0x` + 40 hex + `eeeeeeeeeeeeeeeeeeeeeeee`). Send ICT from Talisman or
  Polkadot.js to that padded address and it shows in MetaMask.
- The EVM balance is the spendable balance: 0.001 ICT (existential deposit) less than the native
  balance.
- A **Talisman account** (sr25519) must call `revive.map_account` once (small refundable deposit)
  before it can use the EVM. A bridge page should do this automatically.
- The faucet takes Substrate addresses only; a `0x` address fails with `CannotLookup`. Send to the
  padded address instead, or extend the faucet to do the padding.

## Build

Build on a server, not a laptop: a full build needs 30–50 GB of disk.

```bash
sudo apt install -y build-essential clang libclang-dev protobuf-compiler pkg-config libssl-dev cmake
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
rustup target add wasm32v1-none wasm32-unknown-unknown    # this SDK builds the runtime for wasm32v1-none
rustup component add rust-src

cargo test -p incentium-runtime                 # 15 tests
cargo build --release -p incentium-runtime      # only the runtime: what an upgrade needs
```

- Without `wasm32v1-none`, cargo prints a warning and **skips the wasm**; the tests still pass, so
  check that the wasm file exists.
- The node itself needs `protoc` (`protobuf-compiler`).
- The runtime wasm for an upgrade is
  `target/release/wbuild/incentium-runtime/incentium_runtime.compact.compressed.wasm` (about 1.1 MB).
- `runtime-benchmarks` builds pallet-revive's test fixtures, which also need `rust-src`.
- On servers where processes such as `llama-server` use most of the memory, run builds with
  `nice -n 19` and `-j 2`, inside `tmux` so a dropped SSH connection does not stop them.

## Upgrade a chain

Rehearse on a throwaway dev chain with the node binary the chain runs, then do the real one:

```bash
python3 -m venv ~/upg && ~/upg/bin/pip install substrate-interface
WASM=target/release/wbuild/incentium-runtime/incentium_runtime.compact.compressed.wasm

# rehearsal (sudo is //Alice; --tmp deletes the chain afterwards)
<node-binary> --dev --tmp --rpc-port 9990 --port 30990 --no-prometheus --no-telemetry --no-mdns &
~/upg/bin/python scripts/upgrade_runtime_wasm.py --url ws://127.0.0.1:9990 --key //Alice --yes --wasm $WASM
kill %1

# real chain (asks for confirmation; checks the key is the sudo key first)
~/upg/bin/python scripts/upgrade_runtime_wasm.py --url ws://127.0.0.1:<rpc-port> \
    --key-file <sudo key file> --wasm $WASM
```

The script waits for the new `specVersion`, then checks that blocks advance, that the Revive
pallet is present and that its account `5EYCAe5ijiYfhaAUBd6H9WGRTsvwFFc7GnhQkiHvBYxdvpbV`
(`PalletId(*b"py/reviv")`) holds 0.001 ICT. If the fund could not pay, send 0.001 ICT by hand.

- **The wasm must contain everything the chain runs.** The testnet ran `swap` (spec 8), so it was
  upgraded with `swap` + this branch (`testnet-evm`), not this branch alone, which would have
  removed the swap pallet. Check `specVersion` and the pallet list of the live chain first.
- `transaction_version` goes from 1 to 2: wallets and `substrate-interface` fetch new metadata
  automatically, but long-running services may need a restart if their transactions are rejected.

## eth-rpc

Parity's Ethereum JSON-RPC server, `pallet-revive-eth-rpc` 0.25.0 (matches pallet-revive 0.22.0).
Uses about 30 MB of memory.

### Build it

The crate's lock file pins a working `alloy-trie` (0.9.5), but eth-rpc builds a test runtime
(`revive-dev-runtime`) during its build, and that inner build does not find the lock file and picks
the newest `alloy-trie`, which fails with "missing field `all_branch_nodes`". Build from the crate
source with `WASM_BUILD_WORKSPACE_HINT` pointing at it. Keep the build out of `/tmp`: on Ubuntu
26.04 `/tmp` is `tmpfs` (in RAM), and `cargo install` builds there by default, which filled the
testnet server's memory and swap.

```bash
# get the source into ~/.cargo/registry (this attempt downloads it, then fails; that is fine)
cargo install pallet-revive-eth-rpc --version 0.25.0 --locked --target-dir ~/eth-rpc-build
rm -rf /tmp/cargo-install*                       # leftovers in RAM, if any

cp -r ~/.cargo/registry/src/index.crates.io-*/pallet-revive-eth-rpc-0.25.0 ~/eth-rpc-src
cd ~/eth-rpc-src
grep -A1 'name = "alloy-trie"' Cargo.lock        # must say 0.9.5
rm -rf ~/eth-rpc-build/release/wbuild/revive-dev-runtime
WASM_BUILD_WORKSPACE_HINT=$HOME/eth-rpc-src nice -n 19 cargo build --release --locked -j 2 \
    --target-dir ~/eth-rpc-build --bin eth-rpc
mkdir -p ~/bin && cp ~/eth-rpc-build/release/eth-rpc ~/bin/
rm -rf ~/eth-rpc-build ~/eth-rpc-src             # 7 GB
```

About 20 minutes on the testnet server. The finished binary can be copied to servers A and B
(same Ubuntu) instead of building there.

### Run it as a service

```ini
# /etc/systemd/system/eth-rpc-testnet.service
[Unit]
Description=Incentium testnet eth-rpc (EVM JSON-RPC)
After=network-online.target

[Service]
User=administrator
ExecStart=/home/administrator/bin/eth-rpc --node-rpc-url ws://127.0.0.1:9960 --eth-pruning 10000 --rpc-port 8545 --rpc-cors all
Restart=always
RestartSec=5

[Install]
WantedBy=multi-user.target
```

- `--eth-pruning 10000` keeps the last 10 000 blocks of EVM data. The default (`archive`) indexes
  the whole chain and needs an archive node; decide this for mainnet.
- Behind Caddy (`/etc/caddy/Caddyfile`), with an A record at Njalla:
  ```
  evm-testnet.incentium.org {
      reverse_proxy 127.0.0.1:8545
  }
  ```
- Check: `eth_chainId` returns `0x13528fe`, `eth_gasPrice` `0x174876e800` (100 gwei).

## Using the EVM

**MetaMask:** Settings → Networks → Add manually: name `Incentium Testnet`, RPC
`https://evm-testnet.incentium.org`, chain ID `20261118`, symbol `ICT`. In recent MetaMask
versions, select the network explicitly to see its balance.

**Foundry** (on the server):

```bash
curl -L https://foundry.paradigm.xyz | bash && ~/.foundry/bin/foundryup
export PATH="$HOME/.foundry/bin:$PATH"
umask 077; echo "0x$(openssl rand -hex 32)" > ~/evm-testkey.txt      # never print the key
export PK=$(cat ~/evm-testkey.txt); export ADDR=$(cast wallet address --private-key $PK)
forge create src/Counter.sol:Counter --rpc-url http://127.0.0.1:8545 --private-key $PK --evm-version cancun --broadcast
```

EIP-1559 (type 2) transactions work; `--legacy` is not needed.

**Costs seen on the testnet** (100 gwei): a simple contract call 174 043 gas ≈ 0.017 ICT, about
ten times a native ICT transfer. The level follows pallet-revive's weights, not `GasScale`; review
it before mainnet, when ICT has a market price.

## Hyperlane

Hyperlane CLI 45.2.0 (needs Node ≥ 22). Install it in the home directory: a global `sudo npm
install` left the CLI without its `bundle/index.js`.

```bash
sudo apt install -y nodejs npm
npm install --prefix ~/hyperlane-cli @hyperlane-xyz/cli@45.2.0
echo 'export PATH="$HOME/hyperlane-cli/node_modules/.bin:$HOME/.foundry/bin:$PATH"' >> ~/.bashrc
export HYP_KEY=$PK
```

### 1. Register the chain

`~/.hyperlane/chains/incentiumtestnet/metadata.yaml`:

```yaml
name: incentiumtestnet
displayName: Incentium Testnet
chainId: 20261118
domainId: 20261118
protocol: ethereum
isTestnet: true
rpcUrls:
  - http: http://127.0.0.1:8545
nativeToken:
  name: Incentium
  symbol: ICT
  decimals: 18
blocks:
  confirmations: 1
  estimateBlockTime: 12
  reorgPeriod: 2
```

### 2. Core contracts

```bash
mkdir -p ~/hyperlane && cd ~/hyperlane
hyperlane core init          # owner = signer, default ISM trustedRelayerIsm, protocol fee 0
hyperlane core deploy --chain incentiumtestnet
```

About 15 contracts; cost 3.4 ICT. Addresses go to `~/.hyperlane/chains/incentiumtestnet/addresses.yaml`.

### 3. Warp route (ICT ⇄ Base Sepolia)

The interactive chain picker is hard to use over SSH (space selects, Enter confirms; selecting
"New custom chain" aborts). Write the config instead. `warp deploy` in 45.2.0 has no `--config`; it
reads the config from the registry by warp route ID:

```yaml
# ~/.hyperlane/deployments/warp_routes/ICT/basesepolia-incentiumtestnet-deploy.yaml
incentiumtestnet:
  type: native
  owner: "<owner>"
  mailbox: "0xa1D2a38666eF19E4c8431847f769Bcc8DC32B2B4"
  interchainSecurityModule:
    type: trustedRelayerIsm
    relayer: "<relayer>"
basesepolia:
  type: synthetic
  name: "Incentium"
  symbol: "ICT"
  decimals: 18
  owner: "<owner>"
  mailbox: "0x6966b0E55883d49BFB24539356a2f8A673E02039"   # Hyperlane's Base Sepolia mailbox
  interchainSecurityModule:
    type: trustedRelayerIsm
    relayer: "<relayer>"
```

```bash
hyperlane warp deploy -w ICT/basesepolia-incentiumtestnet
```

Cost 0.59 ICT and about 0.00003 test ETH. The Basescan verification errors ("Invalid API Key",
"ProxyAdmin not found") only concern source verification and can be ignored.

The trusted-relayer ISM is needed on both sides: Base Sepolia's default ISM trusts Hyperlane's
validators, which do not watch Incentium.

### 4. Send and relay

```bash
# 1 ICT Incentium -> Base Sepolia (amounts in 18 decimals)
hyperlane warp send -w ICT/basesepolia-incentiumtestnet --origin incentiumtestnet --destination basesepolia --amount 1000000000000000000
# balance of the synthetic ICT on Base Sepolia
cast call <synthetic> "balanceOf(address)(uint256)" $ADDR --rpc-url https://sepolia.base.org
```

Add `--relay` to deliver the message yourself. The relayer as a service:

```ini
# /etc/systemd/system/hyperlane-relayer.service
[Unit]
Description=Hyperlane relayer (ICT warp route, Incentium testnet <-> Base Sepolia)
After=network-online.target eth-rpc-testnet.service

[Service]
User=administrator
WorkingDirectory=/home/administrator/hyperlane
EnvironmentFile=/home/administrator/.hyperlane-relayer.env     # HYP_KEY=0x…, mode 600
ExecStart=/usr/bin/node /home/administrator/hyperlane-cli/node_modules/@hyperlane-xyz/cli/bundle/index.js relayer -w ICT/basesepolia-incentiumtestnet --chains incentiumtestnet basesepolia
Restart=always
RestartSec=10

[Install]
WantedBy=multi-user.target
```

- `--chains` takes chains separated by **spaces**; a comma-separated list fails with "No chain
  metadata set for incentiumtestnet,basesepolia".
- The warnings "Hook at 0x000… does not support hookType()" at start-up are harmless.
- **The CLI relayer only handles messages dispatched while it runs.** A message sent while it was
  down stays pending; deliver it by hand:
  `hyperlane status --origin <chain> --id <message id> --relay`.

## Testnet record

Deployed 22–23 Nov 2026. Owner, relayer and deployer are all the test key
`0x3cfd71BE8Bc3c8Fc9E442f838A2298474E6AAFd5` (key file `~/evm-testkey.txt` on the testnet server).

| What | Address |
| --- | --- |
| Public EVM RPC | `https://evm-testnet.incentium.org` |
| Mailbox (Incentium) | `0xa1D2a38666eF19E4c8431847f769Bcc8DC32B2B4` |
| Merkle tree hook | `0x9a44468c06320ac477403767ab3dA8557504c1A1` |
| Validator announce | `0x1E60371394340b957242c3256CE7d353C303282e` |
| Proxy admin | `0xe7e758DdA9b41096BA03b48a3f95585A4165cB69` |
| ICT warp route, Incentium side (`HypNative`) | `0xD37597437E839bf5306bEE5A134ef3E4bB78F710` |
| ICT warp route, Base Sepolia side (`HypERC20`, synthetic ICT) | `0xb11ead252c103E41ab16240c15402FfE2bbDCbbC` |
| Test contract (`Counter`) | `0xf75094f73D7BC28d33e8FF09a039f29625F90EFe` |

The full list is in `~/.hyperlane` on the testnet server (backed up to the operator's Mac as
`~/hyperlane-testnet-backup`). The first message from Incentium (nonce 0):
<https://explorer.hyperlane.xyz/message/0xb2ab604681efb334a69673f8d22a25b5f1ff322b729b1ba7c5183abb31b4a65d>.

Tests passed: Incentium → Base Sepolia (delivered in 2 s), Base Sepolia → Incentium (7 s), a
pending message relayed by hand, and automatic delivery by the relayer service.

## Before mainnet

- **Chain ID:** 20261118 is the testnet's (registration in `ethereum-lists/chains` submitted 23 Nov
  2026). Pick a free ID for mainnet, set `EVM_CHAIN_ID` in `runtime/src/configs.rs` to it in the
  mainnet build, and register it once the mainnet EVM RPC exists.
- **Validators instead of the trusted relayer:** run Hyperlane validators (3–5, different
  operators) and switch both ISMs to a multisig ISM.
- **Relayer:** use Hyperlane's Rust relayer agent, which tracks its position and catches up after
  downtime, instead of the CLI relayer.
- **Keys:** separate deployer, relayer and validator keys; owner of the core and warp route
  contracts a multisig.
- **Fees:** review the EVM fee level (about 0.017 ICT for a simple call on the testnet).
- **Weights:** run the pallet-revive benchmarks on the mainnet hardware (the pallet uses Parity's
  reference weights, `SubstrateWeight`).
- **Contract deployment:** decide whether `UploadOrigin`/`InstantiateOrigin` stay open during the
  launch period.
- **eth-rpc pruning:** archive (needs an archive node) or a window like on the testnet.
- **Faucet:** accept `0x` addresses by padding them.
- **Bridge UI:** e.g. Hyperlane's Warp UI, so users do not need the CLI.
- **Audit:** the runtime change, the Hyperlane setup and the warp route.
