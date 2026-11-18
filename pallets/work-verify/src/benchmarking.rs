//! Benchmarks for bonds, pools and failed-check reports. Each call in its most expensive path.
#![cfg(feature = "runtime-benchmarks")]

use super::*;
use pallet_subnet_registry::{NeuronCount, NeuronUid, Neurons, SN0};
use polkadot_sdk::frame_benchmarking::v2::*;
use polkadot_sdk::frame_support::{pallet_prelude::Get, traits::fungible::Mutate, BoundedVec};
use polkadot_sdk::frame_system::RawOrigin;
use polkadot_sdk::sp_core::H256;

extern crate alloc;
use alloc::vec::Vec;

const SEED: u32 = 0;

fn big<T: Config>() -> u128 {
    T::PoolBond::get()
        .saturating_mul(100)
        .max(1_000_000_000_000_000)
}

fn funded<T: Config>(name: &'static str, i: u32) -> T::AccountId {
    let who: T::AccountId = account(name, i, SEED);
    let _ = T::WorkCurrency::mint_into(&who, big::<T>());
    who
}

fn add_neuron<T: Config>(who: &T::AccountId) -> u16 {
    let uid = NeuronCount::<T>::get(SN0);
    Neurons::<T>::insert(SN0, uid, who.clone());
    NeuronUid::<T>::insert(SN0, who.clone(), uid);
    NeuronCount::<T>::insert(SN0, uid + 1);
    uid
}

/// A pool with the minimum bond, registered as an SN0 neuron.
fn pool<T: Config>() -> T::AccountId {
    let who = funded::<T>("pool", 0);
    add_neuron::<T>(&who);
    let _ = T::WorkCurrency::mint_into(&Pallet::<T>::account_id(), big::<T>());
    Bonds::<T>::insert(&who, T::PoolBond::get().saturating_mul(2));
    Pools::<T>::insert(
        &who,
        PoolInfo {
            fee_ppm: 10_000,
            next_fee: None,
            balance: 0,
        },
    );
    who
}

#[benchmarks]
mod benchmarks {
    use super::*;

    #[benchmark]
    fn bond() {
        let caller = funded::<T>("caller", 0);
        let amount = T::PoolBond::get();

        #[extrinsic_call]
        _(RawOrigin::Signed(caller.clone()), amount);

        assert_eq!(Bonds::<T>::get(&caller), amount);
    }

    /// A pool unbonding on top of an earlier request.
    #[benchmark]
    fn unbond() {
        let caller = pool::<T>();
        Unbonding::<T>::insert(&caller, (1u128, 0u64));

        #[extrinsic_call]
        _(RawOrigin::Signed(caller.clone()), 1);

        assert!(Unbonding::<T>::get(&caller).is_some());
    }

    #[benchmark]
    fn withdraw_unbonded() {
        let caller: T::AccountId = account("caller", 0, SEED);
        let _ = T::WorkCurrency::mint_into(&Pallet::<T>::account_id(), big::<T>());
        Unbonding::<T>::insert(&caller, (T::PoolBond::get(), 0u64));

        #[extrinsic_call]
        _(RawOrigin::Signed(caller.clone()));

        assert!(Unbonding::<T>::get(&caller).is_none());
    }

    #[benchmark]
    fn register_pool() {
        let caller = funded::<T>("caller", 0);
        add_neuron::<T>(&caller);
        Bonds::<T>::insert(&caller, T::PoolBond::get());

        #[extrinsic_call]
        _(RawOrigin::Signed(caller.clone()), T::MaxPoolFeePpm::get());

        assert!(Pools::<T>::contains_key(&caller));
    }

    #[benchmark]
    fn set_pool_fee() {
        let caller = pool::<T>();

        #[extrinsic_call]
        _(RawOrigin::Signed(caller.clone()), 0);

        assert!(Pools::<T>::get(&caller).unwrap().next_fee.is_some());
    }

    /// Pay `p` new accounts (each transfer creates an account) plus the operator fee.
    #[benchmark]
    fn distribute(p: Linear<1, { T::MaxPayees::get() }>) {
        let caller = pool::<T>();
        Pools::<T>::mutate(&caller, |i| {
            if let Some(i) = i {
                i.balance = big::<T>() / 2;
                i.next_fee = Some((20_000, 0));
            }
        });
        let shares: Vec<(T::AccountId, u64)> = (0..p)
            .map(|i| (account::<T::AccountId>("payee", i, SEED), 1u64))
            .collect();

        #[extrinsic_call]
        _(
            RawOrigin::Signed(caller.clone()),
            0u64,
            H256::repeat_byte(3),
            shares,
        );

        assert_eq!(Pools::<T>::get(&caller).unwrap().balance, 0);
    }

    /// `r` earlier reporters; this report reaches the quorum, slashes the miner's bond and
    /// bans the hotkey (its last allowed failure).
    #[benchmark]
    fn report_failed_check(r: Linear<0, { T::MaxReporters::get() - 1 }>) {
        let miner = funded::<T>("miner", 0);
        let uid = add_neuron::<T>(&miner);
        let _ = T::WorkCurrency::mint_into(&Pallet::<T>::account_id(), big::<T>());
        Bonds::<T>::insert(&miner, T::PoolBond::get());
        let earlier: Vec<u64> = (1..T::MaxFailures::get() as u64).collect();
        Failures::<T>::insert(&miner, BoundedVec::truncate_from(earlier));
        let epoch: u64 = 10;
        let mut reporters = Vec::new();
        for i in 0..r {
            let v: T::AccountId = account("reporter", i, SEED);
            T::Stake::set_stake(&v, 1);
            reporters.push(v);
        }
        let reported = r as u128;
        Reporters::<T>::insert(epoch, uid, BoundedVec::truncate_from(reporters));
        ReportedStake::<T>::insert(epoch, uid, reported);
        let caller: T::AccountId = account("caller", 0, SEED);
        T::Stake::set_stake(&caller, 1_000_000_000);

        #[extrinsic_call]
        _(RawOrigin::Signed(caller), epoch, uid, H256::repeat_byte(5));

        assert!(Penalized::<T>::get(epoch, uid));
    }

    impl_benchmark_test_suite!(Pallet, crate::mock::new_test_ext(), crate::mock::Test);
}
