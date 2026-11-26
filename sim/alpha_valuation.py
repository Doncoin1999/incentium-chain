"""What is an Alpha worth in ICT? Cash-flow valuation and pool simulation (26 Nov 2026).

An analyst values an asset as the present value of what it pays its holder. This script
asks that for one Incentium subnet under four designs:

  0  Today's module: Alpha pays its holder no ICT (bonus goes to miners, validators, owner).
  A  Staker dividend: the validators' 38 % of the subnet's ICT goes to everyone with Alpha
     staked on them, less a validator take (18 %, Bittensor's default).
  B  Pool injection: the same amount of ICT goes into the pool's ICT side instead
     (Bittensor's TAO injection works this way).
  A+R  A, plus Phase 3 user revenue (whitepaper 7.4) used to buy back Alpha.

Fundamental value per Alpha (fully diluted, as an analyst counts future issuance):
    V(t) = sum over days u >= t of  D(u) / S(u) * (1 + r) ** (-(u - t) / 365)
where D = ICT paid to holders per day, S = all Alpha issued by then. Then each day traders
buy or sell in the pool to move its price a fraction K of the way to V. Pool rules are the
recommended ones: stock straight into the pool, partial fills, start price sale / 1,000,000,
start depth 36,000 Alpha, 0.5 % fee half burned.

Calibration (sources in sim/VALUATION.md): Bittensor's 18/41/41 split, 18 % default take,
root yield 4-8 %, alpha supply growth 4.7 % a month, median alpha down 10 % a month in TAO,
all alpha worth about 27 % of TAO, i.e. about 2 years of the TAO that flows to subnets.

Run: python3 sim/alpha_valuation.py
"""
from dataclasses import dataclass
import math

DAY_ALPHA = 7_200.0          # Alpha issued per day before the first halving
ALPHA_CAP = 21_000_000.0
HALVING_DAYS = 1_460         # 4 years at 7,200 a day = 10.5 M
BONUS = 0.5                  # half bonus, half stock
POT_OTHER_SUBNETS = 2_880.0  # ICT a day for all subnets but SN0 (SN0 keeps >= 50 % through 2028)
ICT_HALVING_AT = 1_280       # subnet day of ICT's first halving if subnets open ~day 180 of mainnet
VALIDATOR_SHARE, TAKE = 0.38, 0.18
FEE, BURN = 0.005, 0.5
START_DEPTH, START_DIVISOR = 36_000.0, 1_000_000.0
HORIZON = 20 * 365


def alpha_issued_per_day(day):
    k = day // HALVING_DAYS
    return DAY_ALPHA / (2 ** k)


def supply_path(days):
    s, out = 0.0, []
    for d in range(days):
        s = min(ALPHA_CAP, s + alpha_issued_per_day(d))
        out.append(s)
    return out


