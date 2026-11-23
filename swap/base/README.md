# IncentiumSwap (Base)

USDC side of ICT <-> USDC atomic swaps (Incentium Swap Technical Specification v0.1, Section 6).
No owner, no upgrade; a pause guardian can stop new orders, reservations and locks, never
claims, refunds or cancellations.

    forge install foundry-rs/forge-std --no-git   # once; lib/ is not committed
    forge test

Fixed in this version: fee 0.1 %, order size caps 100 / 1,000 / 5,000 ICT (until day 90 /
day 120 / after, counted from Incentium genesis), at most 16 fills and 3 open reservations per
order, order lifetime up to 24 h, reservation 19 h, lock up to 12 h, minimum fill 5 USDC.

Hashes are SHA-256 of a 32-byte secret, the same on both chains (test vector in the tests).
