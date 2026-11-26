# Review fixes H1, H2 (and M8) – swap

> **Superseded in part (26 Nov 2026):** version 2 (`SWAP-V2.md`) removes the deposit arbiter, the fee and the pause guardian. At expiry an unclaimed deposit is half returned, half burned. H1 is unchanged.

Fixes for the AI-assisted security review of 24 November 2026. Runtime `spec_version` 9 and a new
`IncentiumSwap` contract on Base.

## What changes

**H1 – deadlines across a chain halt (pallet `ict-swap`).** The pallet now keeps `Downtime`: every gap
between blocks longer than `MaxBlockGap` (120 s) adds its length minus 120 s. A reservation on a sell
order (where the claim on Incentium is the second leg, after the seller has revealed the secret on
Base) gets its deadline extended by the downtime since it was made. Locks keep plain chain time:
there Incentium is the first leg, and extending them would put the seller at risk instead.

**H2 – reservations without cost (pallet and contract).**

- Whoever reserves a fill pays a deposit: 2 % of the fill, at least 2 ICT (Incentium) or 0.5 USDC (Base).
- It is returned when the reservation is claimed.
- If the reservation expires unclaimed, the deposit is held for 24 h. The deposit arbiter (the keeper)
  returns it if the taker did lock on the other chain and only the maker failed to confirm. After 24 h
  anyone can pay it to the maker. The arbiter can only move deposits, never order funds; every
  decision can be checked from the two chains' public data.
- At most one open reservation per recipient (Incentium) or payee (Base) per order.
- The keeper pays the deposit for takers, but only when the taker's wallet already holds (and, on
  Base, has approved) what the fill needs, and only one keeper-funded open fill per taker.

**M8 – fees (contract only, in the same deployment).** Fees accrue in the contract and are sent to the
fee recipient with `withdrawFees()`, so a fee address that cannot receive USDC no longer blocks claims.

## Tests

- `cargo test -p pallet-ict-swap`: 26 passed (19 existing, 7 new: `h1_*`, `h2_*`).
- `forge test` in `swap/base`: 21 passed (16 existing, adapted; 5 new in `ReviewFixes.t.sol`).
- `python -m pytest tests` in `swap/service`: 7 passed (4 existing, adapted; 3 new deposit tests).
- `cargo check -p incentium-runtime` and `cargo clippy -p pallet-ict-swap --all-targets`: clean.

## Rollout

1. **Testnet first.** Build the runtime (CI), upgrade the testnet to spec 9
   (`scripts/upgrade_runtime.py`). No storage migration is needed: the new items start empty;
   reservations made before the upgrade have no deposit and get downtime credit from the upgrade on.
2. Set the arbiter (sudo): `Swap.set_deposit_arbiter(Some(<keeper's Incentium account>))`.
3. Fund the keeper's Incentium account with ICT for deposits (e.g. 50 ICT).
4. Deploy the new contract on Base Sepolia with `DEPOSIT_ARBITER=<keeper's Base address>`
   (`script/Deploy.s.sol`). Fund the keeper's Base address with USDC (e.g. 20 USDC) and a little ETH;
   the keeper approves the contract itself on first use.
5. Update the keeper config (`contract`, `base_start`, `usdc`) and upload the new keeper and page.
6. Run the full flow on testnet, including: no confirmation by the maker (deposit returned), no lock by
   the taker (deposit to maker after 24 h), and a node halt of more than 2 minutes during an open
   reservation (deadline extended).
7. **Mainnet:** repeat steps 1–5. Pause the old Base contract with the pause guardian (claims and
   refunds keep working there) and point the page at the new one.

Consider using a multisig for the fee recipient and pause guardian in the new deployment (review M8);
they are immutable once deployed.
