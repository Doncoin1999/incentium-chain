//! Subnet slots are sold by Dutch auction (subnet registration specification, 25 Nov 2026):
//!
//! - SN1 starts at 50,000 ICT; every later slot starts at 4 x the price the previous slot
//!   sold for.
//! - The price falls by 25 % per day, smoothly per block (x 0.75^(blocks / 7,200)), until it
//!   reaches the floor of that slot, where it stays until someone buys.
//! - Floors by sale number: 1st 1,000 ICT, 2nd-4th 500, 5th-8th 300, 9th and later 200.
//! - The payment is not refunded: 50 % goes to the treasury, 50 % is burned.

use crate::{mul_div, BLOCKS_PER_DAY, PPM, UNIT};

/// Start price of the first slot (SN1).
pub const FIRST_START_PRICE: u128 = 50_000 * UNIT;
/// A later slot starts at this multiple of the previous sale price.
pub const START_MULTIPLIER: u128 = 4;
/// Price kept after one day, in ppm (75 % = a 25 % fall per day).
pub const DAILY_KEEP_PPM: u128 = 750_000;
/// Share of the payment that goes to the treasury, in ppm; the rest is burned.
pub const TREASURY_PPM: u128 = 500_000;

/// 0.75^(1/7,200) in 18-decimal fixed point (rounded down): the price kept per block.
const PER_BLOCK_KEEP: u128 = 999_960_044_954_827_998;
const FIXED_ONE: u128 = 1_000_000_000_000_000_000;

/// Floor price of the `sale`-th slot sold (1-based).
pub fn floor_price(sale: u32) -> u128 {
    match sale {
        0 | 1 => 1_000 * UNIT,
        2..=4 => 500 * UNIT,
        5..=8 => 300 * UNIT,
        _ => 200 * UNIT,
    }
}

/// Start price of the `sale`-th slot, given what the previous slot sold for.
pub fn start_price(sale: u32, previous_sale_price: u128) -> u128 {
    if sale <= 1 {
        FIRST_START_PRICE
    } else {
        previous_sale_price.saturating_mul(START_MULTIPLIER)
    }
}

fn fixed_mul(a: u128, b: u128) -> u128 {
    mul_div(a, b, FIXED_ONE)
}

/// x^n in 18-decimal fixed point, for x <= 1.
fn fixed_pow(mut x: u128, mut n: u64) -> u128 {
    let mut acc = FIXED_ONE;
    while n > 0 {
        if n & 1 == 1 {
            acc = fixed_mul(acc, x);
        }
        x = fixed_mul(x, x);
        n >>= 1;
    }
    acc
}

/// Auction price `elapsed` blocks after the auction started: start x 0.75^(elapsed / 7,200),
/// never below `floor`.
pub fn auction_price(start: u128, elapsed: u64, floor: u128) -> u128 {
    let days = elapsed / BLOCKS_PER_DAY;
    let rest = elapsed % BLOCKS_PER_DAY;
    let mut p = start;
    for _ in 0..days {
        if p <= floor {
            return floor;
        }
        p = mul_div(p, DAILY_KEEP_PPM, PPM);
    }
    if rest > 0 {
        p = mul_div(p, fixed_pow(PER_BLOCK_KEEP, rest), FIXED_ONE);
    }
    p.max(floor)
}

/// Split of a payment: (to treasury, burned).
pub fn payment_split(price: u128) -> (u128, u128) {
    let treasury = mul_div(price, TREASURY_PPM, PPM);
    (treasury, price - treasury)
}

#[cfg(test)]
mod tests {
    use super::*;

    const D: u64 = BLOCKS_PER_DAY;

    fn ict(x: u128) -> u128 {
        x / UNIT
    }

    #[test]
    fn floors_by_sale_number() {
        assert_eq!(floor_price(1), 1_000 * UNIT);
        for s in 2..=4 {
            assert_eq!(floor_price(s), 500 * UNIT);
        }
        for s in 5..=8 {
            assert_eq!(floor_price(s), 300 * UNIT);
        }
        assert_eq!(floor_price(9), 200 * UNIT);
        assert_eq!(floor_price(40), 200 * UNIT);
    }

