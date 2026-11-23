# Incentium Swap on mainnet (trade.incentium.org)

Branch `mainnet-swap` = `main` + the Swap pallet and swap service from branch `swap`. This is the
rollout of ICT ⇄ USDC atomic swaps to Incentium mainnet and Base mainnet, prepared on 23 Nov 2026.

**Decided (23 Nov 2026):** go to mainnet now, before the external audit, relying on the order caps
(100 ICT per order until day 90, 1,000 until day 120, 5,000 after) and the pause guardian. The audit
comes before the caps rise after day 90. Fee recipient and pause guardian on Base:
`0x73aAEC0fBD90901CF275a7df01b02f8842fcf61F` (the operator's MetaMask address; immutable in the
contract).

Not in this branch: the EVM (pallet-revive). Mainnet gets the same Swap runtime the testnet ran as
spec 8 before the EVM upgrade.

## What is in the branch

| Part | Notes |
| --- | --- |
| `pallets/ict-swap`, runtime pallet index 15, `spec_version` 8 | Same as testnet spec 8. No migration: the pallet is new. `transaction_version` unchanged. |
| `swap/base` | `IncentiumSwap.sol`: no owner, no upgrade; pause guardian can stop new orders, reservations and locks only |
| `swap/service` | Keeper + API (`swapsvc`), `install-swap.sh` now takes `NAME=` so testnet and mainnet keepers can run side by side; `config.mainnet.example.json` (Base mainnet, port 8096) |
| `swap/web` | Trading page: shared Incentium top bar (logo → incentium.org), a "Testnet" banner when it is not connected to Base mainnet |
| `explorer` | Swap events shown as "Swaps" on account pages |
| `scripts/upgrade_runtime_wasm.py` | Installs a runtime wasm with `sudo(set_code)`; `--expect Swap` checks the pallet is there afterwards |

Tests run for this branch: Solidity 16/16 (incl. 512-run fuzz of fund conservation), keeper end to
end against the real contract on anvil 4/4, pallet and runtime unit tests.

## 1. Build the runtime (testnet server)

```bash
cd ~/incentium-evm && git fetch origin && git checkout mainnet-swap && git pull
nice -n 19 cargo build --release -p incentium-runtime
WASM=$PWD/target/release/wbuild/incentium-runtime/incentium_runtime.compact.compressed.wasm
ls -lh $WASM
```

Rehearse on a throwaway dev chain (as for the EVM upgrade):

```bash
~/incentium-node-testnet --dev --tmp --rpc-port 9990 --port 30990 --no-prometheus --no-telemetry --no-mdns > /tmp/ovelse.log 2>&1 &
sleep 20
~/upg/bin/python scripts/upgrade_runtime_wasm.py --url ws://127.0.0.1:9990 --key //Alice --yes --expect Swap --wasm $WASM
kill %1
```

## 2. Upgrade mainnet (server A)

First check what mainnet runs: `specVersion` must be **7** and there must be no `Swap` pallet.

```bash
scp $WASM administrator@<server A>:~/incentium-runtime-spec8-swap.wasm       # from the testnet server
# on server A:
ps -eo args | grep "[r]pc-port" | cut -c1-200          # the mainnet node's RPC port
python3 scripts/upgrade_runtime_wasm.py --url ws://127.0.0.1:<rpc-port> \
    --key-file <mainnet sudo key file> --expect Swap --wasm ~/incentium-runtime-spec8-swap.wasm
```

The script shows the chain name and current spec and asks for `yes`. Afterwards: spec 8, blocks
advancing, `Swap` present. A mistake here cannot be undone, only fixed by a further upgrade.

Sudo exists until day 90 (mid-February 2027); after that a runtime upgrade goes through governance.

## 3. Deploy the contract on Base mainnet

Use a fresh deployer key: it has no rights over the contract afterwards and only needs gas
(about 0.002 ETH on Base).

