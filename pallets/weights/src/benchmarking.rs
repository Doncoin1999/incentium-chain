//! Benchmarks for commit-reveal weights.
#![cfg(feature = "runtime-benchmarks")]

use super::*;
use pallet_subnet_registry::{NeuronCount, NeuronUid, Neurons, SN0};
use polkadot_sdk::frame_benchmarking::v2::*;
use polkadot_sdk::frame_support::pallet_prelude::Get;
use polkadot_sdk::frame_system::RawOrigin;
use polkadot_sdk::sp_core::H256;

extern crate alloc;
use alloc::vec::Vec;

const SEED: u32 = 0;

fn validator<T: Config>() -> T::AccountId {
    let who: T::AccountId = account("validator", 0, SEED);
    Neurons::<T>::insert(SN0, 0, who.clone());
    NeuronUid::<T>::insert(SN0, who.clone(), 0);
    NeuronCount::<T>::insert(SN0, 1);
    who
}

#[benchmarks]
mod benchmarks {
    use super::*;

    #[benchmark]
    fn commit_weights() {
        let caller = validator::<T>();

        #[extrinsic_call]
        _(RawOrigin::Signed(caller.clone()), SN0, H256::repeat_byte(1));

        assert!(Commits::<T>::contains_key(SN0, &caller));
    }

    /// Reveal a vector of `n` weights (hashing and range checks are linear in `n`).
    #[benchmark]
    fn reveal_weights(n: Linear<1, { T::MaxWeights::get() }>) {
        let caller = validator::<T>();
        let neurons = n.max(1).min(u16::MAX as u32) as u16;
        NeuronCount::<T>::insert(SN0, neurons);
        let uids: Vec<u16> = (0..n as u16).collect();
        let weights: Vec<u16> = (0..n).map(|i| (i % 65_535) as u16 + 1).collect();
        let salt = [9u8; 32];
        let len = T::EpochLength::get();
        polkadot_sdk::frame_system::Pallet::<T>::set_block_number(len.into());
        let epoch = Pallet::<T>::current_epoch();
        let hash = Pallet::<T>::commit_hash(SN0, &uids, &weights, &salt);
        Commits::<T>::insert(SN0, &caller, (hash, epoch));
        polkadot_sdk::frame_system::Pallet::<T>::set_block_number((2 * len).into());

        #[extrinsic_call]
        _(RawOrigin::Signed(caller.clone()), SN0, uids, weights, salt);

        assert!(Revealed::<T>::contains_key((SN0, epoch), &caller));
    }

    impl_benchmark_test_suite!(Pallet, crate::mock::new_test_ext(), crate::mock::Test);
}
