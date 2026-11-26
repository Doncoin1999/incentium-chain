"""Simulation of an Incentium subnet's Alpha pool (spec Section 7, decided 25 Nov 2026).

Same rules as primitives/ict-economics/src/alpha.rs, in floating point:
- 7,200 Alpha a day (first halving far beyond one year), 50 % bonus, 50 % stock.
- Bonus 50/38/12 to miners / validators / owner; owner's part vests over 180 days.
- Pool: constant product over X = vx + rx and Y = vy + ry; vx = sale price, vy = 1,000,000.
- A purchase first takes stock at the current price (at most 7,200 Alpha a day),
  then trades on the curve against real Alpha only. Sales are paid from real ICT only.
- Fee 0.5 %, half burned, half left in the pool.

Run: python3 sim/alpha_pools.py
"""
from dataclasses import dataclass, field

FEE, BURN, VY, CAP, DAY_ALPHA, BONUS = 0.005, 0.5, 1_000_000.0, 7_200.0, 7_200.0, 0.5


@dataclass
class Pool:
    vx: float
    rx: float = 0.0
    ry: float = 0.0
    stock: float = 0.0
    sold_today: float = 0.0

    def x(self): return self.vx + self.rx
    def y(self): return VY + self.ry
    def price(self): return self.x() / self.y()

    def buy(self, ict):
        net = ict * (1 - FEE); kept = ict * FEE * (1 - BURN)
        a = min(net * self.y() / self.x(), CAP - self.sold_today, self.stock)
        self.ry += a; self.stock -= a; self.sold_today += a
        out = self.y() - self.x() * self.y() / (self.x() + net)
        if out > self.ry:  # cannot fill: undo the injection
            self.ry -= a; self.stock += a; self.sold_today -= a
            return 0.0
        self.rx += net + kept; self.ry -= out
        return out

    def max_buy(self):
        """Largest ICT purchase that fills now (binary search)."""
        lo, hi = 0.0, 1e9
        for _ in range(60):
            mid = (lo + hi) / 2
            c = Pool(**self.__dict__)
            (lo, hi) = (mid, hi) if c.buy(mid) > 0 or mid == 0 else (lo, mid)
        return lo

    def sell(self, alpha):
        net = alpha * (1 - FEE); kept = alpha * FEE * (1 - BURN)
        out = self.x() - self.x() * self.y() / (self.y() + net)
        if out > self.rx:
            out = 0.0
            return 0.0
        self.ry += net + kept; self.rx -= out
        return out


def run(name, sale_price, demand, sell_share, days=365, run_day=None, run_share=0.5):
    """demand(day) = ICT stakers want to put in per day; sell_share = share of
    the day's bonus that miners and validators sell at once."""
    p = Pool(vx=sale_price)
    staker_alpha = staker_cost = 0.0
    bonus_value = []
    rows = []
    for day in range(days):
        p.sold_today = 0.0
        p.stock += DAY_ALPHA * (1 - BONUS)
        bonus = DAY_ALPHA * BONUS
        # stakers buy, in 20 slices over the day
        want = demand(day)
        for _ in range(20):
            got = p.buy(want / 20)
            if got > 0:
                staker_alpha += got; staker_cost += want / 20
        # miners and validators sell part of their bonus (owner holds, vesting)
        to_sell = bonus * 0.88 * sell_share
        sold_for = 0.0
        for _ in range(20):
            sold_for += p.sell(to_sell / 20)
        bonus_value.append(bonus * p.price())
        if run_day is not None and day == run_day:
            out = p.sell(staker_alpha * run_share)
            if out > 0:
                staker_alpha *= (1 - run_share)
        if day in (0, 29, 89, 179, 364) or (run_day is not None and day in (run_day - 1, run_day)):
            rows.append((day + 1, p.price(), p.rx, p.stock, staker_alpha * p.price(), staker_cost, sold_for))
    print(f"\n{name}: sale price {sale_price:,.0f} ICT, start price {sale_price / VY:.6f} ICT/Alpha")
    print(f"{'day':>4} {'price ICT':>11} {'ICT in pool':>12} {'stock':>11} {'stakers worth':>14} {'stakers paid':>13} {'bonus sold for/day':>19}")
    for d, pr, rx, st, sv, sc, sf in rows:
        print(f"{d:>4} {pr:>11.6f} {rx:>12,.0f} {st:>11,.0f} {sv:>14,.0f} {sc:>13,.0f} {sf:>19,.1f}")
    return p


