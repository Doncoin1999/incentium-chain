# Mainnet launch runbook

Two validator servers, each running two of the four genesis authorities, so keys can later move to
servers 3 and 4 without a chain change. With two servers, one going down keeps blocks coming at half
the rate, but finality pauses until it is back (GRANDPA needs 3 of 4).

| Server | Runs |
|---|---|
| A | validators v1, v2; public archive RPC (9946, behind Caddy); SN0 validator with Qwen |
| B | validators v1, v2; reserve RPC (9946) |

Both: Ubuntu, user `administrator`, repo cloned to `~/incentium-chain`, the normal-epoch node binary
(CI artifact `incentium-node`) at `~/incentium-mainnet/incentium-node`. Open TCP 30333-30335.

1. On A and on B: `bash scripts/mainnet-node.sh a` (or `b`). Creates the server's keys; prints its
   public half (`~/incentium-mainnet/public-a.json` / `public-b.json`).
2. On A: copy `public-b.json` from B, then `bash scripts/mainnet-genesis.sh`. Creates the operator and
   curator1..3 keys, the curator 2-of-3 multisig and `incentium-mainnet-raw.json`.
3. Back up `~/incentium-mainnet/secrets` on both servers, off the servers. Move curator2/curator3 off A.
4. Copy `incentium-mainnet-raw.json` to B and compare the sha256 printed on both servers.
5. Go live: `bash scripts/mainnet-start.sh b`, then `bash scripts/mainnet-start.sh a`. Block 1 starts
   the 90-day launch period; finality starts once both servers run.
6. Then: Caddy for the public RPC, explorer, watchdog, SN0 (validator, operator miner).

Never run the same authority key on two machines at the same time.