def subnet_ict(day, n_subnets):
    return POT_OTHER_SUBNETS / n_subnets / (2 ** ((day + (1_460 - ICT_HALVING_AT)) // 1_460))


@dataclass
class Design:
    name: str
    holder_share: float      # share of the subnet's ICT paid to Alpha holders (dividend)
    inject_share: float      # share of the subnet's ICT put into the pool's ICT side
    revenue_share: float = 0.0   # Phase 3 revenue, as a share of the subnet's ICT, from year 2


DESIGNS = [
    Design("0  Today: no ICT to Alpha holders", 0.0, 0.0),
    Design("A  Staker dividend (38 % less 18 % take)", VALIDATOR_SHARE * (1 - TAKE), 0.0),
    Design("B  Same ICT injected into the pool", 0.0, VALIDATOR_SHARE * (1 - TAKE)),
    Design("A+R  Dividend + revenue buyback (30 %)", VALIDATOR_SHARE * (1 - TAKE), 0.0, 0.30),
]


def fundamental(design, n_subnets, r, circ=None):
    """V(t) for every day, by backward recursion over a 20-year horizon. S(u) is the Alpha that
    shares the payout: fully diluted (all Alpha issued) unless `circ`, the Alpha actually held
    outside the pool in a previous run, is given; after that run's last day it grows with issuance.
    In B the injected ICT backs every Alpha in the same way, so it counts as paid for valuation."""
    S = supply_path(HORIZON)
    if circ:
        last = circ[-1]
        S = list(circ) + [0.0] * (HORIZON - len(circ))
        for d in range(len(circ), HORIZON):
            last = min(ALPHA_CAP, last + alpha_issued_per_day(d)); S[d] = last
        S = [max(x, 1.0) for x in S]
    disc = (1 + r) ** (-1 / 365)
    V = [0.0] * (HORIZON + 1)
    for d in range(HORIZON - 1, -1, -1):
        e = subnet_ict(d, n_subnets)
        paid = e * (design.holder_share + design.inject_share)
        if d >= 365:   # buyback: only the half handed to stakers reaches holders directly;
            paid += e * design.revenue_share * 0.5   # the burned half's ICT stays in the pool
        V[d] = paid / S[d] + disc * V[d + 1]
    return V


@dataclass
class Pool:
    vx: float
    vy: float
    rx: float = 0.0
    ry: float = 0.0

    def X(self): return self.vx + self.rx
    def Y(self): return self.vy + self.ry
    def price(self): return self.X() / self.Y()

    def buy(self, ict):
        """ICT in, Alpha out; partial fill at 99 % of real Alpha. Returns (alpha, ict_paid)."""
        if ict <= 0 or self.ry <= 0:
            return 0.0, 0.0
        net = ict * (1 - FEE)
        out = self.Y() - self.X() * self.Y() / (self.X() + net)
        if out > self.ry * 0.99:
            out = self.ry * 0.99
            net = self.X() * out / (self.Y() - out)
            ict = net / (1 - FEE)
        self.rx += net + ict * FEE * (1 - BURN)
        self.ry -= out
        return out, ict

    def sell(self, alpha):
        """Alpha in, ICT out; paid from real ICT only (partial fill)."""
        if alpha <= 0 or self.rx <= 0:
            return 0.0, 0.0
        net = alpha * (1 - FEE)
        out = self.X() - self.X() * self.Y() / (self.Y() + net)
        if out > self.rx * 0.99:
            out = self.rx * 0.99
            net = self.Y() * out / (self.X() - out)
            alpha = net / (1 - FEE)
        self.ry += net + alpha * FEE * (1 - BURN)
        self.rx -= out
        return out, alpha

    def to_price(self, target):
        """ICT to buy (>0) or Alpha to sell (<0, as alpha) to reach `target`, ignoring fees."""
        k = self.X() * self.Y()
        if target > self.price():
            return math.sqrt(k * target) - self.X(), 0.0
        return 0.0, math.sqrt(k / target) - self.Y()


def simulate(design, n_subnets=10, r=0.20, sale_price=1_000.0, days=4 * 365, K=0.3,
             sell_share=0.7, cohort_day=365, cohort_ict=1_000.0, quiet=False, replay=None, log=None,
             rounds=8):
    """replay: the traders' daily ICT flows from another run (same buyers, other payout);
    log: list that receives this run's daily trader flows."""
    if rounds > 1 and replay is None and (design.holder_share or design.revenue_share):
        # rational traders: value from the Alpha that actually shares the payout, found by
        # iterating until the circulating path the prices imply is the one they assumed
        circ_path = None
        for _ in range(rounds - 1):
            path = []
            _run(design, n_subnets, r, sale_price, days, K, sell_share, cohort_day, cohort_ict,
                 fundamental(design, n_subnets, r, circ_path), None, None, path)
            circ_path = path
        V = fundamental(design, n_subnets, r, circ_path)
    else:
        V = fundamental(design, n_subnets, r)
    rows = _run(design, n_subnets, r, sale_price, days, K, sell_share, cohort_day, cohort_ict, V,
                replay, log, None)
    if not quiet:
        _print(design, n_subnets, r, sale_price, rows)
    return rows


def _run(design, n_subnets, r, sale_price, days, K, sell_share, cohort_day, cohort_ict, V,
         replay, log, circ_out):
    p0 = sale_price / START_DIVISOR
    pool = Pool(vx=p0 * START_DEPTH, vy=START_DEPTH)
    circ = 0.0               # Alpha held outside the pool (all of it staked)
    burned = 0.0
    net_ict_in = 0.0         # ICT put in by buyers less ICT taken out by sellers
    cohort_alpha = cohort_div = 0.0
    rows, multiples = [], []
    for d in range(days):
        a = alpha_issued_per_day(d)
        e = subnet_ict(d, n_subnets)
        pool.ry += a * (1 - BONUS)                       # stock straight into the pool
        bonus = a * BONUS
        circ += bonus
        # bonus holders (miners 50 %, validators 38 %) sell part; owner (12 %) vests and holds
        got, used = pool.sell(bonus * 0.88 * sell_share)
        circ -= used; net_ict_in -= got
        # ICT to holders
        div = e * design.holder_share
        if design.inject_share:
            pool.rx += e * design.inject_share
        if d >= 365 and design.revenue_share:            # buyback: half burned, half to stakers
            bought, _ = pool.buy(e * design.revenue_share)
            if cohort_alpha and circ > 0:
                cohort_alpha += bought / 2 * cohort_alpha / circ
            burned += bought / 2; circ += bought / 2
        if cohort_alpha and circ > 0:      # dividends reinvested in Alpha at the market price
            got_ict = div * cohort_alpha / circ
            cohort_div += got_ict
            cohort_alpha += got_ict / pool.price()
        if replay is None:
            # traders move the price part of the way to fundamental value
            target = max(pool.price() + K * (V[d] - pool.price()), 1e-12)
            ict_in, alpha_out = pool.to_price(target)
            flow = 0.0
            if ict_in > 0:
                got, paid = pool.buy(ict_in)
                circ += got; net_ict_in += paid; flow = paid
            elif alpha_out > 0:
                got, used = pool.sell(min(alpha_out, circ))
                circ -= used; net_ict_in -= got; flow = -got
            if log is not None:
                log.append(flow)
        else:
            flow = replay[d]
            if flow > 0:
                got, paid = pool.buy(flow)
                circ += got; net_ict_in += paid
            elif flow < 0:
                got, used = pool.sell(min(-flow / pool.price(), circ))
                circ -= used; net_ict_in -= got
        if d == cohort_day:              # a small buyer at the market price (price-taker)
            cohort_alpha = cohort_ict / pool.price()
        cap = circ * pool.price()
        multiples.append(cap / (e * 365))
        if circ_out is not None:
            circ_out.append(circ)
        if d + 1 in (30, 90, 180, 365, 730, 1460):
            held = cohort_alpha * pool.price()
            yld = div * 365 / circ / pool.price() if circ > 0 and pool.price() > 0 else 0.0
            yrs = (d - cohort_day) / 365
            ann = (held / cohort_ict) ** (1 / yrs) - 1 if yrs > 0.2 and held > 0 else float("nan")
            rows.append((d + 1, pool.price(), V[d], pool.rx, net_ict_in, circ, cap, cap / (e * 365),
                         bonus * pool.price(), e, held, ann, pool.ry, yld))
    return rows


def _print(design, n_subnets, r, sale_price, rows):
    if True:
        print(f"\n{design.name}  | {n_subnets} subnets, r = {r:.0%}, slot sold for {sale_price:,.0f} ICT")
        print(f"{'day':>5} {'price':>9} {'value V':>9} {'ICT in pool':>12} {'net ICT in':>11} "
              f"{'Alpha out':>10} {'mkt cap':>9} {'cap/yr ICT':>10} {'bonus ICT/d':>11} {'yield':>6} {'cohort':>8} {'p.a.':>7}")
        for (day, pr, v, rx, nin, c, cap, mult, bv, e, held, ann, ry, yld) in rows:
            print(f"{day:>5} {pr:>9.5f} {v:>9.5f} {rx:>12,.0f} {nin:>11,.0f} {c:>10,.0f} {cap:>9,.0f} "
                  f"{mult:>10.2f} {bv:>11,.1f} {yld:>6.0%} {held:>8,.0f} {ann:>7.1%}")




if __name__ == "__main__":
    print("=" * 110)
    print("Base: 10 subnets share 2,880 ICT a day (288 each), required return 20 %, slot sold for 1,000 ICT")
    print("cohort = 1,000 ICT put in on day 366, dividends reinvested, at market price; p.a. = total return from then; yield = ICT dividends a year / price")
    flows = []
    simulate(DESIGNS[0])
    simulate(DESIGNS[1], log=flows)
    print("\n(B is run with exactly the same buyers and sellers as A: same ICT, paid into the pool instead)")
    simulate(DESIGNS[2], replay=flows)
    simulate(DESIGNS[3])

    print("\n" + "=" * 110)
    print("Sensitivity, design A: price, ICT in the pool and market cap (in years of the subnet's ICT)")
    print(f"{'subnets':>8} {'ICT/day':>8} {'r':>5} {'price d365':>11} {'ICT in pool d365':>17} {'ICT in pool d1460':>18} {'cap/yr d365':>12} {'cohort p.a. 4y':>15}")
    for n in (5, 10, 30):
        for r in (0.08, 0.20, 0.50):
            rows = simulate(DESIGNS[1], n, r, quiet=True)
            y1 = [x for x in rows if x[0] == 365][0]; y4 = rows[-1]
            print(f"{n:>8} {POT_OTHER_SUBNETS / n:>8,.0f} {r:>5.0%} {y1[1]:>11.5f} {y1[3]:>17,.0f} {y4[3]:>18,.0f} {y1[7]:>12.2f} {y4[11]:>15.1%}")

    print("\nShare of the validators' 38 % passed to stakers (10 subnets, r 20 %), after 1 year:")
    for share, label in ((0.38 * 0.82, "all of it less 18 % take"), (0.19 * 0.82, "half, less take"), (0.38 * 0.82 / 4, "a quarter")):
        d = Design("A'", share, 0.0)
        y1 = [x for x in simulate(d, 10, 0.20, quiet=True) if x[0] == 365][0]
        print(f"  {label:<26} {share:>6.1%} of subnet ICT: price {y1[1]:.5f}, ICT in pool {y1[3]:>7,.0f}, "
              f"bonus worth {y1[8]:>5.1f} ICT/day, yield {y1[13]:.0%}")

    print("\nSlot price only sets the start (design A, 10 subnets, r 20 %):")
    for sp in (100.0, 1_000.0, 50_000.0):
        rows = simulate(DESIGNS[1], 10, 0.20, sale_price=sp, quiet=True)
        print(f"  sold for {sp:>8,.0f}: price day 30 {rows[0][1]:.5f}, day 90 {rows[1][1]:.5f}, day 365 {rows[3][1]:.5f}")

    print("\nBonus sellers: share of the bonus sold at once (design A, 10 subnets, r 20 %), after 1 year:")
    for ss in (0.3, 0.7, 1.0):
        y1 = [x for x in simulate(DESIGNS[1], 10, 0.20, sell_share=ss, quiet=True) if x[0] == 365][0]
        print(f"  sell {ss:.0%}: price {y1[1]:.5f}, ICT in pool {y1[3]:,.0f}, bonus worth {y1[8]:,.1f} ICT/day")