    #[test]
    fn start_prices() {
        assert_eq!(start_price(1, 0), 50_000 * UNIT);
        assert_eq!(start_price(2, 3_000 * UNIT), 12_000 * UNIT);
    }

    #[test]
    fn sn1_falls_25_percent_a_day_to_the_floor() {
        let f = floor_price(1);
        let p = |d: u64| ict(auction_price(FIRST_START_PRICE, d * D, f));
        assert_eq!(p(0), 50_000);
        assert_eq!(p(1), 37_500);
        assert_eq!(p(5), 11_865);
        assert_eq!(p(10), 2_815);
        assert_eq!(p(13), 1_187);
        assert_eq!(p(14), 1_000);
        assert_eq!(p(400), 1_000);
    }

    #[test]
    fn falls_smoothly_within_a_day() {
        let f = floor_price(1);
        let half = auction_price(FIRST_START_PRICE, D / 2, f);
        // 50,000 x 0.75^0.5 = 43,301.27
        assert_eq!(ict(half), 43_301);
        let mut last = FIRST_START_PRICE;
        for b in (0..2 * D).step_by(97) {
            let p = auction_price(FIRST_START_PRICE, b, f);
            assert!(p <= last);
            last = p;
        }
        // The smooth curve meets the daily steps.
        let end_of_day = auction_price(FIRST_START_PRICE, D - 1, f);
        assert!(end_of_day >= 37_500 * UNIT && end_of_day < 37_502 * UNIT);
    }

    #[test]
    fn later_floors_apply() {
        assert_eq!(
            auction_price(4_000 * UNIT, 30 * D, floor_price(2)),
            500 * UNIT
        );
        assert_eq!(
            auction_price(4_000 * UNIT, 30 * D, floor_price(6)),
            300 * UNIT
        );
        assert_eq!(
            auction_price(4_000 * UNIT, 60 * D, floor_price(12)),
            200 * UNIT
        );
    }

    #[test]
    fn half_to_treasury_half_burned() {
        assert_eq!(payment_split(1_001), (500, 501));
        assert_eq!(payment_split(10_000 * UNIT), (5_000 * UNIT, 5_000 * UNIT));
    }
}

/// Share (ppm) of the locked launch-period rewards that is unlocked `elapsed` blocks after
/// the launch period ended: fully locked for `cliff` blocks, then `step_ppm` released at the
/// end of the cliff and again every `step` blocks. Defaults: first 10 % two months after the
/// launch period (day 150), then 10 % every 2 months, fully unlocked 20 months after it.
pub fn launch_unlocked_ppm(elapsed: u64, cliff: u64, step: u64, step_ppm: u128) -> u128 {
    if step == 0 || elapsed < cliff {
        return 0;
    }
    let steps = ((elapsed - cliff) / step + 1) as u128;
    (steps * step_ppm).min(crate::PPM)
}

#[cfg(test)]
mod launch_tests {
    use super::*;

    const M: u64 = 30 * 7_200; // one month of 12-second blocks

    #[test]
    fn two_month_lock_then_ten_percent_every_two_months() {
        let f = |m: u64| launch_unlocked_ppm(m * M, 2 * M, 2 * M, 100_000);
        assert_eq!(f(0), 0);
        assert_eq!(launch_unlocked_ppm(2 * M - 1, 2 * M, 2 * M, 100_000), 0); // last locked block
        assert_eq!(f(2), 100_000); // day 150: first 10 %
        assert_eq!(f(3), 100_000);
        assert_eq!(f(4), 200_000);
        assert_eq!(f(12), 600_000);
        assert_eq!(f(19), 900_000);
        assert_eq!(f(20), 1_000_000); // fully unlocked
        assert_eq!(f(40), 1_000_000);
    }
}
