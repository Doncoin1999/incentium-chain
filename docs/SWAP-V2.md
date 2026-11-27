# Swap version 2: no fee, no pause, no arbiter

Decided 26 Nov 2026. Replaces the first mainnet deployment (contract
`0x118B45764555B57c0886D53978de4aa0D0D32e55`, runtime spec 8) and supersedes the arbiter design in
`REVIEW-FIXES-H1-H2.md`. Goal: after deployment no person holds any key with rights over the swap.

## What changes

| | Version 1 (mainnet now) | Version 2 |
| --- | --- | --- |
| Fee | 0.1 % of the USDC side, to the operator's MetaMask (immutable) | None |
| Pause | Pause guardian = operator's MetaMask (immutable) | None |
| Reservations | Free: anyone can reserve fills and let them run out, blocking orders (review H2) | Deposit 2 %, at least 2 ICT / 0.5 USDC, one open reservation per recipient per order |
| Unclaimed deposit | (v1 review fix: keeper as arbiter returns it, else paid to the maker) | At expiry half goes back to its payer and half is burned, no arbiter: nobody gains from a failed trade (decided 26 Nov 2026) |
| Halt of Incentium | Reservation deadlines pass during a halt (review H1) | Extended by the chain's downtime |
| Privileged addresses | Fee recipient, pause guardian | None, on either chain |

On Base the burned half stays in the contract for good (`burnedDeposits`); no call can move it. It
is not sent to a dead address, so a USDC blacklist of that address cannot block expiry. A deposit
refund (the full deposit on a claim, the returned half on expiry) that USDC refuses, for example to
a blacklisted address, is recorded in `owed` and paid later by `claimOwed(address)`, which anyone may
call; it never blocks a claim or an expiry. On Incentium the burned half of the held ICT is burned
(`burn_held`), reducing total issuance, and the other half released.

**Trade-off.** The chains cannot see each other, so an expiry cannot tell why the trade failed. A
taker who blocks an order (reserves and never locks) loses half the deposit, 1 % of the fill. So
does an honest taker whose maker never confirmed within the 12 h window; the taker's own funds on
the other chain are refunded as before. The maker gains nothing either way, so there is no reason
to let a trade fail on purpose. The keeper pays deposits for takers who already hold what the fill
needs, so in practice the keeper carries this cost. Every case can be classified afterwards from the
two chains' public data, and losses from a maker's failure can be compensated from the treasury by
referendum.

## Code

| Part | Change | Tests |
| --- | --- | --- |
| `swap/base/src/IncentiumSwap.sol` | Constructor `(usdc, genesisTime)` only; fee, pause, arbiter, `withdrawFees`, `returnDeposit`, `payDepositToMaker` removed; half-burn at expiry (`DepositHalfBurned`); refused refunds owed (`owed`, `claimOwed`) | `forge test`: 21 passed (incl. 512-run fund-conservation fuzz) |
| `pallets/ict-swap` | `DepositArbiter`, `return_deposit`, `pay_deposit_to_maker`, `set_deposit_arbiter`, `DepositDisputeWindow` removed; half the deposit released and half burned in `expire_reservation`; `DepositHalfBurned` event | `cargo test -p pallet-ict-swap`: 25 passed; clippy clean |
| `runtime` | Config without `DepositDisputeWindow` | `cargo check -p incentium-runtime` |
| `swap/service` | No arbiter loop; `DepositHalfBurned` handled; ABI regenerated | `pytest`: 6 passed against the real contract on anvil |
| `swap/web` | "No fee"; deposit text; reservations ABI | — |

## Rollout

1. **Testnet:** build a testnet runtime with this pallet (the testnet's runtime 12 still has the
   version-1 pallet) and upgrade with sudo. No migration: the pallet's H1/H2 items start empty.
2. **Base Sepolia:** deploy with a fresh deployer key (it gets no rights):
   `USDC=0x036CbD53842c5426634e7929541eC2318f3dCF7e GENESIS_TIME=<testnet block 1 time> forge script script/Deploy.s.sol --rpc-url https://sepolia.base.org --broadcast --private-key $(cat ~/base-deployer.key)`
3. Update the testnet keeper config (`contract`, `base_start`) and upload keeper and page; fund the
   keeper with a little ICT, USDC and ETH for deposits and gas.
4. Run the full flow, including: maker never confirms, taker never locks (both: half the deposit
   burned, funds refunded), and a node halt over 2 minutes during an open reservation.
5. **Audit** of contract and pallet together with the Alpha pools.
6. **Mainnet:** the pallet arrives with mainnet runtime 12 (sudo, before day 90). Deploy the contract
   on Base mainnet (`USDC=0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913`), switch keeper and page, and
   pause the old contract with its guardian one last time: new orders stop there, claims and refunds
   keep working.
7. Governance attests the new contract on chain (`attest_swap`), one of the SN1 criteria.