if __name__ == "__main__":
    run("A. Healthy subnet: 300 ICT a day staked, bonus sellers sell 70 %",
        1_000, lambda d: 300, 0.7)
    run("B. Quiet subnet: 10 ICT a day staked, bonus sellers sell 70 %",
        1_000, lambda d: 10, 0.7)
    run("C. Popular slot bought for 50,000 ICT, 300 ICT a day staked",
        50_000, lambda d: 300, 0.7)
    run("D. Healthy subnet, half the stakers leave on day 180",
        1_000, lambda d: 300, 0.7, run_day=180, run_share=0.5)
    run("E. Nobody stakes: can miners sell their bonus at all?",
        1_000, lambda d: 0, 0.7, days=30)


# ---------------------------------------------------------------------------
# Variant B: the stock goes straight onto the curve each epoch (no daily cap).
# Buyers move along the curve and pay for their own price impact.

@dataclass
class CurvePool(Pool):
    def add(self, alpha):
        self.ry += alpha

    def buy(self, ict):
        """Partial fill: if the pool's real Alpha runs out, the buyer gets 99 % of
        it and pays only for that. Returns Alpha received; self.last_paid = ICT paid."""
        net = ict * (1 - FEE)
        out = self.y() - self.x() * self.y() / (self.x() + net)
        if out > self.ry:
            out = self.ry * 0.99
            if out <= 0:
                self.last_paid = 0.0
                return 0.0
            net = self.x() * out / (self.y() - out)
            ict = net / (1 - FEE)
        kept = ict * FEE * (1 - BURN)
        self.rx += net + kept; self.ry -= out
        self.last_paid = ict
        return out


def run_curve(name, sale_price, demand, sell_share, days=365, run_day=None, run_share=0.5):
    p = CurvePool(vx=sale_price)
    staker_alpha = staker_cost = 0.0
    rows = []
    for day in range(days):
        for _ in range(20):  # issuance per epoch
            p.add(DAY_ALPHA * (1 - BONUS) / 20)
        bonus = DAY_ALPHA * BONUS
        want = demand(day)
        for _ in range(20):
            got = p.buy(want / 20)
            if got > 0:
                staker_alpha += got; staker_cost += p.last_paid
        sold_for = 0.0
        to_sell = bonus * 0.88 * sell_share
        for _ in range(20):
            sold_for += p.sell(to_sell / 20)
        if run_day is not None and day == run_day:
            if p.sell(staker_alpha * run_share) > 0:
                staker_alpha *= (1 - run_share)
        if day in (0, 29, 89, 179, 364) or (run_day is not None and day in (run_day - 1, run_day)):
            rows.append((day + 1, p.price(), p.rx, p.ry, staker_alpha * p.price(), staker_cost, sold_for))
    print(f"\n{name}: sale price {sale_price:,.0f} ICT, start price {sale_price / VY:.6f} ICT/Alpha")
    print(f"{'day':>4} {'price ICT':>11} {'ICT in pool':>12} {'Alpha in pool':>13} {'stakers worth':>14} {'stakers paid':>13} {'bonus sold for/day':>19}")
    for d, pr, rx, ry, sv, sc, sf in rows:
        print(f"{d:>4} {pr:>11.6f} {rx:>12,.0f} {ry:>13,.0f} {sv:>14,.0f} {sc:>13,.0f} {sf:>19,.1f}")


if __name__ == "__main__":
    print("\n=================== Variant B: stock onto the curve ===================")
    run_curve("A. Healthy subnet: 300 ICT a day staked", 1_000, lambda d: 300, 0.7)
    run_curve("B. Quiet subnet: 10 ICT a day staked", 1_000, lambda d: 10, 0.7)
    run_curve("C. Slot bought for 50,000 ICT, 300 ICT a day staked", 50_000, lambda d: 300, 0.7)
    run_curve("D. Healthy subnet, half the stakers leave on day 180", 1_000, lambda d: 300, 0.7, run_day=180)
    run_curve("E. Nobody stakes", 1_000, lambda d: 0, 0.7, days=30)
    run_curve("F. Healthy subnet, stakers stop on day 180", 1_000, lambda d: 300 if d < 180 else 0, 0.7)
