//! Alpha tokens and subnet pools (spec Section 7, decided 25 and 26 Nov 2026).
//!
//! - Issuance per subnet is exactly ICT's: the same block reward and halving
//!   thresholds, counted from the subnet's own first block (`emission`).
//! - Each epoch's issuance is split: `bonus_ppm` is paid to the subnet's
//!   miners, validators and owner; the rest is stock, which waits outside the pool.
//! - The pool is an ordinary constant-product pool with real reserves only:
//!   x ICT and y Alpha, price x / y. Nothing enters it that has not been issued.
//! - It opens 10 days after the subnet, at the start price of the valuation
//!   formula (`start_price`), with ICT = min(auction ICT held, 36,000 Alpha at
//!   that price, waiting stock at that price) and Alpha = ICT / price.
//! - Afterwards stock is released block by block (`release`), held back while the
//!   price is more than 20 % below its 30-day average.
//! - Every trade carries a price limit; there are no partial fills.
//! - Alpha holders are paid in ICT (`STAKER_SHARE_PPM`, `MAX_TAKE_PPM`); nothing
//!   here buys Alpha back.
//!
//! All amounts in base units (1 Alpha = 1 ICT = 10^9). Every division rounds in
//! the pool's favour.

use crate::{emission, mul_div, BLOCKS_PER_DAY, PPM, UNIT};

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

/// The pool stays closed for 10 days after the subnet opens.
pub const OPEN_AFTER_BLOCKS: u64 = 10 * BLOCKS_PER_DAY;
/// At most ten days of stock (36,000 Alpha) go in when the pool opens.
pub const OPEN_DEPTH: u128 = 36_000 * UNIT;
/// Stock is held back while the price is more than 20 % below its 30-day average.
pub const HOLD_BACK_PPM: u128 = 200_000;
/// Days in the price average.
pub const AVG_DAYS: u32 = 30;
/// Required return in the valuation formula (20 % a year; Parameters track).
pub const DEFAULT_REQUIRED_RETURN_PPM: u128 = 200_000;
/// Highest validator take on its stakers' dividends (18 %).
pub const MAX_TAKE_PPM: u128 = 180_000;
/// Share of a subnet's ICT that reaches Alpha holders in the valuation formula:
/// the validators' 38 % less the highest take, 38 % x 82 % = 31.16 %.
pub const STAKER_SHARE_PPM: u128 = VALIDATOR_PPM * (PPM - MAX_TAKE_PPM) / PPM;

/// Trading fee and the part of it that is burned, both in parts per million.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Fees {
    pub fee_ppm: u128,
    pub burn_ppm: u128,
}

