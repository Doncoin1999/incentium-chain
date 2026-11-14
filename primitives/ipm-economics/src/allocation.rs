//! Issuance shares between subnets (spec Section 8): metric L, Phase 3 mix,
//! floors for new subnets and a cap, all in parts per million.

use crate::{isqrt, mul_div, PPM};
use alloc::vec::Vec;

/// L = sqrt(protocol-owned IPM reserve * Alpha price * Alpha supply)
///   = sqrt(reserve_owned * (pool_ipm / pool_alpha) * alpha_supply).
/// `owned_ppm` is the share of pool liquidity not owned by third-party LPs.
pub fn locked_value(pool_ipm: u128, pool_alpha: u128, owned_ppm: u128, alpha_supply: u128) -> u128 {
    if pool_alpha == 0 {
        return 0;
    }
    let reserve_owned = mul_div(pool_ipm, owned_ppm, PPM);
    let mcap = mul_div(pool_ipm, alpha_supply, pool_alpha);
    // sqrt(a*b) = sqrt(a)*sqrt(b) keeps the product inside u128
    isqrt(reserve_owned) * isqrt(mcap)
}

/// Exponential moving average with N-epoch window: prev + 2/(N+1) * (x - prev).
pub fn ema(prev: u128, x: u128, n: u128) -> u128 {
    if x >= prev {
        prev + mul_div(x - prev, 2, n + 1)
    } else {
        prev - mul_div(prev - x, 2, n + 1)
    }
}

/// Normalise a vector to ppm shares (sum == PPM exactly, remainder to the largest).
pub fn normalize(v: &[u128]) -> Vec<u128> {
    let n = v.len();
    let total: u128 = v.iter().sum();
    if n == 0 {
        return Vec::new();
    }
    if total == 0 {
        let mut out = alloc::vec![PPM / n as u128; n];
        out[0] += PPM - (PPM / n as u128) * n as u128;
        return out;
    }
    let mut out: Vec<u128> = v.iter().map(|x| mul_div(*x, PPM, total)).collect();
    let rem = PPM - out.iter().sum::<u128>();
    let imax = (0..n).max_by_key(|i| v[*i]).unwrap();
    out[imax] += rem;
    out
}

/// Phase 3 mix: wl*norm(L) + wr*norm(R) + wf*norm(F), weights in ppm summing to PPM.
pub fn phase3_raw(l: &[u128], r: &[u128], f: &[u128], w: (u128, u128, u128)) -> Vec<u128> {
    let (nl, nr, nf) = (
        normalize_or_zero(l),
        normalize_or_zero(r),
        normalize_or_zero(f),
    );
    (0..l.len())
        .map(|i| mul_div(nl[i], w.0, PPM) + mul_div(nr[i], w.1, PPM) + mul_div(nf[i], w.2, PPM))
        .collect()
}

fn normalize_or_zero(v: &[u128]) -> Vec<u128> {
    if v.iter().sum::<u128>() == 0 {
        alloc::vec![0; v.len()]
    } else {
        normalize(v)
    }
}

/// Apply per-subnet floors, then the cap, redistributing pro rata (same algorithm
/// as the simulation). Returns ppm shares summing to PPM.
pub fn cap_and_floor(raw: &[u128], cap_ppm: u128, floors_ppm: &[u128]) -> Vec<u128> {
    let n = raw.len();
    if n == 0 {
        return Vec::new();
    }
    let s = normalize(raw);
    let cap = cap_ppm.max(PPM.div_ceil(n as u128));
    let mut fixed = alloc::vec![false; n];
    let mut out = s.clone();
    for _ in 0..64 {
        let budget = PPM - (0..n).filter(|i| fixed[*i]).map(|i| out[i]).sum::<u128>();
        let base: u128 = (0..n).filter(|i| !fixed[*i]).map(|i| s[i]).sum();
        let free: Vec<usize> = (0..n).filter(|i| !fixed[*i]).collect();
        if free.is_empty() {
            break;
        }
        for &i in &free {
            out[i] = if base > 0 {
                mul_div(s[i], budget, base)
            } else {
                budget / free.len() as u128
            };
        }
        let mut changed = false;
        for &i in &free {
            if out[i] > cap {
                out[i] = cap;
                fixed[i] = true;
                changed = true;
            } else if out[i] < floors_ppm[i] {
                out[i] = floors_ppm[i];
                fixed[i] = true;
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    // rounding remainder to the largest share that is below the cap
    let sum: u128 = out.iter().sum();
    if sum < PPM {
        if let Some(i) = (0..n).filter(|i| out[*i] < cap).max_by_key(|i| out[*i]) {
            out[i] += PPM - sum;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::UNIT;

    #[test]
    fn cap_and_floor_like_simulation() {
        let mut raw = alloc::vec![100u128; 1];
        raw.extend(std::iter::repeat_n(1, 9));
        let mut floors = alloc::vec![0u128; 10];
        floors[9] = 10_000;
        let s = cap_and_floor(&raw, 180_000, &floors);
        assert_eq!(s.iter().sum::<u128>(), PPM);
        assert!(s.iter().all(|x| *x <= 180_000));
        assert!(s[9] >= 10_000);
    }

    #[test]
    fn locked_value_geometric_mean() {
        // reserve 1,000 IPM, price 2, supply 4,000 Alpha -> mcap 8,000 -> sqrt(8e6) = 2828.4
        let l = locked_value(1_000 * UNIT, 500 * UNIT, PPM, 4_000 * UNIT);
        let expect = (1_000f64 * 8_000f64).sqrt() * UNIT as f64;
        assert!(((l as f64) - expect).abs() / expect < 1e-6);
    }

    #[test]
    fn third_party_liquidity_does_not_count() {
        let all = locked_value(1_000 * UNIT, 1_000 * UNIT, PPM, 1_000 * UNIT);
        let half = locked_value(2_000 * UNIT, 2_000 * UNIT, 500_000, 1_000 * UNIT);
        // doubling the pool with 50 % third-party LP leaves L unchanged
        assert_eq!(all, half);
    }

    #[test]
    fn phase3_weights_sum() {
        let s = phase3_raw(&[1, 3], &[2, 2], &[5, 1], (450_000, 450_000, 100_000));
        let total: u128 = s.iter().sum();
        assert!((PPM - 2..=PPM).contains(&total));
    }

    #[test]
    fn ema_moves_toward_input() {
        assert_eq!(ema(100, 100, 30), 100);
        assert!(ema(100, 200, 30) > 100);
        assert!(ema(100, 0, 30) < 100);
    }
}
