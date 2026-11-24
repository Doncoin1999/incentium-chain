# Incentium Swap service

The keeper and API behind trade.incentium.org. It never holds user funds or secrets it was not
given: it watches both chains, keeps the order book, sends reservations for takers, passes a
maker's confirmation to the chain, and relays revealed secrets so the taker's side is claimed at once.
Anyone can do the relay and expiry steps; the keeper only saves users the trouble.

## Parts

| Path | What it does |
| --- | --- |
| `swapsvc/store.py` | SQLite order book: orders, reservations, locks |
| `swapsvc/chains.py` | Reads events from Incentium (Swap pallet) and Base (IncentiumSwap contract), sends keeper transactions |
| `swapsvc/keeper.py` | Main loop (sync, relay secrets, expire, refund) and the HTTP API |
| `../web/` | The trading page (plain JS; vendored @polkadot/api and ethers) |
| `deploy/install-swap.sh` | Installs the service, page and Caddy site on server B |

## API

| Method | Path | |
| --- | --- | --- |
| GET | `/api/config` | Public settings for the page (RPCs, contract, USDC, genesis time) |
| GET | `/api/book` | Open sell and buy orders |
| GET | `/api/order/{inc\|base}/{id}` | One order with its 16 hashes, fills and the matching locks |
| GET | `/api/account?addr=…&addr=…` | Orders, takes and reservations for up to 8 addresses |
| GET | `/api/status` | Last synced block on each chain |
| POST | `/api/reserve` | `{chain, order, qty, recipient}` → the keeper reserves the next free fill for the taker |
| POST | `/api/confirm` | `{chain, lock, secret}` → claims the taker's lock after checking it on chain |

POST requests are limited to 20 per hour per IP address.

## Keys

The keeper has two keys, used only to pay transaction fees: an Incentium seed
(`incentium-keeper.seed`) and a Base private key (`base-keeper.key`). Fund them with a few ICT and a
little ETH on Base. Losing them loses nothing but those fees; a stolen key cannot move users' funds,
because every claim pays an address fixed when the order, reservation or lock was made.

## Order of rollout

1. Incentium testnet: runtime upgrade with the Swap pallet (spec_version 8).
2. Base Sepolia: deploy `swap/base` with test USDC `0x036CbD53842c5426634e7929541eC2318f3dCF7e`.
3. Install this service against both, with `DOMAIN=trade-test.incentium.org`, and run the full
   flow including the failure paths (no confirmation, expired reservation, cancel with open fills).
4. External audit of the contract and pallet.
5. Mainnet: runtime upgrade, contract on Base mainnet (USDC `0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913`),
   then this service with `DOMAIN=trade.incentium.org`.

## Install

```
cd ~/incentium-chain && git checkout swap && git pull
bash swap/service/deploy/install-swap.sh      # first run writes ~/incentium-swap/config.json and stops
nano ~/incentium-swap/config.json             # contract, base_start, genesis_time
bash swap/service/deploy/install-swap.sh      # installs and starts
```

Logs: `journalctl -u incentium-swap -f`

## Tests

```
pip install -r requirements.txt pytest
python -m pytest tests        # needs anvil (Foundry) on PATH; runs against the real contract
```
