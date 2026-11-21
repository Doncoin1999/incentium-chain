# Incentium sign-in

Keeps the Incentium web pages (trade-test, faucet, register, explorer) for invited people while
incentium.org itself and the RPC endpoints stay public. People request access with email and
password at auth.incentium.org; an admin approves each request at https://auth.incentium.org/admin.
One session cookie on `.incentium.org` covers all sites on all servers (30 days).

Caddy asks the service before serving a protected site (`forward_auth` to `/verify`).
Standard-library Python only; passwords are stored as scrypt hashes in `~/incentium-auth/auth.db`.

## Install (server B)

1. DNS at Njalla: A record `auth` -> 156.67.101.75.
2. `cd ~/incentium-chain && git pull && bash auth/deploy/install-auth.sh`
3. `python3 ~/incentium-chain/auth/auth_service.py add-admin you@example.org`

trade-test.incentium.org is protected by step 2 (and stays protected when install-swap.sh runs again).

## Protect sites on another server (old server)

    sudo python3 ~/incentium-chain/auth/deploy/protect-caddy.py /etc/caddy/Caddyfile \
         faucet.incentium.org register.incentium.org explorer.incentium.org --remote https://auth.incentium.org
    sudo systemctl reload caddy

Undo with the same command plus `--remove`. The script keeps `<file>.bak-auth` and only writes a
change that Caddy validates.

## Admin from the command line

    python3 auth_service.py list
    python3 auth_service.py approve someone@example.org
    python3 auth_service.py revoke someone@example.org
    python3 auth_service.py delete someone@example.org

## Never protect

rpc / mainnet-rpc / testnet-rpc: wallets, miners, nodes and wallet listings need them. Chain data is
public anyway; this only hides the web pages.
