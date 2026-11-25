//! Alpha tokens and subnet pools (spec Section 7, decided 25 Nov 2026).
//!
//! - Issuance per subnet is exactly ICT's: the same block reward and halving
//!   thresholds, counted from the subnet's own first block (`emission`).
//! - Each epoch's issuance is split: `bonus_ppm` is paid to the subnet's
//!   miners, validators and owner; the rest goes to the pool's stock.
//! - The pool is a constant-product curve over X = vx + rx ICT and Y = vy + ry
//!   Alpha. vx and vy are virtual: they only shape the price curve, set from the
//!   slot's sale price (p0 = sale price / 1,000,000 Alpha). rx and ry are real
//!   and are the only amounts that can ever be paid out.
//! - Staking is buying: before a purchase, stock is moved onto the curve at the
//!   current price (at most `daily_cap` per day), then the swap runs.
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

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AlphaPool {
    /// Virtual ICT (the slot's sale price). Never paid out.
    pub vx: u128,
    /// Virtual Alpha (START_ALPHA). Never paid out.
    pub vy: u128,
    /// Real ICT in the pool.
    pub rx: u128,
    /// Real Alpha on the curve.
    pub ry: u128,
    /// Unsold Alpha in stock, outside the curve.
    pub stock: u128,
    /// Stock sold in the current day window.
    pub sold_today: u128,
    /// First block of the current day window.
    pub day_start: u64,
    /// False once the subnet is removed: no stock is sold any more.
    pub open: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Trade {
    /// Asset received by the trader.
    pub amount_out: u128,
    /// Part of the input fee that is burned (input asset).
    pub burned: u128,
    /// Part of the input fee left in the pool (input asset).
    pub kept: u128,
    /// Alpha moved from stock onto the curve by this trade.
    pub from_stock: u128,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PoolError {
    ZeroInput,
    /// Not enough real reserve on the other side.
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

impl AlphaPool {
    /// A new pool for a slot sold for `sale_price` ICT, opened at `block`.
    pub fn open(sale_price: u128, block: u64) -> Self {
        Self {
            vx: sale_price.max(1),
            vy: START_ALPHA,
            rx: 0,
            ry: 0,
            stock: 0,
            sold_today: 0,
            day_start: block,
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

    /// Stock sale room left today, rolling the day window forward if needed.
    fn room(&mut self, block: u64, daily_cap: u128) -> u128 {
        if block >= self.day_start + BLOCKS_PER_DAY {
            let days = (block - self.day_start) / BLOCKS_PER_DAY;
            self.day_start += days * BLOCKS_PER_DAY;
            self.sold_today = 0;
        }
        if !self.open {
            return 0;
        }
        daily_cap.saturating_sub(self.sold_today).min(self.stock)
    }

    /// Add newly issued stock.
    pub fn add_stock(&mut self, amount: u128) {
        self.stock += amount;
    }

    /// ICT in, Alpha out (staking). Stock is sold first at the current price,
    /// then the rest trades on the curve against real Alpha only.
    pub fn buy(
        &mut self,
        ict_in: u128,
        min_alpha: u128,
        fees: Fees,
        daily_cap: u128,
        block: u64,
    ) -> Result<Trade, PoolError> {
        if ict_in == 0 {
            return Err(PoolError::ZeroInput);
        }
        let mut next = *self;
        let fee = mul_div(ict_in, fees.fee_ppm, PPM);
        let burned = mul_div(fee, fees.burn_ppm, PPM);
        let kept = fee - burned;
        let net = ict_in - fee;
        // Inject stock at the current price: a = net * Y / X, capped.
        let room = next.room(block, daily_cap);
        let at_price = mul_div(net, next.y(), next.x());
        let a = at_price.min(room);
        next.ry += a;
        next.stock -= a;
        next.sold_today += a;
        // Swap net ICT on the curve, rounding the remaining reserve up.
        let x0 = next.x();
        let y0 = next.y();
        let new_x = x0 + net;
        let y_after = mul_div(x0, y0, new_x) + 1;
        let out = y0.saturating_sub(y_after);
        if out > next.ry {
            return Err(PoolError::Insufficient);
        }
        if out < min_alpha {
            return Err(PoolError::SlippageExceeded);
        }
        next.rx += net + kept;
        next.ry -= out;
        *self = next;
        Ok(Trade {
            amount_out: out,
            burned,
            kept,
            from_stock: a,
        })
    }

    /// Alpha in, ICT out (unstaking). Pays only from real ICT.
    pub fn sell(
        &mut self,
        alpha_in: u128,
        min_ict: u128,
        fees: Fees,
        block: u64,
    ) -> Result<Trade, PoolError> {
        if alpha_in == 0 {
            return Err(PoolError::ZeroInput);
        }
        let mut next = *self;
        let _ = next.room(block, 0); // keep the day window current
        let fee = mul_div(alpha_in, fees.fee_ppm, PPM);
        let burned = mul_div(fee, fees.burn_ppm, PPM);
        let kept = fee - burned;
        let net = alpha_in - fee;
        let x0 = next.x();
        let y0 = next.y();
        let new_y = y0 + net;
        let x_after = mul_div(x0, y0, new_y) + 1;
        let out = x0.saturating_sub(x_after);
        if out > next.rx {
            return Err(PoolError::Insufficient);
        }
        if out < min_ict {
            return Err(PoolError::SlippageExceeded);
        }
        next.ry += net + kept;
        next.rx -= out;
        *self = next;
        Ok(Trade {
            amount_out: out,
            burned,
            kept,
            from_stock: 0,
        })
    }

    pub fn quote_buy(&self, ict_in: u128, fees: Fees, daily_cap: u128, block: u64) -> Option<u128> {
        let mut c = *self;
        c.buy(ict_in, 0, fees, daily_cap, block)
            .ok()
            .map(|t| t.amount_out)
    }

    pub fn quote_sell(&self, alpha_in: u128, fees: Fees, block: u64) -> Option<u128> {
        let mut c = *self;
        c.sell(alpha_in, 0, fees, block)
            .ok()
            .map(|t| t.amount_out)
    }

    /// The subnet is removed: no more stock is sold; selling keeps working.
    pub fn close(&mut self) {
        self.open = false;
    }

    /// ICT that counts as the subnet's stake: real ICT only.
    pub fn stake_ict(&self) -> u128 {
        self.rx
    }
}

/// One day's Alpha issuance at the current reward: the stock sale cap.
pub fn daily_cap(issuance: &Issuance) -> u128 {
    emission::block_reward(issuance.state.halvings) * BLOCKS_PER_DAY as u128
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
        assert_eq!(daily_cap(&i), ict(7_200));
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
        // 10 blocks at 1 Alpha, then 10 at 0.5
        assert_eq!(e.issued, ict(10) + ict(5));
        assert_eq!(i.state.halvings, 1);
        assert_eq!(daily_cap(&i), ict(3_600));
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
        let p = AlphaPool::open(ict(1_000), 0);
        assert_eq!(p.price(), UNIT / 1_000); // 0.001 ICT
        let p = AlphaPool::open(ict(50_000), 0);
        assert_eq!(p.price(), 50 * UNIT / 1_000); // 0.05 ICT
    }

    #[test]
    fn empty_pool_cannot_pay_a_seller_or_a_buyer_without_stock() {
        let mut p = AlphaPool::open(ict(1_000), 0);
        assert_eq!(p.sell(ict(10), 0, NO_FEES, 1), Err(PoolError::Insufficient));
        assert_eq!(
            p.buy(ict(1), 0, NO_FEES, ict(7_200), 1),
            Err(PoolError::Insufficient)
        );
    }

    #[test]
    fn a_buyer_gets_stock_at_the_current_price() {
        let mut p = AlphaPool::open(ict(1_000), 0);
        p.add_stock(ict(3_600));
        let t = p.buy(ict(1), 0, NO_FEES, ict(7_200), 1).unwrap();
        // 1 ICT at 0.001 ICT/Alpha = 1,000 Alpha (minus one base unit of rounding)
        assert!(t.amount_out >= ict(1_000) - 2 && t.amount_out <= ict(1_000));
        assert_eq!(t.from_stock, ict(1_000));
        assert_eq!(p.rx, ict(1));
        assert!(p.price() > UNIT / 1_000);
    }

    #[test]
    fn price_doubles_when_real_ict_equals_the_sale_price() {
        // All Alpha bought from stock leaves Y at vy, so price = X / vy.
        let mut p = AlphaPool::open(ict(1_000), 0);
        p.add_stock(ict(10_000_000));
        let mut block = 1;
        while p.rx < ict(1_000) {
            p.buy(ict(10), 0, NO_FEES, u128::MAX, block).unwrap();
            block += 1;
        }
        let ratio = p.price() * 1_000 / (UNIT / 1_000);
        assert!((1_990..=2_010).contains(&ratio), "ratio {ratio}");
    }

    #[test]
    fn daily_cap_limits_stock_sales_then_the_curve_takes_over() {
        let mut p = AlphaPool::open(ict(1_000), 0);
        p.add_stock(ict(100_000));
        // 50 ICT at 0.001 would want 50,000 Alpha; only 7,200 may come from stock.
        let t = p.buy(ict(50), 0, NO_FEES, ict(7_200), 10);
        // the curve holds only the injected 7,200 real Alpha: the buy cannot fill
        assert_eq!(t, Err(PoolError::Insufficient));
        let t = p.buy(ict(5), 0, NO_FEES, ict(7_200), 10).unwrap();
        assert_eq!(t.from_stock, ict(5_000));
        let t = p.buy(ict(2), 0, NO_FEES, ict(7_200), 20).unwrap();
        assert!(t.from_stock <= ict(2_200));
        // the rest of today's room is too small for 5 ICT, and no one has sold
        assert_eq!(p.buy(ict(5), 0, NO_FEES, ict(7_200), 30), Err(PoolError::Insufficient));
        assert!(p.sold_today <= ict(7_200));
        // next day the window resets
        let t = p.buy(ict(1), 0, NO_FEES, ict(7_200), BLOCKS_PER_DAY + 5).unwrap();
        assert!(t.from_stock > 0);
        assert_eq!(p.sold_today, t.from_stock);
    }

    #[test]
    fn a_seller_is_paid_only_from_real_ict_and_never_more_than_bought_for() {
        let mut p = AlphaPool::open(ict(1_000), 0);
        p.add_stock(ict(3_600));
        let bought = p.buy(ict(2), 0, NO_FEES, ict(7_200), 1).unwrap().amount_out;
        let back = p.sell(bought, 0, NO_FEES, 2).unwrap().amount_out;
        assert!(back <= ict(2));
        assert!(back >= ict(2) - 10);
        assert!(p.rx < 20);
    }

    #[test]
    fn fees_burn_half_and_keep_half() {
        let mut p = AlphaPool::open(ict(1_000), 0);
        p.add_stock(ict(3_600));
        let t = p.buy(ict(2), 0, DEFAULT_FEES, ict(7_200), 1).unwrap();
        assert_eq!(t.burned, ict(2) * 5 / 2_000);
        assert_eq!(t.kept, t.burned);
        assert_eq!(p.rx, ict(2) - t.burned);
    }

    #[test]
    fn slippage_limit_is_enforced() {
        let mut p = AlphaPool::open(ict(1_000), 0);
        p.add_stock(ict(3_600));
        let q = p.quote_buy(ict(1), NO_FEES, ict(7_200), 1).unwrap();
        let before = p;
        assert_eq!(
            p.buy(ict(1), q + 1, NO_FEES, ict(7_200), 1),
            Err(PoolError::SlippageExceeded)
        );
        assert_eq!(p, before, "a failed trade changes nothing");
        assert_eq!(p.buy(ict(1), q, NO_FEES, ict(7_200), 1).unwrap().amount_out, q);
    }

    #[test]
    fn a_closed_pool_sells_no_stock_but_lets_holders_out() {
        let mut p = AlphaPool::open(ict(1_000), 0);
        p.add_stock(ict(3_600));
        let a = p.buy(ict(2), 0, NO_FEES, ict(7_200), 1).unwrap().amount_out;
        p.close();
        assert!(p.sell(a / 2, 0, NO_FEES, 2).is_ok());
        let before = p.stock;
        let _ = p.buy(ict(1), 0, NO_FEES, ict(7_200), 3);
        assert_eq!(p.stock, before);
    }

    /// Deterministic pseudo-random walk over buys, sells and new stock:
    /// real reserves never go negative, nothing is paid out of virtual
    /// reserves, and Alpha is conserved.
    #[test]
    fn random_walk_keeps_the_invariants() {
        let mut seed = 0x2545_F491_4F6C_DD1Du64;
        let mut rnd = |m: u64| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed % m
        };
        let mut p = AlphaPool::open(ict(1_000), 0);
        let mut iss = Issuance::default();
        let mut held = 0u128; // Alpha in traders' hands
        let mut minted_stock = 0u128;
        let mut burned_alpha = 0u128;
        for step in 0..20_000u64 {
            let block = step * 36;
            if step % 10 == 0 {
                let e = iss.issue(360, DEFAULT_BONUS_PPM);
                p.add_stock(e.stock);
                minted_stock += e.stock;
                held += e.bonus; // bonus goes to participants
            }
            match rnd(3) {
                0 => {
                    let amt = (rnd(50) as u128 + 1) * UNIT / 10;
                    if let Ok(t) = p.buy(amt, 0, DEFAULT_FEES, daily_cap(&iss), block) {
                        held += t.amount_out;
                    }
                }
                _ => {
                    if held > 0 {
                        let amt = held * (rnd(100) as u128 + 1) / 400;
                        if amt > 0 {
                            if let Ok(t) = p.sell(amt, 0, DEFAULT_FEES, block) {
                                held -= amt;
                                burned_alpha += t.burned;
                            }
                        }
                    }
                }
            }
            assert!(p.x() >= p.vx && p.y() >= p.vy);
            assert!(p.sold_today <= daily_cap(&iss));
        }
        // Alpha conservation: issued = held + on curve + stock + burned
        assert_eq!(iss.total(), held + p.ry + p.stock + burned_alpha);
        assert!(minted_stock >= p.stock);
    }
}
