//! Placeholder weights for `pallet-ict-swap`, sized from similar calls in this runtime
//! (a few storage reads and writes plus one balance hold). Replace with benchmarked values
//! before mainnet (spec Section 11, step 1).
use polkadot_sdk::frame_support::weights::Weight;

pub trait WeightInfo {
    fn place_sell() -> Weight;
    fn close() -> Weight;
    fn reserve() -> Weight;
    fn claim() -> Weight;
    fn expire() -> Weight;
    fn lock() -> Weight;
}

impl WeightInfo for () {
    fn place_sell() -> Weight {
        Weight::from_parts(60_000_000, 6_000)
    }
    fn close() -> Weight {
        Weight::from_parts(40_000_000, 4_000)
    }
    fn reserve() -> Weight {
        Weight::from_parts(40_000_000, 4_000)
    }
    fn claim() -> Weight {
        Weight::from_parts(60_000_000, 6_000)
    }
    fn expire() -> Weight {
        Weight::from_parts(40_000_000, 4_000)
    }
    fn lock() -> Weight {
        Weight::from_parts(50_000_000, 5_000)
    }
}
