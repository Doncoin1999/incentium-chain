//! Benchmarks for the emission pallet.
#![cfg(feature = "runtime-benchmarks")]

use super::*;
use polkadot_sdk::frame_benchmarking::v2::*;
use polkadot_sdk::frame_system::RawOrigin;

#[benchmarks]
mod benchmarks {
    use super::*;

    #[benchmark]
    fn set_fund_share() {
        #[extrinsic_call]
        _(RawOrigin::Root, 150_000u32);

        assert_eq!(FundShare::<T>::get(), 150_000);
    }

    /// One block's issuance (the per-block hook).
    #[benchmark]
    fn on_initialize() {
        let before = TotalIssued::<T>::get();
        #[block]
        {
            Pallet::<T>::issue_block();
        }
        assert!(TotalIssued::<T>::get() > before);
    }

    impl_benchmark_test_suite!(Pallet, crate::mock::new_test_ext(), crate::mock::Test);
}
