//! Subnet deposit price (spec Section 4): each registration doubles the price,
//! which then decays linearly to the floor over `decay_blocks`.

use crate::mul_div;

pub fn deposit_price(
    last_price: u128,
    blocks_since_last: u64,
    floor: u128,
    decay_blocks: u64,
) -> u128 {
    if decay_blocks == 0 || blocks_since_last >= decay_blocks {
        return floor;
    }
    let doubled = last_price.saturating_mul(2);
    let left = (decay_blocks - blocks_since_last) as u128;
    mul_div(doubled, left, decay_blocks as u128).max(floor)
}

/// Deregistration: owner gets `return_ppm` of the withdrawn ICT side, the rest is burned.
pub fn deregistration_split(withdrawn_ict: u128, return_ppm: u128) -> (u128, u128) {
    let back = mul_div(withdrawn_ict, return_ppm, crate::PPM);
    (back, withdrawn_ict - back)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{BLOCKS_PER_DAY, UNIT};

    const FLOOR: u128 = 1_000 * UNIT;
    const DECAY: u64 = 14 * BLOCKS_PER_DAY;

    #[test]
    fn doubles_then_decays_to_floor() {
        assert_eq!(deposit_price(FLOOR, 0, FLOOR, DECAY), 2 * FLOOR);
        assert_eq!(deposit_price(4 * FLOOR, DECAY / 2, FLOOR, DECAY), 4 * FLOOR);
        assert_eq!(deposit_price(FLOOR, DECAY, FLOOR, DECAY), FLOOR);
        assert_eq!(deposit_price(FLOOR, DECAY / 2, FLOOR, DECAY), FLOOR);
    }

    #[test]
    fn ninety_percent_returned() {
        assert_eq!(
            deregistration_split(1_000 * UNIT, 900_000),
            (900 * UNIT, 100 * UNIT)
        );
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
