//! Yuma-lite: the first, on-chain version of IPM consensus for SN0 (spec Section 6).
//!
//! Inputs: each validator's stake and its weight vector over miners.
//! 1. Effective stake = sqrt(stake), capped at `max_validator_ppm` of the total.
//! 2. Rows are normalised to ppm.
//! 3. Consensus weight c_j = the largest value v such that validators holding at
//!    least `kappa_ppm` of effective stake gave miner j a weight >= v.
//! 4. Weights above consensus are clipped to c_j.
//! 5. Miner incentive ~ sum_i s_i * clipped_ij; validator dividend ~ s_i * sum_j clipped_ij.
//!
//! Bonds (rewarding early agreement over time) are left for the proof-based version.

use crate::allocation::{cap_and_floor, normalize};
use crate::{isqrt, mul_div, PPM};
use alloc::vec::Vec;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct YumaOutput {
    /// Miner incentive shares in ppm (sum == PPM if any weight survives, else all 0).
    pub incentive: Vec<u128>,
    /// Validator dividend shares in ppm (sum == PPM if any weight survives, else all 0).
    pub dividends: Vec<u128>,
    /// Consensus weight per miner (ppm of a row).
    pub consensus: Vec<u128>,
}

/// Effective stake shares: sqrt of stake, capped per validator. Returns ppm.
pub fn effective_stake(stakes: &[u128], max_validator_ppm: u128) -> Vec<u128> {
    let roots: Vec<u128> = stakes.iter().map(|s| isqrt(*s)).collect();
    if roots.iter().all(|r| *r == 0) {
        return alloc::vec![0; stakes.len()];
    }
    let floors = alloc::vec![0; stakes.len()];
    cap_and_floor(&roots, max_validator_ppm, &floors)
}

/// `weights[i][j]`: validator i's raw weight for miner j (any scale).
pub fn yuma_lite(
    stakes: &[u128],
    weights: &[Vec<u128>],
    n_miners: usize,
    kappa_ppm: u128,
    max_validator_ppm: u128,
) -> YumaOutput {
    let nv = stakes.len();
    let s = effective_stake(stakes, max_validator_ppm);
    let w: Vec<Vec<u128>> = weights
        .iter()
        .map(|row| {
            let mut r = row.clone();
            r.resize(n_miners, 0);
            if r.iter().sum::<u128>() == 0 {
                r
            } else {
                normalize(&r)
            }
        })
        .collect();

    // Only validators that submitted a non-zero row count toward the stake total.
    let active: Vec<bool> = w.iter().map(|r| r.iter().any(|x| *x > 0)).collect();
    let active_stake: u128 = (0..nv).filter(|i| active[*i]).map(|i| s[i]).sum();

    let mut consensus = alloc::vec![0u128; n_miners];
    if active_stake > 0 {
        let need = mul_div(active_stake, kappa_ppm, PPM);
        for (j, c) in consensus.iter_mut().enumerate() {
            // candidate values sorted descending; accumulate stake until `need` is met
            let mut vals: Vec<(u128, u128)> = (0..nv)
                .filter(|i| active[*i])
                .map(|i| (w[i][j], s[i]))
                .collect();
            vals.sort_by_key(|v| core::cmp::Reverse(v.0));
            let mut acc = 0u128;
            for (v, st) in vals {
                acc += st;
                if acc >= need {
                    *c = v;
                    break;
                }
            }
        }
    }

    let mut rank = alloc::vec![0u128; n_miners];
    let mut div_raw = alloc::vec![0u128; nv];
    for i in 0..nv {
        if !active[i] {
            continue;
        }
        let mut kept = 0u128;
        for j in 0..n_miners {
            let clipped = w[i][j].min(consensus[j]);
            rank[j] += mul_div(s[i], clipped, PPM);
            kept += clipped;
        }
        div_raw[i] = mul_div(s[i], kept, PPM);
    }
    let incentive = if rank.iter().sum::<u128>() > 0 {
        normalize(&rank)
    } else {
        alloc::vec![0; n_miners]
    };
    let dividends = if div_raw.iter().sum::<u128>() > 0 {
        normalize(&div_raw)
    } else {
        alloc::vec![0; nv]
    };
    YumaOutput {
        incentive,
        dividends,
        consensus,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    const KAPPA: u128 = 500_000;
    const CAP: u128 = 100_000;

    #[test]
    fn honest_majority_sets_incentive() {
        // three equal validators agree: miner 0 does 3x the work of miner 1
        let stakes = vec![100, 100, 100];
        let w = vec![vec![75, 25], vec![75, 25], vec![75, 25]];
        let out = yuma_lite(&stakes, &w, 2, KAPPA, CAP);
        assert_eq!(out.incentive.iter().sum::<u128>(), PPM);
        assert!((out.incentive[0] as i128 - 750_000).abs() <= 2);
        // all validators agreed, so equal dividends
        assert!(out
            .dividends
            .iter()
            .all(|d| (*d as i128 - 333_333).abs() <= 2));
    }

    #[test]
    fn outlier_validator_is_clipped() {
        // validator 2 tries to give everything to miner 1 (its own miner)
        let stakes = vec![100, 100, 100];
        let w = vec![vec![75, 25], vec![75, 25], vec![0, 100]];
        let out = yuma_lite(&stakes, &w, 2, KAPPA, CAP);
        // The outlier's weight above consensus is clipped away: miner 1 gets at most what
        // the honest majority gave it plus the outlier's clipped share (1/3 here).
        assert!(out.incentive[0] > 600_000, "{:?}", out.incentive);
        assert!(out.incentive[1] <= 333_334, "{:?}", out.incentive);
        assert!(out.dividends[2] < out.dividends[0] / 2);
    }

    #[test]
    fn sqrt_and_cap_limit_a_whale() {
        // 1 whale with 100x stake vs 9 small validators: sqrt -> 10x, then capped
        let mut stakes = vec![10_000u128];
        stakes.extend(vec![100u128; 19]);
        let s = effective_stake(&stakes, CAP);
        assert!(s[0] <= CAP);
        assert_eq!(s.iter().sum::<u128>(), PPM);
    }

    #[test]
    fn no_weights_no_payout() {
        let out = yuma_lite(&[100, 100], &[vec![], vec![]], 3, KAPPA, CAP);
        assert!(out.incentive.iter().all(|x| *x == 0));
        assert!(out.dividends.iter().all(|x| *x == 0));
    }
}
