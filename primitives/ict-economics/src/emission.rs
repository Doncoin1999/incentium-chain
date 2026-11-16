//! ICT issuance (spec Section 3). The k-th halving happens when total issued
//! reaches MAX_SUPPLY * (1 - 2^-k). Protected parameters: MAX_SUPPLY, R0.

use crate::{mul_div, PPM, UNIT};

/// Maximum ICT supply in base units (protected).
pub const MAX_SUPPLY: u128 = 21_000_000 * UNIT;
/// Initial block reward in base units (protected).
pub const INITIAL_REWARD: u128 = UNIT;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct EmissionState {
    pub total_issued: u128,
    pub halvings: u8,
}

/// Reward per block after `k` halvings.
pub fn block_reward(k: u8) -> u128 {
    if k >= 64 {
        0
    } else {
        INITIAL_REWARD >> k
    }
}

/// Total issued at which halving number k+1 happens.
pub fn next_threshold(k: u8) -> u128 {
    if k >= 64 {
        return MAX_SUPPLY;
    }
    MAX_SUPPLY - (MAX_SUPPLY >> (k + 1))
}

/// Issue one block. Returns the amount issued; updates the state, crossing a
/// halving threshold exactly (the part above the threshold uses the new reward).
pub fn issue_block(s: &mut EmissionState) -> u128 {
    let mut remaining_fraction = PPM; // share of the block still to issue
    let mut out = 0u128;
    while remaining_fraction > 0 && s.total_issued < MAX_SUPPLY {
        let r = block_reward(s.halvings);
        if r == 0 {
            break;
        }
        let want = mul_div(r, remaining_fraction, PPM);
        if want == 0 {
            break;
        }
        let room = next_threshold(s.halvings).saturating_sub(s.total_issued);
        if want < room {
            s.total_issued += want;
            out += want;
            remaining_fraction = 0;
        } else {
            // issue up to the threshold, then halve and continue with the rest
            s.total_issued += room;
            out += room;
            let used = mul_div(room, PPM, r);
            remaining_fraction = remaining_fraction.saturating_sub(used);
            s.halvings = s.halvings.saturating_add(1);
        }
    }
    out
}

/// Split of one block's issuance (spec Section 3, rule 3).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Split {
    pub fund: u128,
    pub subnets: u128,
}

pub fn split(issued: u128, fund_ppm: u128) -> Split {
    let fund = mul_div(issued, fund_ppm, PPM);
    Split {
        fund,
        subnets: issued - fund,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::BLOCKS_PER_DAY;

    fn run_days(days: u64) -> EmissionState {
        let mut s = EmissionState::default();
        for _ in 0..days * BLOCKS_PER_DAY {
            issue_block(&mut s);
        }
        s
    }

    #[test]
    fn year_four_matches_simulation() {
        // Python simulation: 10,506,000 ICT issued after 4 x 365 days, reward 0.5
        let s = run_days(4 * 365);
        assert_eq!(s.total_issued, 10_506_000 * UNIT);
        assert_eq!(s.halvings, 1);
        assert_eq!(block_reward(s.halvings), UNIT / 2);
    }

    #[test]
    fn first_halving_on_day_1459() {
        let mut s = EmissionState::default();
        let mut block = 0u64;
        while s.halvings == 0 {
            issue_block(&mut s);
            block += 1;
        }
        // 10.5M ICT at 1 ICT/block = 10,500,000 blocks = day 1458.33
        assert_eq!(block, 10_500_000);
        assert_eq!(s.total_issued, 10_500_000 * UNIT);
    }

    #[test]
    fn supply_never_exceeds_cap() {
        // jump straight to a late state and issue to exhaustion
        let mut s = EmissionState {
            total_issued: MAX_SUPPLY - 3 * UNIT,
            halvings: 30,
        };
        for _ in 0..10_000_000u64 {
            if issue_block(&mut s) == 0 {
                break;
            }
        }
        assert!(s.total_issued <= MAX_SUPPLY);
    }

    #[test]
    fn crossing_threshold_inside_a_block() {
        let mut s = EmissionState {
            total_issued: next_threshold(0) - UNIT / 4,
            halvings: 0,
        };
        let got = issue_block(&mut s);
        // 0.25 ICT at the old reward, then 75% of a block at 0.5 ICT = 0.375
        assert_eq!(got, UNIT / 4 + 3 * UNIT / 8);
        assert_eq!(s.halvings, 1);
    }

    #[test]
    fn split_ten_percent_fund() {
        let sp = split(UNIT, 100_000);
        assert_eq!(sp.fund, UNIT / 10);
        assert_eq!(sp.fund + sp.subnets, UNIT);
    }
}
