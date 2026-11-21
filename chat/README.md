# chat.incentium.org (prototype)

A free chat page and an OpenAI-compatible API whose answers come from the Qwen models that SN0
miners already run. Prototype scope: free tier only, testnet miners, no accounts or payments.

```
visitor ──HTTPS──> Caddy ──> gateway.py ──(job, long poll)──> Incentium Node (miner, at home)
                                  ^                                   │ llama-server, same model
                                  └──────────(answer, streamed)───────┘ as its declared tier
```

- **Miners** need nothing new: the Incentium Node (`neuron/incentium_neuron/chat.py`) asks the gateway
  for work over outgoing HTTPS while it is mining, signed by its hotkey. It can be switched off on the
  node's dashboard. The gateway admits only hotkeys registered on SN0, on the tier they declared, so
  validators' spot checks cover the same model.
- **Visitors** get a daily free quota per IP (`--free-per-day`, default 40), one answer at a time,
  answers up to 768 tokens. Chats are stored only in their browser. The gateway keeps counters, never
  the text, and writes no request logs.
- **Models**: Auto, Fast (tier 0, Qwen3.5-2B), Smart (tier 1, Qwen3.5-9B), Pro (tier 2, Qwen3.5-27B).
  Only tiers with miners online are offered. Auto takes the largest tier with an idle miner.
- **Backup model** (optional): `--fallback URL` answers tier-0 requests that no miner picks up within
  6 seconds, so the page works while few miners are online.
- If a miner takes a job and goes quiet, the job goes to another miner; its late answer is refused.

## API

```
GET  /v1/models
POST /v1/chat/completions   {"model": "auto"|"fast"|"smart"|"pro", "messages": [...], "stream": true}
GET  /api/status            miners online per tier, queue, your quota
```

Any OpenAI client works with `base_url="https://chat.incentium.org/v1"` (no key in the prototype).
Each response carries an `incentium` object: tier, which miner (short address) or backup answered,
free messages left.

## Run locally

```
python3 chat/gateway.py --dev --port 8400          # any signed miner may serve, no chain
python3 -m pytest -q chat                          # end-to-end tests with real keys and a fake model
```

## Install on server B

1. DNS at Njalla: A record `chat` -> server B.
2. `cd ~/incentium-chain && git pull && bash chat/deploy/install-chat.sh`
   (add `WITH_FALLBACK=1` to also run Qwen3.5-2B on the server as a backup).

Logs: `journalctl -u incentium-chat -f`. Miners pick up the gateway automatically once they run a
node version with chat support (re-run the installer to update).

## Not in the prototype

Accounts, payments in ICT/USDC/card, private mode, spot checks on live chat traffic, bot checks
(Cloudflare Turnstile) and content filtering. See the design sketch for the later phases.