/// 0.5 % per trade, half burned.
pub const DEFAULT_FEES: Fees = Fees {
    fee_ppm: 5_000,
    burn_ppm: 500_000,
};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Trade {
    /// Input taken from the trader (including the fee).
    pub amount_in: u128,
    /// Output paid to the trader.
    pub amount_out: u128,
    /// Part of the fee that is burned (input asset).
    pub burned: u128,
    /// Part of the fee left in the pool (input asset).
    pub kept: u128,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PoolError {
    ZeroInput,
    /// The pool is not open for trading (closed period, or nothing in it).
    NotTrading,
    /// The price moved past the trader's limit.
    SlippageExceeded,
}

/// Alpha issued to a subnet after `days` days (fully diluted), on ICT's schedule.
pub fn alpha_issued_after(days: u64) -> u128 {
    let per_day = BLOCKS_PER_DAY as u128 * emission::INITIAL_REWARD;
    let period = emission::MAX_SUPPLY / 2 / per_day; // 1,458 days at 7,200 a day
    let mut left = days as u128;
    let mut total = 0u128;
    let mut k = 0u32;
    while left > 0 && k < 64 {
        let d = left.min(period);
        total += d * (per_day >> k);
        left -= d;
        k += 1;
    }
    total.min(emission::MAX_SUPPLY)
}

/// Start price in ICT base units per whole Alpha, by the valuation formula (spec
/// Section 7): the present value of the ICT an Alpha is expected to pay, from the day
/// the pool opens over 20 years in monthly steps, each month's ICT divided over all
/// Alpha issued by the middle of that month (fully diluted).
///
/// - `subnet_ict_per_day`: ICT a day the average subnet other than SN0 receives now
///   (the pot for those subnets divided by their number, this one included);
/// - `days_to_ict_halving`: days until ICT's next halving;
/// - `required_return_ppm`: discount rate a year (200,000 = 20 %), compounded monthly.
pub fn start_price(subnet_ict_per_day: u128, days_to_ict_halving: u64, required_return_ppm: u128) -> u128 {
    const SCALE: u128 = 1_000_000_000_000; // 1e12 fixed point for the discount factor
    let period = (emission::MAX_SUPPLY / 2 / (BLOCKS_PER_DAY as u128 * emission::INITIAL_REWARD)) as u64;
    let open_day = OPEN_AFTER_BLOCKS / BLOCKS_PER_DAY;
    let ict_month = mul_div(subnet_ict_per_day * 30, STAKER_SHARE_PPM, PPM);
    let mut disc = SCALE;
    let mut value = 0u128;
    for m in 0..240u64 {
        let mid = open_day + m * 30 + 15;
        let since_open = m * 30 + 15;
        let halvings = if since_open < days_to_ict_halving { 0 } else { 1 + (since_open - days_to_ict_halving) / period };
        let supply = alpha_issued_after(mid).max(1);
        let per_alpha = mul_div(ict_month >> halvings.min(63), UNIT, supply);
        value += mul_div(per_alpha, disc, SCALE);
        disc = mul_div(disc, 12 * PPM, 12 * PPM + required_return_ppm);
    }
    value
}

/// One subnet's pool: real reserves only.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AlphaPool {
    /// ICT in the pool.
    pub x: u128,
    /// Alpha in the pool.
    pub y: u128,
    /// Issued stock waiting to be released into the pool.
    pub waiting: u128,
    /// Auction ICT held for the pool until it opens.
    pub reserved_ict: u128,
    /// Open for trading.
    pub trading: bool,
    /// The subnet was removed: no more stock.
    pub removed: bool,
}

/// What happened when a pool opened.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Opening {
    pub ict: u128,
    pub alpha: u128,
    /// Auction ICT the pool could not match: goes to the treasury.
    pub to_treasury: u128,
}

impl AlphaPool {
    /// A pool for a new subnet: closed, with the auction ICT held for it.
    pub fn new(reserved_ict: u128) -> Self {
        AlphaPool { reserved_ict, ..Default::default() }
    }

    /// Price in ICT base units per whole Alpha (0 while closed).
    pub fn price(&self) -> u128 {
        if !self.trading || self.y == 0 {
            return 0;
        }
        mul_div(self.x, UNIT, self.y)
    }

    /// ICT that counts as the subnet's stake: the pool's ICT, or the held auction
    /// ICT while closed.
    pub fn stake_ict(&self) -> u128 {
        if self.trading { self.x } else { self.reserved_ict }
    }

    /// New stock: always waits first.
    pub fn add_stock(&mut self, amount: u128) {
        if !self.removed {
            self.waiting += amount;
        }
    }

    /// Open at `p0` (ICT base units per whole Alpha). Returns none, and stays closed,
    /// if it cannot put both ICT and Alpha in.
    pub fn open(&mut self, p0: u128) -> Option<Opening> {
        if self.trading || self.removed || p0 == 0 {
            return None;
        }
        let alpha_cap = self.waiting.min(OPEN_DEPTH);
        let ict = self.reserved_ict.min(mul_div(alpha_cap, p0, UNIT));
        let alpha = mul_div(ict, UNIT, p0).min(alpha_cap);
        if ict == 0 || alpha == 0 {
            return None;
        }
        let o = Opening { ict, alpha, to_treasury: self.reserved_ict - ict };
        self.x = ict;
        self.y = alpha;
        self.waiting -= alpha;
        self.reserved_ict = 0;
        self.trading = true;
        Some(o)
    }

