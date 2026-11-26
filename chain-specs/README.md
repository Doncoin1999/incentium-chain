# Chain specifications

The raw chain specs that external nodes start from. They are public: genesis state, boot nodes,
no secrets. `scripts/install-validator.sh` reads them from here, and incentium.org/validators shows
their sha256 so anyone can check a copy.

- `incentium-mainnet-raw.json` — mainnet (created by `scripts/mainnet-genesis.sh` on server A)
- `incentium-testnet-raw.json` — testnet

Boot nodes: add every independent block producer that offers one, so new nodes do not depend on
the first two servers.

## Boot nodes

In the mainnet spec (server A and B):

- `/ip4/85.190.105.211/tcp/30333/p2p/12D3KooWFZjHtUNV6kUKjwr8dz1CxsdS1wuQK1Vj6opPwZkejT9W`
- `/ip4/156.67.101.75/tcp/30333/p2p/12D3KooWMcUoxec6ATU4MjYPx4saN2LtqJqJQw1x6NCxRzQPCNZe`

Added since, outside the spec file so its sha256 stays the same; pass them with `--bootnodes`
(install-validator.sh does):

- `/ip4/94.72.107.58/tcp/30333/p2p/12D3KooWQdXvwW1R5u5bwqDqb6jAo1dRWGLbHTn8XDGKLuPQubyu` (server C, UK, archive node, from 26 Nov 2026)

## Public RPC

- `wss://rpc.incentium.org` (server A)
- `wss://rpc2.incentium.org` (server C), the reserve the incentium.org pages switch to by themselves