```bash
cd ~/incentium-evm/swap/base
git clone -q --depth 1 https://github.com/foundry-rs/forge-std lib/forge-std
forge test                                               # 16 passed

umask 077; echo "0x$(openssl rand -hex 32)" > ~/base-deployer.key
cast wallet address --private-key $(cat ~/base-deployer.key)      # send ~0.002 ETH on Base to this

# Incentium mainnet block 1 time, for the order caps
GENESIS_TIME=$(~/upg/bin/python - <<'PY'
from substrateinterface import SubstrateInterface
s = SubstrateInterface(url="wss://rpc.incentium.org")
h = s.get_block_hash(1)
print(s.query("Timestamp", "Now", block_hash=h).value // 1000)
PY
); echo $GENESIS_TIME; date -d @$GENESIS_TIME

USDC=0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913 \
GENESIS_TIME=$GENESIS_TIME \
FEE_RECIPIENT=0x73aAEC0fBD90901CF275a7df01b02f8842fcf61F \
PAUSE_GUARDIAN=0x73aAEC0fBD90901CF275a7df01b02f8842fcf61F \
forge script script/Deploy.s.sol --rpc-url https://mainnet.base.org --broadcast \
    --private-key $(cat ~/base-deployer.key)
```

Note the contract address and the Base block it was deployed in (`base_start`). Check it:

```bash
C=<contract>
cast call $C "feeRecipient()(address)" --rpc-url https://mainnet.base.org
cast call $C "pauseGuardian()(address)" --rpc-url https://mainnet.base.org
cast call $C "genesisTime()(uint256)" --rpc-url https://mainnet.base.org
cast call $C "maxOrderQty()(uint256)" --rpc-url https://mainnet.base.org   # 100000000000 = 100 ICT
```

**Pause:** if anything looks wrong, the guardian (MetaMask) calls `setPaused(true)` on the contract.
That stops new orders, reservations and locks; claims, refunds and cancellations keep working, so
nobody's funds are stuck.

## 4. Keeper and trading page

Runs next to the testnet keeper on the same server under its own name, port and domain. If the
testnet page currently answers on `trade.incentium.org`, move it to `trade-test.incentium.org`
first (`DOMAIN=trade-test.incentium.org bash swap/service/deploy/install-swap.sh`, plus a DNS record).

```bash
cd ~/incentium-chain && git fetch origin && git checkout mainnet-swap && git pull
NAME=incentium-swap-mainnet DOMAIN=trade.incentium.org bash swap/service/deploy/install-swap.sh   # writes config, stops
nano ~/incentium-swap-mainnet/config.json
```

In `config.json`: `contract`, `base_start`, `genesis_time`, and `incentium_rpc` — a mainnet node
(`ws://127.0.0.1:<port>` if this server runs one, otherwise `wss://rpc.incentium.org`). Set
`incentium_start` to the block of the runtime upgrade.

Keeper keys (they only pay fees; a stolen key cannot move users' funds):

```bash
cd ~/incentium-swap-mainnet && umask 077
~/upg/bin/python -c "from substrateinterface import Keypair; m=Keypair.generate_mnemonic(); open('incentium-keeper.seed','w').write(m); print(Keypair.create_from_mnemonic(m).ss58_address)"
echo "0x$(openssl rand -hex 32)" > base-keeper.key; cast wallet address --private-key $(cat base-keeper.key)
```

Fund them: about 5 ICT to the Incentium address, about 0.005 ETH on Base to the Base address.
Then run the installer again and add the DNS record `trade` → this server at Njalla:

```bash
NAME=incentium-swap-mainnet DOMAIN=trade.incentium.org bash swap/service/deploy/install-swap.sh
journalctl -u incentium-swap-mainnet -f
```

## 5. First trade (smoke test)

With two of your own wallets and small amounts (minimum fill 5 USDC): place a sell order for a few
ICT from Talisman, take part of it from MetaMask on Base, confirm, and check that ICT and USDC
(minus 0.1 %) arrive and the fee reaches the fee recipient. Then cancel the rest of the order.

## 6. Website

The top bar and footer on every page now link to Trade, and the logo links to incentium.org.

```bash
cd ~/incentium-chain && git checkout mainnet-swap && git pull
bash site/deploy/install-site.sh                       # incentium.org (front page, events, …)
bash explorer/deploy/install-explorer.sh               # explorer, for the top bar and Swaps filter
# chat: re-run its installer the same way, for the top bar
```

## Before the caps rise (day 90, about 16 Feb 2027)

- External audit of `IncentiumSwap.sol`, `pallet-ict-swap` and the keeper.
- Consider moving fee recipient and pause guardian to a multisig: that needs a new contract
  deployment (the addresses are immutable), and the trading page pointed at it.