    /// Release stock for one block: up to twice `per_block` (the block's share plus as
    /// much again of what was held back), nothing while the price is more than 20 %
    /// below `avg_price`. Returns the Alpha released.
    pub fn release(&mut self, per_block: u128, avg_price: Option<u128>) -> u128 {
        if !self.trading || self.removed || self.waiting == 0 {
            return 0;
        }
        if let Some(avg) = avg_price {
            if avg > 0 && self.price() < mul_div(avg, PPM - HOLD_BACK_PPM, PPM) {
                return 0;
            }
        }
        let r = self.waiting.min(per_block.saturating_mul(2));
        self.waiting -= r;
        self.y += r;
        r
    }

    /// x * y = k swap. `input`/`output` are the reserves on each side. Output is
    /// output * net / (input + net), rounded down (in the pool's favour).
    fn swap(input: u128, output: u128, amount_in: u128, fees: Fees) -> Trade {
        let fee = mul_div(amount_in, fees.fee_ppm, PPM).max(if fees.fee_ppm > 0 { 1 } else { 0 });
        let burned = mul_div(fee, fees.burn_ppm, PPM);
        let net = amount_in.saturating_sub(fee);
        let out = mul_div(output, net, input + net);
        Trade { amount_in, amount_out: out, burned, kept: fee - burned }
    }

    fn check(&self, amount_in: u128) -> Result<(), PoolError> {
        if amount_in == 0 {
            return Err(PoolError::ZeroInput);
        }
        if !self.trading || self.x == 0 || self.y == 0 {
            return Err(PoolError::NotTrading);
        }
        Ok(())
    }

    /// ICT in, Alpha out (staking). Fails if fewer than `min_alpha` would come out.
    pub fn buy(&mut self, ict_in: u128, min_alpha: u128, fees: Fees) -> Result<Trade, PoolError> {
        self.check(ict_in)?;
        let t = Self::swap(self.x, self.y, ict_in, fees);
        if t.amount_out == 0 || t.amount_out < min_alpha || t.amount_out >= self.y {
            return Err(PoolError::SlippageExceeded);
        }
        self.x += ict_in - t.burned;
        self.y -= t.amount_out;
        Ok(t)
    }

    /// Alpha in, ICT out (unstaking). Fails if less than `min_ict` would come out.
    pub fn sell(&mut self, alpha_in: u128, min_ict: u128, fees: Fees) -> Result<Trade, PoolError> {
        self.check(alpha_in)?;
        let t = Self::swap(self.y, self.x, alpha_in, fees);
        if t.amount_out == 0 || t.amount_out < min_ict || t.amount_out >= self.x {
            return Err(PoolError::SlippageExceeded);
        }
        self.y += alpha_in - t.burned;
        self.x -= t.amount_out;
        Ok(t)
    }

    pub fn quote_buy(&self, ict_in: u128, fees: Fees) -> Option<Trade> {
        let mut p = *self;
        p.buy(ict_in, 0, fees).ok()
    }

    pub fn quote_sell(&self, alpha_in: u128, fees: Fees) -> Option<Trade> {
        let mut p = *self;
        p.sell(alpha_in, 0, fees).ok()
    }

    /// The subnet is removed: no more stock; waiting stock is burned (returned);
    /// trading goes on against the reserves.
    pub fn remove(&mut self) -> u128 {
        self.removed = true;
        let w = self.waiting;
        self.waiting = 0;
        w
    }
}

#[cfg(test)]
mod tests {
    use super::*;


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

    fn open_pool(reserved: u128, waiting: u128, p0: u128) -> AlphaPool {
        let mut p = AlphaPool::new(reserved);
        p.add_stock(waiting);
        p.open(p0).unwrap();
        p
    }

    #[test]
    fn fully_diluted_supply_follows_the_schedule() {
        assert_eq!(alpha_issued_after(0), 0);
        assert_eq!(alpha_issued_after(10), ict(72_000));
        assert_eq!(alpha_issued_after(365), ict(2_628_000));
        assert!(alpha_issued_after(100_000) <= emission::MAX_SUPPLY);
    }

