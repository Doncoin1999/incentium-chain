# Chain specifications

The raw chain specs that external nodes start from. They are public: genesis state, boot nodes,
no secrets. `scripts/install-operator.sh` (and `install-validator.sh`, which calls it) reads them from here, and incentium.org/validators shows
their sha256 so anyone can check a copy.

- `incentium-mainnet-raw.json` — mainnet (created by `scripts/mainnet-genesis.sh` on server A)
- `incentium-testnet-raw.json` — testnet

Boot nodes: add every independent block producer that offers one, so new nodes do not depend on
the first two servers.
