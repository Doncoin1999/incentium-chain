//! Alpha tokens and subnet pools (spec Section 7, decided 25 Nov 2026).
//!
//! - Issuance per subnet is exactly ICT's: the same block reward and halving
//!   thresholds, counted from the subnet's own first block (`emission`).
//! - Each epoch's issuance is split: `bonus_ppm` is paid to the subnet's
//!   miners, validators and owner; the rest goes to the pool's stock.
//! - The pool is a constant-product curve over X = vx + rx ICT and Y = vy + ry
//!   Alpha. vx and vy are virtual: they only shape the price curve (start price
//!   = sale price / 1,000,000, depth START_DEPTH). rx and ry are real and are
//!   the only amounts that can ever be paid out.
//! - Each epoch's stock goes straight into the pool; staking is buying on the
//!   curve, unstaking is selling. A trade too large for the real reserve fills
//!   partially. (Revised after simulation, see sim/RESULTS.md; the decision of
//!   25 Nov 2026 had a daily cap on stock sales, which stalled trading.)
//!
//! All amounts in base units (1 Alpha = 1 ICT = 10^9). Every division rounds in
//! the pool's favour.

use crate::{emission, mul_div, BLOCKS_PER_DAY, PPM, UNIT};

/// Virtual Alpha at the start of every pool: 1,000,000 Alpha.
pub const START_ALPHA: u128 = 1_000_000 * UNIT;
/// Default share of each epoch's Alpha paid as a bonus (50 %).
pub const DEFAULT_BONUS_PPM: u128 = 500_000;
/// Bounds for the bonus share of subnets registered later (25 %–75 %).
pub const MIN_BONUS_PPM: u128 = 250_000;
pub const MAX_BONUS_PPM: u128 = 750_000;
/// Subnet split of the bonus, the same as for ICT: miners / validators / owner.
pub const MINER_PPM: u128 = 500_000;
pub const VALIDATOR_PPM: u128 = 380_000;
/// Owner's Alpha vests linearly over 180 days.
pub const OWNER_VESTING_BLOCKS: u64 = 180 * BLOCKS_PER_DAY;

// ---------------------------------------------------------------- issuance

/// Alpha issuance of one subnet: ICT's schedule from the subnet's first block.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Issuance {
    pub state: emission::EmissionState,
}

/// One epoch's new Alpha, already split.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct EpochAlpha {
    pub issued: u128,
    pub bonus: u128,
    pub stock: u128,
}

/// Bonus split between miners, validators and owner (owner gets the remainder).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BonusSplit {
    pub miners: u128,
    pub validators: u128,
    pub owner: u128,
}

impl Issuance {
    /// Issue `blocks` blocks of Alpha and split them.
    pub fn issue(&mut self, blocks: u64, bonus_ppm: u128) -> EpochAlpha {
        let mut issued = 0u128;
        for _ in 0..blocks {
            let r = emission::issue_block(&mut self.state);
            if r == 0 {
                break;
            }
            issued += r;
        }
        let bonus = mul_div(issued, bonus_ppm.min(PPM), PPM);
        EpochAlpha {
            issued,
            bonus,
            stock: issued - bonus,
        }
    }

    pub fn total(&self) -> u128 {
        self.state.total_issued
    }
}

pub fn split_bonus(bonus: u128) -> BonusSplit {
    let miners = mul_div(bonus, MINER_PPM, PPM);
    let validators = mul_div(bonus, VALIDATOR_PPM, PPM);
    BonusSplit {
        miners,
        validators,
        owner: bonus - miners - validators,
    }
}

/// Owner's vested amount of `total` granted at `granted_at`, at block `now`.
pub fn vested(total: u128, granted_at: u64, now: u64) -> u128 {
    let elapsed = now.saturating_sub(granted_at);
    if elapsed >= OWNER_VESTING_BLOCKS {
        total
    } else {
        mul_div(total, elapsed as u128, OWNER_VESTING_BLOCKS as u128)
    }
}

// ---------------------------------------------------------------- pool

