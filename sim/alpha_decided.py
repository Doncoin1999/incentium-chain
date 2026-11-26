"""The Alpha pool as decided on 26 Nov 2026, simulated.

Rules:
- Alpha holders are paid in ICT: the validators' 38 % of the subnet's ICT, less a take of 0-18 %
  (18 % assumed here), goes to the Alpha staked with each validator.
- No Alpha exists before it is issued. For the first 10 days the pool is closed: stock collects
  (3,600 a day), bonus is paid but cannot be sold, and the treasury half of the auction price is
  held for the pool.
- On day 10 the pool opens as an ordinary x * y = k pool with real reserves only:
    start price P0 = value of an Alpha by the valuation formula (r = 20 %, fully diluted),
    ICT = min(treasury half, 36,000 * P0), Alpha = ICT / P0;
  the rest of the stock waits and is released like the daily stock.
- Daily stock is released block by block; it is held back while the price is more than 20 %
  below its 30-day average, and what was held back is released later, at most one extra day's
  stock per day.
- Trades carry a price limit instead of partial fills; 0.5 % fee, half burned, half kept.

Traders buy or sell toward the value of an Alpha (dividends on the Alpha actually outside the
pool, found by iteration as in sim/alpha_valuation.py). Run: python3 sim/alpha_decided.py
"""
import math
from alpha_valuation import (DESIGNS, fundamental, alpha_issued_per_day, subnet_ict, FEE, BURN)

A = DESIGNS[1]
WAIT_DAYS, OPEN_DEPTH_DAYS, HOLD_DROP, AVG_DAYS = 10, 10, 0.20, 30


class RealPool:
    def __init__(self):
        self.x = 0.0  # ICT
        self.y = 0.0  # Alpha

    def price(self):
        return self.x / self.y if self.y > 0 else 0.0

    def buy(self, ict):
        if ict <= 0 or self.y <= 0:
            return 0.0
        net = ict * (1 - FEE)
        out = self.y - self.x * self.y / (self.x + net)
        self.x += net + ict * FEE * (1 - BURN)
        self.y -= out
        return out

    def sell(self, alpha):
        if alpha <= 0 or self.x <= 0:
            return 0.0
        net = alpha * (1 - FEE)
        out = self.x - self.x * self.y / (self.y + net)
        self.y += net + alpha * FEE * (1 - BURN)
        self.x -= out
        return out

    def ict_to(self, target):
        """ICT to buy (> 0) or Alpha to sell (< 0) to reach `target`, ignoring fees."""
        k = self.x * self.y
        if target > self.price():
            return math.sqrt(k * target) - self.x
        return -(math.sqrt(k / target) - self.y)


