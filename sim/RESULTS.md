# Alpha pools: simulation results (25 Nov 2026)

`python3 sim/alpha_pools.py` (and the variants below) simulate one subnet for a year under the rules decided on
25 Nov 2026: 7,200 Alpha a day, half bonus (50/38/12) and half stock; miners and validators sell 70 % of their
bonus each day, the owner holds; stakers put a fixed amount of ICT in each day; 0.5 % fee, half burned.

## Finding 1: a daily cap on stock sales stalls trading

With the stock sold only at the current price and at most one day's issuance (7,200 Alpha) a day, a pool that
starts at 0.001 ICT can take in only about 7 ICT a day. The price rises only as ICT comes in, so it stays low,
and stakers who want to put in 300 ICT a day cannot buy at all (scenario A: 0 ICT in the pool after a year).

## Finding 2: a deep virtual Alpha reserve makes the price move far too slowly

Even without the cap, a start curve as deep as 1,000,000 Alpha makes the pool behave as if it held far more
Alpha than it does. With 3,000 ICT a day of demand only about 8 ICT a day gets filled.

## What works: stock into the pool each epoch, partial fills, a shallow start curve

| Design (slot sold for 1,000 ICT) | 300 ICT/day: price after 30 / 90 / 365 days | Stakers' Alpha worth vs ICT paid, day 90 | 3,000 ICT/day: ICT in pool after 90 days |
| --- | --- | --- | --- |
| Decided: daily cap, depth 1,000,000 | 0.0010 / 0.0010 / 0.0010 (no trades) | no stakers | 0 |
| No cap, depth 1,000,000 | 0.0011 / 0.0014 / 0.0037 | 717 / 616 | 380 |
| No cap, no virtual Alpha (vx = sale price) | 0.073 / 0.074 / 0.074 | 26,233 / 27,000 | 115,141 |
| **No cap, start price sale/1,000,000, depth 36,000 (recommended)** | 0.013 / 0.074 / 0.074 | 33,067 / 16,873 | 47,766 |

- Whatever the start, the price settles where demand meets supply (about 0.074 ICT at 300 ICT a day, 0.74 at
  3,000). The start price only shapes the first one to three months.
- Starting high (vx = sale price) works like a Dutch auction: early stakers in a slot bought for 50,000 ICT
  lose about half in the first month as the price falls. Starting at sale price / 1,000,000 with a shallow
  curve does the opposite: early stakers gain as the price rises to its level.
- Risks stay as stated: when half the stakers leave at once the price falls about 80 % that day (scenario D);
  when stakers stop coming, the daily stock pushes the price down about 85 % over six months (scenario F).
- Nobody can sell bonus Alpha until someone has bought: a pool with no stakers holds no ICT (scenario E).

The library (`primitives/ict-economics/src/alpha.rs`) implements the recommended design. It needs your approval
before the whitepaper and specification are changed.
