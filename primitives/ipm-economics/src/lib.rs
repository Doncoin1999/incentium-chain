//! Economic rules of IPM (Intelligence Protocol Mechanism), implemented as pure
//! integer functions so that the runtime pallets and off-chain tools share one
//! source of truth. Specification: "IPM Technical Specification v0.1".
//!
//! Units: 1 IPM = 10^9 base units (`UNIT`). Shares and rates are parts per
//! million (`PPM`). Every division rounds down.
#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

pub mod allocation;
pub mod amm;
pub mod buyback;
pub mod emission;
pub mod registry;
pub mod yuma;

/// Base units per IPM (and per Alpha).
pub const UNIT: u128 = 1_000_000_000;
/// Parts per million.
pub const PPM: u128 = 1_000_000;
/// Blocks per day at 12-second blocks.
pub const BLOCKS_PER_DAY: u64 = 7_200;

/// Integer square root (floor).
pub fn isqrt(n: u128) -> u128 {
    if n < 2 {
        return n;
    }
    let mut x = 1u128 << ((128 - n.leading_zeros()).div_ceil(2));
    loop {
        let y = (x + n / x) / 2;
        if y >= x {
            return x;
        }
        x = y;
    }
}

/// a * b / c without intermediate overflow for the value ranges used here
/// (all amounts < 2^64 base units in practice; falls back to split multiply).
pub fn mul_div(a: u128, b: u128, c: u128) -> u128 {
    if c == 0 {
        return 0;
    }
    match a.checked_mul(b) {
        Some(p) => p / c,
        None => {
            // (a / c) * b + (a % c) * b / c
            let q = a / c;
            let r = a % c;
            q.saturating_mul(b).saturating_add(r.saturating_mul(b) / c)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn isqrt_is_floor() {
        for n in [0u128, 1, 2, 3, 4, 15, 16, 17, 1_000_000, u64::MAX as u128] {
            let r = isqrt(n);
            assert!(r * r <= n);
            assert!((r + 1) * (r + 1) > n);
        }
    }

    #[test]
    fn mul_div_basic() {
        assert_eq!(mul_div(10, 20, 3), 66);
        assert_eq!(mul_div(u128::MAX / 2, 4, 8), u128::MAX / 4);
    }
}