def run(sale_price=1_000.0, n_subnets=10, r=0.20, days=4 * 365, K=0.3, sell_share=0.7,
        holdback=True, shock_day=None, shock_share=0.5, V=None, circ_out=None, cohort_day=365,
        no_buyers=None):
    if V is None:
        V = fundamental(A, n_subnets, r)
    pool = RealPool()
    p0 = fundamental(A, n_subnets, r)[WAIT_DAYS]          # on-chain formula: fully diluted
    reserved_ict = sale_price / 2
    waiting = 0.0         # stock not yet in the pool
    locked_bonus = 0.0    # bonus paid before opening, sellable from opening
    circ = 0.0
    prices = []
    cohort_alpha = 0.0
    out = {"p0": p0, "rows": [], "held_days": 0}
    for d in range(days):
        a = alpha_issued_per_day(d)
        e = subnet_ict(d, n_subnets)
        waiting += a / 2
        bonus = a / 2
        circ += bonus
        div = e * A.holder_share
        if cohort_alpha and circ > 0 and pool.price() > 0:
            cohort_alpha += div * cohort_alpha / circ / pool.price()
        if d < WAIT_DAYS:
            locked_bonus += bonus * 0.88 * sell_share
        elif d == WAIT_DAYS:
            seed = min(reserved_ict, OPEN_DEPTH_DAYS * 3_600 * p0)
            pool.x, pool.y = seed, seed / p0
            waiting -= pool.y
            out["seed"] = (pool.x, pool.y)
        if d >= WAIT_DAYS:
            # stock release: today's share plus at most one extra day of what waits
            avg = sum(prices[-AVG_DAYS:]) / len(prices[-AVG_DAYS:]) if prices else pool.price()
            if holdback and pool.price() < (1 - HOLD_DROP) * avg:
                out["held_days"] += 1
            else:
                rel = min(waiting, a / 2 * 2)
                pool.y += rel; waiting -= rel
            # bonus sellers: today's bonus and the backlog from the closed days (spread over 10 days)
            backlog = min(locked_bonus, out.setdefault("backlog_daily", locked_bonus / 10))
            locked_bonus -= backlog
            sold = bonus * 0.88 * sell_share + backlog
            pool.sell(sold); circ -= sold
            # traders toward value
            if d == shock_day:   # panic: a share of all holders sell at once
                dump = circ * shock_share
                pool.sell(dump); circ -= dump
            elif no_buyers and no_buyers[0] <= d < no_buyers[1]:
                pass     # nobody buys or sells but the bonus sellers
            else:
                target = max(pool.price() + K * (V[d] - pool.price()), 1e-12)
                f = pool.ict_to(target)
                if f > 0:
                    circ += pool.buy(f)
                elif f < 0:
                    s = min(-f, circ); pool.sell(s); circ -= s
            prices.append(pool.price())
            if d == cohort_day:
                cohort_alpha = 1_000.0 / pool.price()
        if circ_out is not None:
            circ_out.append(max(circ, 1.0))
        if d + 1 in (10, 11, 30, 90, 180, 181, 200, 240, 270, 365, 730, 1460):
            held = cohort_alpha * pool.price()
            yrs = (d - cohort_day) / 365
            ann = (held / 1_000) ** (1 / yrs) - 1 if yrs > 0.2 and held > 0 else float("nan")
            out["rows"].append((d + 1, pool.price(), V[d], pool.x, pool.y, waiting, circ, ann))
    return out


def solve(**kw):
    """Iterate the traders' value on the circulating path until it settles."""
    n, r = kw.get("n_subnets", 10), kw.get("r", 0.20)
    path = None
    for _ in range(8):
        p = []
        run(**kw, V=fundamental(A, n, r, path), circ_out=p)
        path = p
    return run(**kw, V=fundamental(A, n, r, path))


def show(title, res):
    print(f"\n{title}")
    print(f"  start price P0 {res['p0']:.5f} ICT/Alpha; opened with {res['seed'][0]:,.0f} ICT and "
          f"{res['seed'][1]:,.0f} Alpha; stock held back on {res['held_days']} days")
    print(f"  {'day':>5} {'price':>9} {'value':>9} {'ICT in pool':>12} {'Alpha in pool':>14} "
          f"{'stock waiting':>14} {'Alpha out':>11} {'p.a. from d366':>15}")
    for (day, pr, v, x, y, w, c, ann) in res["rows"]:
        print(f"  {day:>5} {pr:>9.5f} {v:>9.5f} {x:>12,.0f} {y:>14,.0f} {w:>14,.0f} {c:>11,.0f} {ann:>15.1%}")


if __name__ == "__main__":
    show("1. Base: slot sold for 1,000 ICT (500 ICT to the pool), 10 subnets, r 20 %", solve())
    show("2. Slot sold for 10,000 ICT (pool ICT capped at 36,000 x P0)", solve(sale_price=10_000.0))
    show("3. Panic on day 180: half of all holders sell at once, stock held back", solve(shock_day=180))
    show("4. Same panic without holding stock back", solve(shock_day=180, holdback=False))
    show("6. No buyers from day 180 to 240, stock held back", solve(no_buyers=(180, 240)))
    show("7. No buyers from day 180 to 240, stock released anyway", solve(no_buyers=(180, 240), holdback=False))
    show("5. 30 subnets (96 ICT a day each)", solve(n_subnets=30))