    #[test]
    fn start_price_matches_the_valuation_model() {
        // sim/alpha_decided.py: 10 subnets share 2,880 ICT a day, r 20 %: P0 about 0.056 ICT
        let p0 = start_price(ict(288), 1_280, DEFAULT_REQUIRED_RETURN_PPM);
        assert!(p0 > UNIT / 25 && p0 < UNIT * 8 / 100, "p0 = {p0}");
        // three times the subnets: about a third of the price
        let p30 = start_price(ict(96), 1_280, DEFAULT_REQUIRED_RETURN_PPM);
        assert!(p30 * 3 <= p0 + 1_000 && p30 * 3 + 1_000 >= p0);
        // a higher required return lowers it
        assert!(start_price(ict(288), 1_280, 500_000) < p0);
        assert_eq!(start_price(0, 1_280, DEFAULT_REQUIRED_RETURN_PPM), 0);
    }

    #[test]
    fn a_new_pool_is_closed_and_counts_the_held_ict_as_stake() {
        let mut p = AlphaPool::new(ict(500));
        p.add_stock(ict(36_000));
        assert!(!p.trading);
        assert_eq!(p.price(), 0);
        assert_eq!(p.stake_ict(), ict(500));
        assert_eq!(p.buy(ict(1), 0, DEFAULT_FEES), Err(PoolError::NotTrading));
        assert_eq!(p.release(ict(1), None), 0);
    }

    #[test]
    fn opening_takes_the_smaller_of_the_held_ict_and_36000_alpha_at_the_price() {
        let p0 = UNIT * 4 / 100; // 0.04 ICT
        // sold for 1,000: 500 ICT and 12,500 Alpha; nothing to the treasury
        let mut p = AlphaPool::new(ict(500));
        p.add_stock(ict(36_000));
        assert_eq!(p.open(p0), Some(Opening { ict: ict(500), alpha: ict(12_500), to_treasury: 0 }));
        assert_eq!((p.x, p.y, p.waiting, p.price()), (ict(500), ict(12_500), ict(23_500), p0));
        assert_eq!(p.stake_ict(), ict(500));
        // sold for 10,000: capped at 36,000 x 0.04 = 1,440 ICT; 3,560 to the treasury
        let mut q = AlphaPool::new(ict(5_000));
        q.add_stock(ict(36_000));
        assert_eq!(q.open(p0), Some(Opening { ict: ict(1_440), alpha: ict(36_000), to_treasury: ict(3_560) }));
        assert_eq!(q.waiting, 0);
        // opens once
        assert_eq!(q.open(p0), None);
    }

    #[test]
    fn opening_never_puts_in_alpha_that_has_not_been_issued() {
        let mut p = AlphaPool::new(ict(5_000));
        p.add_stock(ict(1_000));
        let o = p.open(UNIT).unwrap();
        assert_eq!((o.alpha, o.ict, o.to_treasury), (ict(1_000), ict(1_000), ict(4_000)));
        let mut empty = AlphaPool::new(ict(5_000));
        assert_eq!(empty.open(UNIT), None);
        assert!(!empty.trading);
    }

    #[test]
    fn stock_is_released_block_by_block_and_held_back_after_a_fall() {
        let mut p = open_pool(ict(500), ict(36_000), UNIT * 4 / 100);
        let per_block = UNIT / 2;
        assert_eq!(p.release(per_block, Some(p.price())), UNIT);
        // price 25 % under its average: held back
        let avg = p.price() * 4 / 3;
        let w = p.waiting;
        assert_eq!(p.release(per_block, Some(avg)), 0);
        assert_eq!(p.waiting, w);
        // within 20 %: released again, at most twice a block's share
        assert_eq!(p.release(per_block, Some(p.price() * 11 / 10)), UNIT);
    }

