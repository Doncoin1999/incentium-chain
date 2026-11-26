# What is an Alpha worth in ICT? (26 Nov 2026)

`python3 sim/alpha_valuation.py`. Values Alpha the way an analyst would: the present value of the ICT
an Alpha pays its holder, after dilution. Traders in the pool buy or sell toward that value each day,
so the ICT that flows into the pool is an output, not an assumption. Pool rules are the recommended
ones (stock straight into the pool, partial fills, start price sale / 1,000,000, depth 36,000 Alpha,
0.5 % fee half burned). The value is computed on the Alpha actually outside the pool (it alone gets
paid) and iterated until the path the prices imply is the path assumed. Base case: 10 subnets share
2,880 ICT a day (SN0 keeps at least half of issuance through 2028), required return 20 % a year.

## Designs

| Design | What an Alpha holder gets |
| --- | --- |
| 0 Today's module | No ICT. Bonus Alpha goes to miners, validators and owner. |
| A Staker dividend | The validators' 38 % of the subnet's ICT, less an 18 % validator take (Bittensor's default): 31.2 % of the subnet's ICT, paid in ICT to Alpha staked with each validator |
| B Pool injection | The same ICT paid into the pool's ICT side (Bittensor's TAO injection works this way) |
| A+R | A, plus Phase 3 user revenue (30 % of the subnet's ICT from year 2) buying back Alpha: half burned, half to stakers |

## Results, one subnet with 288 ICT a day, r = 20 %

| | Day 90 | Day 365 | Day 1,460 |
| --- | --- | --- | --- |
| **0** price (ICT/Alpha) | 0.0001 | 0.00003 | 0.00001 |
| **0** ICT in pool | 0 | 0 | 0 |
| **A** price | 0.0365 | 0.0229 | 0.0063 |
| **A** ICT in pool | 4,722 | 16,416 | 25,913 |
| **A** dividend yield (ICT a year / price) | 162 % | 74 % | 40 % |
| **A** bonus Alpha worth, ICT a day | 131 | 82 | 23 |
| **A+R** price | 0.0481 | 0.0384 | 0.0119 |
| **A+R** ICT in pool | 5,919 | 25,347 | 46,440 |

Total return of 1,000 ICT staked on day 366, dividends reinvested: A 21.5 % a year, A+R 20.6 % (the
model is consistent: holders earn the required return). Today's design: -36.5 % a year.

B, run with exactly the same buyers and sellers as A: more ICT ends up in the pool (109,416 after
four years against 25,913) and the price is higher, but the holder earns -20 % a year, because the ICT
stays in the pool and backs the unsold stock as much as the holders' Alpha.

## Sensitivity (A, day 365)

| Subnets | ICT a day each | r 8 % | r 20 % | r 50 % |
| --- | --- | --- | --- | --- |
| 5 | 576 | 0.0551 / 37,905 | 0.0454 / 31,738 | 0.0338 / 24,234 |
| 10 | 288 | 0.0277 / 19,586 | 0.0229 / 16,416 | 0.0170 / 12,554 |
| 30 | 96 | 0.0094 / 6,868 | 0.0077 / 5,765 | 0.0058 / 4,418 |

(price in ICT per Alpha / ICT in the pool). Price and pool scale with the ICT passed to holders: half
of the validators' share gives half the price and pool. The slot's sale price only shapes the first
months (day 365: 0.0223 at 100 ICT, 0.0229 at 1,000, 0.0241 at 50,000). How much bonus miners sell at
once hardly matters (30-100 %: price 0.0231-0.0227).

## Comparison with Bittensor

- Same split: 18 % owner, 41 % miners, 41 % validators and their stakers; default validator take 18 %.
- Bittensor pays its stakers in newly issued alpha. A 90-day audit of 127 subnets to June 2026 found a
  median marketed yield of 54 % a year, alpha supply up 4.7 % a month, alpha down 10.1 % a month in
  TAO and a median staker return of -7.2 % in 30 days; 73 % of subnets lost money in TAO.
  Model A has similar headline numbers (yield 40-74 %, supply up about 5 % a month and price down about
  4 % a month in year 2) but pays the yield in ICT, so the holder's total return is the required return.
- All subnet alpha was worth about 27 % of TAO in March 2026: about 2 years of the TAO that flows to
  subnets. Model A prices Alpha at 0.42 years of the subnet's ICT (1.36 years of the ICT its holders
  get) after one year: well below what Bittensor's market pays.

Sources: docs.learnbittensor.org (emissions, delegation), docs.bittensor.com (dynamic TAO emission),
Flowsniper Research "The Yield Illusion" (June 2026), The Block and CoinGecko on the March 2026 alpha
market cap, Cointelegraph/The Block on the December 2025 halving (3,600 TAO a day).

## Model limits

One subnet at a time with a fixed share of issuance (in the chain, the share follows the ICT in the
pool, which adds a feedback loop; whitepaper 7.1 averages it over 90 days). Traders are rational and
move 30 % of the way to value a day; no speculation, no panics (see sim/RESULTS.md for runs). Revenue in
A+R is an assumption (Bittensor's subnet revenue in Q1 2026 was estimated, unaudited, at about $43M;
by our rough calculation, about a third of the value of the TAO issued that quarter).
