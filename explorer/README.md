# Incentium Explorer

The block explorer at explorer.incentium.org: network overview, launch period, top accounts,
account pages with balance, launch-vault releases and history, a read-only wallet connection
(Talisman, SubWallet, Polkadot.js) and search by address, block number or block hash.

| Path | What |
|---|---|
| `indexer/indexer.py` | Follows finalized blocks over RPC, stores blocks, events and a readable per-account history in SQLite; takes a balance snapshot of every account about every 10 minutes |
| `indexer/api.py` | Read-only JSON API over the database (`/api/stats`, `/api/blocks`, `/api/activity`, `/api/accounts`, `/api/account/<addr>`, `/api/account/<addr>/events`, `/api/block/<n or hash>`, `/api/search`); CSV with `?format=csv` |
| `indexer/common.py` | Units, protocol accounts, address handling, launch-vault schedule (mirrors the runtime) |
| `web/` | The page: plain HTML, CSS and JavaScript, no build step |
| `deploy/install-explorer.sh` | Installs or updates everything on server B |
| `tests/` | Tests without a node: `python -m pytest -q` |

## How totals are counted

An account's total is its free and reserved balance plus ICT the protocol holds for it:
validator stake (and stake being withdrawn), work bonds, and its share of the launch vault not yet
claimed. Protocol accounts (launch vault, development fund, subnet pot, stake and bond accounts,
insurance fund, registry deposits) are listed separately, so nothing is counted twice.

Not yet counted: ICT credited to a mining pool and not yet distributed (held by work-verify), and
the future `access` lock. Amounts in the API are strings in base units (1 ICT = 10^9).

## Install on server B

    cd ~/incentium-chain && git pull
    PING_URL=https://hc-ping.com/<uuid> bash explorer/deploy/install-explorer.sh

The indexer reads `wss://rpc.incentium.org` (the archive node on A): B's own RPC node is pruned and
cannot serve events from old blocks. A first run indexes every block since genesis, then follows
the chain block by block.

Preview before switching DNS: `ssh -L 8090:127.0.0.1:8090 administrator@<B>` and open
http://localhost:8090. Go live by pointing the A record of explorer.incentium.org to B at Njalla,
then remove the old explorer from A's Caddy.

Known accounts get names from `~/incentium-explorer/labels.json` (`{"<address>": "Curator multisig"}`).

Start again from scratch: stop the indexer, delete `~/incentium-explorer/explorer.db*`, start it.