    #[test]
    fn buying_raises_the_price_and_selling_lowers_it() {
        let mut p = open_pool(ict(1_000), ict(36_000), UNIT * 4 / 100);
        let p0 = p.price();
        let t = p.buy(ict(100), 0, DEFAULT_FEES).unwrap();
        assert!(p.price() > p0);
        assert!(t.amount_out > 0);
        let p1 = p.price();
        p.sell(t.amount_out, 0, DEFAULT_FEES).unwrap();
        assert!(p.price() < p1);
        // a round trip loses the fees
        assert!(p.x > ict(1_000) - ict(1));
    }

    #[test]
    fn a_trade_past_its_price_limit_fails_and_changes_nothing() {
        let mut p = open_pool(ict(1_000), ict(36_000), UNIT * 4 / 100);
        let before = p;
        let q = p.quote_buy(ict(100), DEFAULT_FEES).unwrap();
        assert_eq!(p.buy(ict(100), q.amount_out + 1, DEFAULT_FEES), Err(PoolError::SlippageExceeded));
        assert_eq!(p, before);
        assert_eq!(p.buy(ict(100), q.amount_out, DEFAULT_FEES).unwrap(), q);
    }

    #[test]
    fn a_huge_trade_never_empties_a_side() {
        let mut p = open_pool(ict(1_000), ict(36_000), UNIT * 4 / 100);
        let t = p.buy(ict(10_000_000), 0, DEFAULT_FEES).unwrap();
        assert!(p.y > 0 && t.amount_out < ict(36_000));
        let s = p.sell(ict(10_000_000), 0, DEFAULT_FEES).unwrap();
        assert!(p.x > 0 && s.amount_out > 0);
    }

    #[test]
    fn fees_burn_half_and_keep_half() {
        let mut p = open_pool(ict(1_000), ict(36_000), UNIT * 4 / 100);
        let t = p.buy(ict(100), 0, DEFAULT_FEES).unwrap();
        assert_eq!(t.burned + t.kept, ict(100) / 200);
        assert_eq!(t.burned, t.kept);
        assert_eq!(p.x, ict(1_100) - t.burned);
    }

    #[test]
    fn a_removed_pool_burns_waiting_stock_and_lets_holders_out() {
        let mut p = open_pool(ict(500), ict(36_000), UNIT * 4 / 100);
        assert_eq!(p.remove(), ict(23_500));
        p.add_stock(ict(100));
        assert_eq!(p.waiting, 0);
        assert!(p.sell(ict(10), 0, DEFAULT_FEES).is_ok());
    }

    #[test]
    fn random_walk_keeps_the_invariants() {
        // k never falls in a swap; Alpha is conserved; nothing is paid from an empty side
        let mut p = open_pool(ict(800), ict(36_000), UNIT * 5 / 100);
        let mut held = 0u128;
        let mut seed = 7u64;
        let mut rnd = || {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            seed >> 33
        };
        let mut burned_alpha = 0u128;
        let mut stock_in = ict(36_000);
        for i in 0..5_000 {
            let k0 = p.x * p.y;
            match rnd() % 3 {
                0 => {
                    if let Ok(t) = p.buy(ict(rnd() as u128 % 500 + 1), 0, DEFAULT_FEES) {
                        held += t.amount_out;
                        assert!(p.x * p.y >= k0, "k fell in a buy at step {i}");
                    }
                }
                1 if held > 0 => {
                    let a = (rnd() as u128 % held).max(1);
                    if let Ok(t) = p.sell(a, 0, DEFAULT_FEES) {
                        held -= a;
                        burned_alpha += t.burned;
                        assert!(p.x * p.y >= k0, "k fell in a sale at step {i}");
                    }
                }
                _ => {
                    p.add_stock(UNIT / 2);
                    stock_in += UNIT / 2;
                    p.release(UNIT / 2, None);
                }
            }
            assert!(p.x > 0 && p.y > 0);
            assert_eq!(p.y + p.waiting + held + burned_alpha, stock_in, "Alpha conserved at step {i}");
        }
    }
}
