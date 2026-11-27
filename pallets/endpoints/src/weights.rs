//! Placeholder weights for `pallet-endpoints`, sized from similar calls in this runtime.
//! Replace with benchmarked values before mainnet.
use polkadot_sdk::frame_support::weights::Weight;

pub trait WeightInfo {
    fn register() -> Weight;
    fn update() -> Weight;
    fn deregister() -> Weight;
    fn report(n: u32) -> Weight;
    fn set_reporter() -> Weight;
    fn set_params() -> Weight;
    fn settle(n: u32) -> Weight;
}

impl WeightInfo for () {
    fn register() -> Weight {
        Weight::from_parts(60_000_000, 6_000)
    }
    fn update() -> Weight {
        Weight::from_parts(40_000_000, 4_000)
    }
    fn deregister() -> Weight {
        Weight::from_parts(50_000_000, 5_000)
    }
    fn report(n: u32) -> Weight {
        Weight::from_parts(30_000_000, 3_000)
            .saturating_add(Weight::from_parts(15_000_000, 2_000).saturating_mul(n as u64))
    }
    fn set_reporter() -> Weight {
        Weight::from_parts(30_000_000, 3_000)
    }
    fn set_params() -> Weight {
        Weight::from_parts(15_000_000, 1_000)
    }
    fn settle(n: u32) -> Weight {
        Weight::from_parts(50_000_000, 5_000)
            .saturating_add(Weight::from_parts(40_000_000, 4_000).saturating_mul(n as u64))
    }
}
