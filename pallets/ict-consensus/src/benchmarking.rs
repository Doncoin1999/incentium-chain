//! Benchmarks for staking, launch-reward claims and the per-epoch settlement.
#![cfg(feature = "runtime-benchmarks")]

use super::*;
use pallet_subnet_registry::{BootstrapUntil, NeuronCount, NeuronUid, Neurons, SN0};
use polkadot_sdk::frame_benchmarking::v2::*;
use polkadot_sdk::frame_support::{
    pallet_prelude::Get,
    traits::fungible::{Inspect, Mutate},
    BoundedVec,
};
use polkadot_sdk::frame_system::RawOrigin;
use polkadot_sdk::sp_runtime::traits::SaturatedConversion;

extern crate alloc;
use alloc::vec::Vec;

const SEED: u32 = 0;

fn big<T: Config>() -> u128 {
    T::MinValidatorStake::get()
        .saturating_mul(1_000)
        .max(1_000_000_000_000_000)
}

fn funded<T: Config>(name: &'static str, i: u32) -> T::AccountId {
    let who: T::AccountId = account(name, i, SEED);
    let _ = T::StakeCurrency::mint_into(&who, big::<T>());
    who
}

fn add_neuron<T: Config>(who: &T::AccountId) -> u16 {
    let uid = NeuronCount::<T>::get(SN0);
    Neurons::<T>::insert(SN0, uid, who.clone());
    NeuronUid::<T>::insert(SN0, who.clone(), uid);
    NeuronCount::<T>::insert(SN0, uid + 1);
    uid
}

fn set_block<T: Config>(n: u64) {
    polkadot_sdk::frame_system::Pallet::<T>::set_block_number(n.saturated_into());
}

#[benchmarks]
mod benchmarks {
    use super::*;

    /// Outside the launch period, so the minimum-stake check runs.
    #[benchmark]
    fn stake() {
        BootstrapUntil::<T>::put(0);
        let caller = funded::<T>("validator", 0);
        add_neuron::<T>(&caller);
        let amount = T::MinValidatorStake::get();

        #[extrinsic_call]
        _(RawOrigin::Signed(caller.clone()), amount);

        assert_eq!(Stake::<T>::get(&caller), amount);
    }

    /// On top of an earlier unstake request.
    #[benchmark]
    fn unstake() {
        let caller = funded::<T>("validator", 0);
        Stake::<T>::insert(&caller, 10u128);
        TotalStake::<T>::put(10u128);
        Unstaking::<T>::insert(&caller, (1u128, 0u64));

        #[extrinsic_call]
        _(RawOrigin::Signed(caller.clone()), 5);

        assert_eq!(Stake::<T>::get(&caller), 5);
    }

    #[benchmark]
    fn withdraw_unstaked() {
        let caller: T::AccountId = account("validator", 0, SEED);
        let _ = T::StakeCurrency::mint_into(&Pallet::<T>::account_id(), big::<T>());
        Unstaking::<T>::insert(&caller, (T::MinValidatorStake::get(), 0u64));

        #[extrinsic_call]
        _(RawOrigin::Signed(caller.clone()));

        assert!(Unstaking::<T>::get(&caller).is_none());
    }

    /// After the cliff and one release step.
    #[benchmark]
    fn claim_launch_rewards() {
        let caller: T::AccountId = account("miner", 0, SEED);
        BootstrapUntil::<T>::put(1);
        set_block::<T>(1 + T::LaunchCliff::get() + T::LaunchStep::get() + 1);
        let _ = T::StakeCurrency::mint_into(&Pallet::<T>::launch_vault(), big::<T>());
        LaunchLocked::<T>::insert(&caller, big::<T>() / 2);
        LaunchClaimed::<T>::insert(&caller, 1u128);

        #[extrinsic_call]
        _(RawOrigin::Signed(caller.clone()));

        assert!(LaunchClaimed::<T>::get(&caller) > 1);
    }

    /// Settle one epoch with `v` validators, each weighting all `n` miners, for work done in the
    /// launch period: every miner and validator gets a payment to a new account plus a locked
    /// share in the launch vault (the most expensive path).
    #[benchmark]
    fn settle(
        v: Linear<1, { <T as pallet_subnet_registry::Config>::MaxNeurons::get() as u32 }>,
        n: Linear<1, { <T as pallet_subnet_registry::Config>::MaxNeurons::get() as u32 }>,
    ) {
        BootstrapUntil::<T>::put(u32::MAX);
        let we: u64 = 5;
        set_block::<T>((we + 2) * <T as pallet_weights::Config>::EpochLength::get() as u64);
        for i in 0..n {
            let m: T::AccountId = account("miner", i, SEED);
            add_neuron::<T>(&m);
        }
        let max_w = <T as pallet_weights::Config>::MaxWeights::get();
        for j in 0..v {
            let val: T::AccountId = account("validator", j, SEED);
            Stake::<T>::insert(&val, 1_000u128 + j as u128);
            let row: Vec<(u16, u16)> = (0..n.min(max_w))
                .map(|u| (u as u16, 1 + ((u + j) % 1000) as u16))
                .collect();
            pallet_weights::Revealed::<T>::insert((SN0, we), &val, BoundedVec::truncate_from(row));
        }
        let pot = T::SubnetPot::get();
        let _ = T::StakeCurrency::mint_into(&pot, big::<T>());

        #[block]
        {
            Pallet::<T>::settle(we);
        }

        assert_eq!(LastSettled::<T>::get(), Some(we));
        // The epoch was paid out, not skipped.
        assert!(T::StakeCurrency::balance(&pot) < big::<T>() / 2);
    }

    impl_benchmark_test_suite!(Pallet, crate::mock::new_test_ext(), crate::mock::Test);
}