/// Depth of the start curve: ten days of stock at the first reward (36,000 Alpha).
/// Revised after simulation (sim/RESULTS.md): a million virtual Alpha made the
/// price move far too slowly, so buyers could hardly buy at all.
pub const START_DEPTH: u128 = 36_000 * UNIT;
/// Start price = sale price / START_DIVISOR (decided 25 Nov 2026).
pub const START_DIVISOR: u128 = 1_000_000;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AlphaPool {
    /// Virtual ICT. Shapes the curve only; never paid out.
    pub vx: u128,
    /// Virtual Alpha. Shapes the curve only; never paid out.
    pub vy: u128,
    /// Real ICT in the pool.
    pub rx: u128,
    /// Real Alpha in the pool (the stock plus everything sold back).
    pub ry: u128,
    /// False once the subnet is removed: no more stock arrives.
    pub open: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Trade {
    /// Input actually used (less than offered on a partial fill).
    pub amount_in: u128,
    /// Asset received by the trader.
    pub amount_out: u128,
    /// Part of the input fee that is burned (input asset).
    pub burned: u128,
    /// Part of the input fee left in the pool (input asset).
    pub kept: u128,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PoolError {
    ZeroInput,
    /// Nothing real on the other side to trade against.
    Insufficient,
    SlippageExceeded,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Fees {
    pub fee_ppm: u128,
    pub burn_ppm: u128,
}

/// 0.5 % per trade, half burned (proposed).
pub const DEFAULT_FEES: Fees = Fees {
    fee_ppm: 5_000,
    burn_ppm: 500_000,
};

/// A partial fill leaves this share of the real reserve in the pool, so the
/// price stays finite and the next trader can still trade.
const FILL_MAX_PPM: u128 = 990_000;

impl AlphaPool {
    /// A new pool for a slot sold for `sale_price` ICT:
    /// start price = sale price / 1,000,000, depth = START_DEPTH.
    pub fn open(sale_price: u128) -> Self {
        Self {
            vx: mul_div(sale_price, START_DEPTH, START_DIVISOR * UNIT).max(1),
            vy: START_DEPTH,
            rx: 0,
            ry: 0,
            open: true,
        }
    }

    pub fn x(&self) -> u128 {
        self.vx + self.rx
    }
    pub fn y(&self) -> u128 {
        self.vy + self.ry
    }

    /// Price in ICT base units per whole Alpha.
    pub fn price(&self) -> u128 {
        mul_div(self.x(), UNIT, self.y())
    }

    /// Newly issued stock goes straight into the pool.
    pub fn add_stock(&mut self, amount: u128) {
        if self.open {
            self.ry += amount;
        }
    }

    /// One side of a swap. `x` is the input side, `y` the output side;
    /// `real_out` the real part of the output side. Fills partially when the
    /// real output would run out.
    fn swap(
        x: u128,
        y: u128,
        real_out: u128,
        amount_in: u128,
        fees: Fees,
    ) -> Result<(Trade, u128), PoolError> {
        if amount_in == 0 {
            return Err(PoolError::ZeroInput);
        }
        if real_out == 0 {
            return Err(PoolError::Insufficient);
        }
        let mut amount_in = amount_in;
        let mut fee = mul_div(amount_in, fees.fee_ppm, PPM);
        let mut net = amount_in - fee;
        // out = y - x*y/(x+net), the remaining reserve rounded up
        let mut out = y.saturating_sub(mul_div(x, y, x + net) + 1);
        if out > real_out {
            // partial fill: take FILL_MAX of the real reserve, pay only for it
            out = mul_div(real_out, FILL_MAX_PPM, PPM);
            if out == 0 {
                return Err(PoolError::Insufficient);
            }
            // net needed so that y - x*y/(x+net) = out: net = x*out/(y-out), rounded up
            net = mul_div(x, out, y - out) + 1;
            amount_in = mul_div(net, PPM, PPM - fees.fee_ppm) + 1;
            fee = amount_in - net;
        }
        let burned = mul_div(fee, fees.burn_ppm, PPM);
        let kept = fee - burned;
        Ok((
            Trade {
                amount_in,
                amount_out: out,
                burned,
                kept,
            },
            net,
        ))
    }

    /// ICT in, Alpha out (staking).
    pub fn buy(&mut self, ict_in: u128, min_alpha: u128, fees: Fees) -> Result<Trade, PoolError> {
        let (t, net) = Self::swap(self.x(), self.y(), self.ry, ict_in, fees)?;
        if t.amount_out < min_alpha {
            return Err(PoolError::SlippageExceeded);
        }
        self.rx += net + t.kept;
        self.ry -= t.amount_out;
        Ok(t)
    }

    /// Alpha in, ICT out (unstaking). Pays only from real ICT.
    pub fn sell(&mut self, alpha_in: u128, min_ict: u128, fees: Fees) -> Result<Trade, PoolError> {
        let (t, net) = Self::swap(self.y(), self.x(), self.rx, alpha_in, fees)?;
        if t.amount_out < min_ict {
            return Err(PoolError::SlippageExceeded);
        }
        self.ry += net + t.kept;
        self.rx -= t.amount_out;
        Ok(t)
    }

    pub fn quote_buy(&self, ict_in: u128, fees: Fees) -> Option<Trade> {
        let mut c = *self;
        c.buy(ict_in, 0, fees).ok()
    }

    pub fn quote_sell(&self, alpha_in: u128, fees: Fees) -> Option<Trade> {
        let mut c = *self;
        c.sell(alpha_in, 0, fees).ok()
    }

    /// The subnet is removed: no more stock; trading against the real reserves goes on.
    pub fn close(&mut self) {
        self.open = false;
    }

    /// ICT that counts as the subnet's stake: real ICT only.
    pub fn stake_ict(&self) -> u128 {
        self.rx
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NO_FEES: Fees = Fees {
        fee_ppm: 0,
        burn_ppm: 0,
    };

    fn ict(n: u128) -> u128 {
        n * UNIT
    }

    #[test]
    fn issuance_is_icts_schedule_from_day_one() {
        let mut i = Issuance::default();
        let e = i.issue(BLOCKS_PER_DAY, DEFAULT_BONUS_PPM);
        assert_eq!(e.issued, ict(7_200));
        assert_eq!(e.bonus, ict(3_600));
        assert_eq!(e.stock, ict(3_600));
    }

    #[test]
    fn issuance_halves_like_ict() {
        let mut i = Issuance {
            state: emission::EmissionState {
                total_issued: emission::next_threshold(0) - ict(10),
                halvings: 0,
            },
        };
        let e = i.issue(20, DEFAULT_BONUS_PPM);
        assert_eq!(e.issued, ict(10) + ict(5));
        assert_eq!(i.state.halvings, 1);
    }

    #[test]
    fn bonus_split_is_50_38_12() {
        let s = split_bonus(ict(3_600));
        assert_eq!(s.miners, ict(1_800));
        assert_eq!(s.validators, ict(1_368));
        assert_eq!(s.owner, ict(432));
    }

    #[test]
    fn owner_vests_linearly_over_180_days() {
        assert_eq!(vested(ict(180), 0, 0), 0);
        assert_eq!(vested(ict(180), 0, 90 * BLOCKS_PER_DAY), ict(90));
        assert_eq!(vested(ict(180), 0, 200 * BLOCKS_PER_DAY), ict(180));
    }

    #[test]
    fn start_price_is_sale_price_over_a_million() {
        assert_eq!(AlphaPool::open(ict(1_000)).price(), UNIT / 1_000);
        assert_eq!(AlphaPool::open(ict(50_000)).price(), 50 * UNIT / 1_000);
        assert_eq!(AlphaPool::open(ict(200)).price(), UNIT / 5_000);
    }

    #[test]
    fn empty_pool_cannot_pay_a_seller_or_sell_to_a_buyer() {
        let mut p = AlphaPool::open(ict(1_000));
        assert_eq!(p.sell(ict(10), 0, NO_FEES), Err(PoolError::Insufficient));
        assert_eq!(p.buy(ict(1), 0, NO_FEES), Err(PoolError::Insufficient));
    }

    #[test]
    fn stock_lowers_the_price_and_buying_raises_it() {
        let mut p = AlphaPool::open(ict(1_000));
        let p0 = p.price();
        p.add_stock(ict(3_600));
        let p1 = p.price();
        assert!(p1 < p0);
        p.buy(ict(1), 0, NO_FEES).unwrap();
        assert!(p.price() > p1);
    }

    #[test]
    fn a_large_buy_fills_partially_and_pays_only_for_what_it_gets() {
        let mut p = AlphaPool::open(ict(1_000));
        p.add_stock(ict(3_600));
        let t = p.buy(ict(1_000), 0, NO_FEES).unwrap();
        assert!(t.amount_out <= ict(3_600) * 99 / 100);
        assert!(t.amount_in < ict(1_000));
        assert!(p.ry >= ict(36) - 1, "1 % of the real Alpha stays");
        assert_eq!(p.rx, t.amount_in);
    }

    #[test]
    fn a_seller_is_paid_only_from_real_ict_and_never_more_than_bought_for() {
        let mut p = AlphaPool::open(ict(1_000));
        p.add_stock(ict(3_600));
        let t = p.buy(ict(1), 0, NO_FEES).unwrap();
        let back = p.sell(t.amount_out, 0, NO_FEES).unwrap();
        assert!(back.amount_out <= t.amount_in);
        assert!(back.amount_out + 10 >= t.amount_in);
    }

    #[test]
    fn a_large_sell_fills_partially_from_real_ict() {
        let mut p = AlphaPool::open(ict(1_000));
        p.add_stock(ict(3_600));
        p.buy(ict(2), 0, NO_FEES).unwrap();
        let rx = p.rx;
        let t = p.sell(ict(1_000_000), 0, NO_FEES).unwrap();
        assert!(t.amount_out <= rx * 99 / 100 + 1);
        assert!(t.amount_in < ict(1_000_000));
    }

    #[test]
    fn fees_burn_half_and_keep_half() {
        let mut p = AlphaPool::open(ict(1_000));
        p.add_stock(ict(3_600));
        let t = p.buy(ict(2), 0, DEFAULT_FEES).unwrap();
        assert_eq!(t.burned, ict(2) * 5 / 2_000);
        assert_eq!(t.kept, t.burned);
        assert_eq!(p.rx, ict(2) - t.burned);
    }

    #[test]
    fn slippage_limit_is_enforced_and_a_failed_trade_changes_nothing() {
        let mut p = AlphaPool::open(ict(1_000));
        p.add_stock(ict(3_600));
        let q = p.quote_buy(ict(1), NO_FEES).unwrap().amount_out;
        let before = p;
        assert_eq!(p.buy(ict(1), q + 1, NO_FEES), Err(PoolError::SlippageExceeded));
        assert_eq!(p, before);
        assert_eq!(p.buy(ict(1), q, NO_FEES).unwrap().amount_out, q);
    }

    #[test]
    fn a_closed_pool_gets_no_stock_but_lets_holders_out() {
        let mut p = AlphaPool::open(ict(1_000));
        p.add_stock(ict(3_600));
        let a = p.buy(ict(2), 0, NO_FEES).unwrap().amount_out;
        p.close();
        let ry = p.ry;
        p.add_stock(ict(3_600));
        assert_eq!(p.ry, ry);
        assert!(p.sell(a / 2, 0, NO_FEES).is_ok());
    }

    /// Deterministic random walk over issuance, buys and sells: virtual reserves
    /// are never paid out, k never falls in a swap, and Alpha is conserved.
    #[test]
    fn random_walk_keeps_the_invariants() {
        let mut seed = 0x2545_F491_4F6C_DD1Du64;
        let mut rnd = |m: u64| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed % m
        };
        let mut p = AlphaPool::open(ict(1_000));
        let mut iss = Issuance::default();
        let mut held = 0u128;
        let mut burned_alpha = 0u128;
        for step in 0..20_000u64 {
            if step % 10 == 0 {
                let e = iss.issue(360, DEFAULT_BONUS_PPM);
                p.add_stock(e.stock);
                held += e.bonus;
            }
            let k = p.x() * p.y() / UNIT;
            match rnd(3) {
                0 => {
                    let amt = (rnd(5_000) as u128 + 1) * UNIT / 10;
                    if let Ok(t) = p.buy(amt, 0, DEFAULT_FEES) {
                        held += t.amount_out;
                        assert!(t.amount_in <= amt);
                        assert!(p.x() * p.y() / UNIT >= k);
                    }
                }
                _ => {
                    let amt = held * (rnd(100) as u128 + 1) / 400;
                    if amt > 0 {
                        if let Ok(t) = p.sell(amt, 0, DEFAULT_FEES) {
                            held -= t.amount_in;
                            burned_alpha += t.burned;
                            assert!(p.x() * p.y() / UNIT >= k);
                        }
                    }
                }
            }
            assert!(p.x() >= p.vx && p.y() >= p.vy);
        }
        assert_eq!(iss.total(), held + p.ry + burned_alpha);
    }
}
