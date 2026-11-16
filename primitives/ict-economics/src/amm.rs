//! Canonical constant-product pool between ICT (x) and one Alpha token (y)
//! (spec Section 7). The fee is taken in the input asset; `fee_burn_ppm` of it
//! is burned and the rest stays in the pool.

use crate::{mul_div, PPM};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Pool {
    pub ict: u128,
    pub alpha: u128,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SwapResult {
    pub amount_out: u128,
    /// Input-asset fee burned.
    pub burned: u128,
    /// Input-asset fee kept in the pool.
    pub kept: u128,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AmmError {
    EmptyPool,
    ZeroInput,
    SlippageExceeded,
}

impl Pool {
    /// Price in ICT base units per Alpha `unit` (scaled by `unit`).
    pub fn price_scaled(&self, unit: u128) -> u128 {
        mul_div(self.ict, unit, self.alpha)
    }

    fn swap(
        x: &mut u128,
        y: &mut u128,
        amount_in: u128,
        fee_ppm: u128,
        fee_burn_ppm: u128,
        min_out: u128,
    ) -> Result<SwapResult, AmmError> {
        if *x == 0 || *y == 0 {
            return Err(AmmError::EmptyPool);
        }
        if amount_in == 0 {
            return Err(AmmError::ZeroInput);
        }
        let fee = mul_div(amount_in, fee_ppm, PPM);
        let burned = mul_div(fee, fee_burn_ppm, PPM);
        let kept = fee - burned;
        let net = amount_in - fee;
        // out = y - k / (x + net), rounded so the pool never loses value
        let new_x = *x + net;
        let k_div = mul_div(*x, *y, new_x) + 1; // round up the remaining reserve
        let out = y.saturating_sub(k_div);
        if out < min_out {
            return Err(AmmError::SlippageExceeded);
        }
        *x = new_x + kept;
        *y -= out;
        Ok(SwapResult {
            amount_out: out,
            burned,
            kept,
        })
    }

    /// ICT in, Alpha out.
    pub fn buy_alpha(
        &mut self,
        ict_in: u128,
        fee_ppm: u128,
        fee_burn_ppm: u128,
        min_alpha: u128,
    ) -> Result<SwapResult, AmmError> {
        Self::swap(
            &mut self.ict,
            &mut self.alpha,
            ict_in,
            fee_ppm,
            fee_burn_ppm,
            min_alpha,
        )
    }

    /// Alpha in, ICT out.
    pub fn sell_alpha(
        &mut self,
        alpha_in: u128,
        fee_ppm: u128,
        fee_burn_ppm: u128,
        min_ict: u128,
    ) -> Result<SwapResult, AmmError> {
        Self::swap(
            &mut self.alpha,
            &mut self.ict,
            alpha_in,
            fee_ppm,
            fee_burn_ppm,
            min_ict,
        )
    }

    pub fn k(&self) -> u128 {
        self.ict.saturating_mul(self.alpha)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::UNIT;

    const FEE: u128 = 5_000; // 0.5 %
    const BURN: u128 = 500_000; // half the fee

    fn pool() -> Pool {
        Pool {
            ict: 1_000 * UNIT,
            alpha: 1_000 * UNIT,
        }
    }

    #[test]
    fn k_never_decreases_on_swaps() {
        let mut p = pool();
        let k0 = p.k();
        p.buy_alpha(100 * UNIT, FEE, BURN, 0).unwrap();
        assert!(p.k() >= k0);
        let k1 = p.k();
        p.sell_alpha(50 * UNIT, FEE, BURN, 0).unwrap();
        assert!(p.k() >= k1);
    }

    #[test]
    fn round_trip_loses_fees() {
        let mut p = pool();
        let a = p.buy_alpha(100 * UNIT, FEE, BURN, 0).unwrap();
        let b = p.sell_alpha(a.amount_out, FEE, BURN, 0).unwrap();
        assert!(b.amount_out < 100 * UNIT);
        assert_eq!(a.burned, 100 * UNIT * FEE / PPM / 2);
    }

    #[test]
    fn slippage_guard() {
        let mut p = pool();
        assert_eq!(
            p.buy_alpha(100 * UNIT, FEE, BURN, 100 * UNIT),
            Err(AmmError::SlippageExceeded)
        );
        assert_eq!(p, pool()); // unchanged on failure
    }

    #[test]
    fn matches_float_formula() {
        let mut p = pool();
        let r = p.buy_alpha(100 * UNIT, FEE, BURN, 0).unwrap();
        let net = 100.0 * (1.0 - 0.005);
        let expected = 1000.0 - 1000.0 * 1000.0 / (1000.0 + net);
        let got = r.amount_out as f64 / UNIT as f64;
        assert!((got - expected).abs() < 1e-6, "{got} vs {expected}");
    }
}
