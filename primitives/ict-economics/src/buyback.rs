//! Per-block subnet settlement (spec Section 7, order of operations):
//! budget -> price-aware Alpha injection -> buyback swap -> burn half / stakers half
//! -> ICT payouts to miners, validators and owner.

use crate::amm::{AmmError, Pool};
use crate::{mul_div, PPM};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SubnetParams {
    pub buyback_ppm: u128,      // 500_000 (protected minimum)
    pub miner_ppm: u128,        // 225_000
    pub validator_ppm: u128,    // 200_000
    pub owner_ppm: u128,        // 75_000
    pub buyback_burn_ppm: u128, // 500_000 of bought Alpha is burned
    pub swap_fee_ppm: u128,     // 5_000
    pub fee_burn_ppm: u128,     // 500_000
}

impl Default for SubnetParams {
    fn default() -> Self {
        Self {
            buyback_ppm: 500_000,
            miner_ppm: 225_000,
            validator_ppm: 200_000,
            owner_ppm: 75_000,
            buyback_burn_ppm: 500_000,
            swap_fee_ppm: 5_000,
            fee_burn_ppm: 500_000,
        }
    }
}

impl SubnetParams {
    pub fn is_valid(&self) -> bool {
        self.buyback_ppm >= 500_000
            && self.buyback_ppm + self.miner_ppm + self.validator_ppm + self.owner_ppm == PPM
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Settlement {
    pub alpha_injected: u128,
    pub alpha_bought: u128,
    pub alpha_burned: u128,
    pub alpha_to_stakers: u128,
    pub ict_fee_burned: u128,
    pub to_miners: u128,
    pub to_validators: u128,
    pub to_owner: u128,
}

/// Price-aware injection: Alpha added to the pool is at most what the buyback
/// absorbs at the current price, and at most the schedule allows.
pub fn price_aware_injection(pool: &Pool, buyback_ict: u128, scheduled_alpha: u128) -> u128 {
    if pool.ict == 0 {
        return 0;
    }
    let absorbable = mul_div(buyback_ict, pool.alpha, pool.ict);
    absorbable.min(scheduled_alpha)
}

/// Settle one block for one subnet. `issuance` is the subnet's ICT for the block,
/// `revenue` is user revenue routed to it, `revenue_funded_ppm` is the share of the
/// issuance-funded buyback replaced by revenue in Phase 3 (0 before Phase 3).
pub fn settle_block(
    pool: &mut Pool,
    p: &SubnetParams,
    issuance: u128,
    revenue: u128,
    revenue_funded_ppm: u128,
    scheduled_alpha: u128,
) -> Result<Settlement, AmmError> {
    let base_buyback = mul_div(issuance, p.buyback_ppm, PPM);
    let from_issuance = mul_div(base_buyback, PPM - revenue_funded_ppm.min(PPM), PPM);
    let freed = base_buyback - from_issuance; // goes to miners and validators
    let budget = from_issuance + revenue;

    let mut s = Settlement {
        alpha_injected: price_aware_injection(pool, budget, scheduled_alpha),
        ..Default::default()
    };
    pool.alpha += s.alpha_injected;

    if budget > 0 {
        let r = pool.buy_alpha(budget, p.swap_fee_ppm, p.fee_burn_ppm, 0)?;
        s.alpha_bought = r.amount_out;
        s.ict_fee_burned = r.burned;
    }
    s.alpha_burned = mul_div(s.alpha_bought, p.buyback_burn_ppm, PPM);
    s.alpha_to_stakers = s.alpha_bought - s.alpha_burned;

    let mv = p.miner_ppm + p.validator_ppm;
    let freed_miners = mul_div(freed, p.miner_ppm, mv);
    s.to_owner = mul_div(issuance, p.owner_ppm, PPM);
    s.to_miners = mul_div(issuance, p.miner_ppm, PPM) + freed_miners;
    // validators take the rounding remainder so the split always sums to `issuance`
    s.to_validators = issuance - base_buyback - s.to_owner - mul_div(issuance, p.miner_ppm, PPM)
        + (freed - freed_miners);
    Ok(s)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::UNIT;

    #[test]
    fn default_params_valid() {
        assert!(SubnetParams::default().is_valid());
        let bad = SubnetParams {
            buyback_ppm: 400_000,
            miner_ppm: 325_000,
            ..Default::default()
        };
        assert!(!bad.is_valid());
    }

    #[test]
    fn injection_never_exceeds_what_buyback_absorbs() {
        let pool = Pool {
            ict: 2_000 * UNIT,
            alpha: 1_000 * UNIT,
        }; // price 2
        assert_eq!(
            price_aware_injection(&pool, 10 * UNIT, 1_000 * UNIT),
            5 * UNIT
        );
        assert_eq!(price_aware_injection(&pool, 10 * UNIT, 3 * UNIT), 3 * UNIT);
    }

    #[test]
    fn settlement_conserves_issuance() {
        let mut pool = Pool {
            ict: 1_000 * UNIT,
            alpha: 1_000 * UNIT,
        };
        let p = SubnetParams::default();
        let e = 9 * UNIT / 10; // 0.9 ICT for the block
        let s = settle_block(&mut pool, &p, e, 0, 0, UNIT).unwrap();
        let buyback = e / 2;
        assert_eq!(s.to_miners + s.to_validators + s.to_owner + buyback, e);
        assert_eq!(s.alpha_burned + s.alpha_to_stakers, s.alpha_bought);
    }

    #[test]
    fn injection_offsets_buyback_so_alpha_reserve_is_not_diluted() {
        // Price-aware injection adds only what the buyback removes: the Alpha side of
        // the pool stays about level while the ICT side grows by the buyback budget.
        let mut pool = Pool {
            ict: 1_000 * UNIT,
            alpha: 1_000 * UNIT,
        };
        let p = SubnetParams::default();
        for _ in 0..7_200 {
            settle_block(&mut pool, &p, 9 * UNIT / 10, 0, 0, UNIT).unwrap();
        }
        let alpha = pool.alpha as f64 / UNIT as f64;
        assert!((alpha - 1_000.0).abs() < 20.0, "alpha reserve {alpha}");
        assert!(pool.ict > 4_000 * UNIT); // 1,000 + 7,200 x 0.45 ICT (minus burned fees)
    }

    #[test]
    fn revenue_shift_moves_freed_issuance_to_miners_and_validators() {
        let mut pool = Pool {
            ict: 1_000 * UNIT,
            alpha: 1_000 * UNIT,
        };
        let p = SubnetParams::default();
        let e = UNIT;
        let s = settle_block(&mut pool, &p, e, 0, 1_000_000, 0).unwrap(); // fully revenue-funded
        assert_eq!(s.to_miners + s.to_validators + s.to_owner, e);
        assert_eq!(s.alpha_bought, 0); // no revenue this block -> no buyback
    }
}
