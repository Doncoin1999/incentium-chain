//! Benchmarks for the subnet registry. Each call is measured in its most expensive path.
#![cfg(feature = "runtime-benchmarks")]

use super::*;
use alloc::vec;
use polkadot_sdk::frame_benchmarking::v2::*;
use polkadot_sdk::frame_support::{
    pallet_prelude::{BoundedVec, Get},
    traits::fungible::Mutate,
};
use polkadot_sdk::frame_system::RawOrigin;

extern crate alloc;

const SEED: u32 = 0;

fn funded<T: Config>(name: &'static str, i: u32) -> T::AccountId {
    let who: T::AccountId = account(name, i, SEED);
    let amount = T::DepositFloor::get()
        .saturating_mul(100)
        .saturating_add(T::NeuronFee::get() * 100);
    let _ = T::Currency::mint_into(&who, amount);
    who
}

/// Put `who` into subnet `netuid` directly (no fee, no events).
fn add_neuron<T: Config>(netuid: NetUid, who: &T::AccountId) -> Uid {
    let uid = NeuronCount::<T>::get(netuid);
    Neurons::<T>::insert(netuid, uid, who.clone());
    NeuronUid::<T>::insert(netuid, who.clone(), uid);
    NeuronCount::<T>::insert(netuid, uid + 1);
    uid
}

fn add_subnet<T: Config>(netuid: NetUid, owner: Option<T::AccountId>, deposit: u128) {
    Subnets::<T>::insert(
        netuid,
        SubnetInfo {
            owner,
            deposit,
            created: polkadot_sdk::frame_system::Pallet::<T>::block_number(),
        },
    );
}

#[benchmarks]
mod benchmarks {
    use super::*;

    /// Worst case: all but one subnet slot taken, so the free-netuid search scans them all.
    #[benchmark]
    fn register_subnet() {
        Phase2Enabled::<T>::put(true);
        let max = MaxSubnets::<T>::get();
        for n in 1..max {
            add_subnet::<T>(n, None, 0);
        }
        SubnetCount::<T>::put(max - 1);
        LastRegPrice::<T>::put(T::DepositFloor::get());
        LastRegBlock::<T>::put(polkadot_sdk::frame_system::Pallet::<T>::block_number());
        let caller = funded::<T>("caller", 0);

        #[extrinsic_call]
        _(RawOrigin::Signed(caller.clone()), u128::MAX);

        assert_eq!(SubnetCount::<T>::get(), max);
    }

    /// `n` neurons, each with an endpoint and a declared tier, all removed.
    #[benchmark]
    fn deregister_subnet(n: Linear<0, { T::MaxNeurons::get() as u32 }>) {
        let owner = funded::<T>("owner", 0);
        let deposit = T::DepositFloor::get();
        let _ = T::Currency::mint_into(&Pallet::<T>::account_id(), deposit * 2);
        add_subnet::<T>(1, Some(owner), deposit);
        SubnetCount::<T>::put(1);
        for i in 0..n {
            let who: T::AccountId = account("neuron", i, SEED);
            add_neuron::<T>(1, &who);
            Endpoints::<T>::insert(1, &who, BoundedVec::truncate_from(vec![b'x'; 128]));
            NeuronTier::<T>::insert(1, &who, 1);
        }

        #[extrinsic_call]
        _(RawOrigin::Root, 1);

        assert!(Subnets::<T>::get(1).is_none());
    }

    /// Outside the launch period: the fee is burned.
    #[benchmark]
    fn register_neuron() {
        BootstrapUntil::<T>::put(0);
        let caller = funded::<T>("caller", 0);
        let hotkey: T::AccountId = account("hotkey", 0, SEED);

        #[extrinsic_call]
        _(RawOrigin::Signed(caller), SN0, hotkey.clone());

        assert!(NeuronUid::<T>::contains_key(SN0, &hotkey));
    }

    #[benchmark]
    fn set_endpoint() {
        let caller = funded::<T>("caller", 0);
        add_neuron::<T>(SN0, &caller);

        #[extrinsic_call]
        _(RawOrigin::Signed(caller.clone()), SN0, vec![b'x'; 128]);

        assert!(Endpoints::<T>::contains_key(SN0, &caller));
    }

    /// Signed by the operator (the root path is cheaper).
    #[benchmark]
    fn end_bootstrap() {
        let caller = funded::<T>("operator", 0);
        BootstrapUntil::<T>::put(u32::MAX);
        BootstrapOperator::<T>::put(caller.clone());

        #[extrinsic_call]
        _(RawOrigin::Signed(caller));

        assert!(!Pallet::<T>::in_bootstrap());
    }

    #[benchmark]
    fn declare_tier() {
        let caller = funded::<T>("caller", 0);
        add_neuron::<T>(SN0, &caller);
        let mut info = default_tiers()[1].1.clone();
        info.open = true;
        Tiers::<T>::insert(1, info);

        #[extrinsic_call]
        _(RawOrigin::Signed(caller.clone()), SN0, 1);

        assert_eq!(NeuronTier::<T>::get(SN0, &caller), 1);
    }

    #[benchmark]
    fn set_tier() {
        #[extrinsic_call]
        _(
            RawOrigin::Root,
            3,
            5_000_000u32,
            400_000u32,
            vec![b'm'; 64],
            [7u8; 32],
            true,
        );

        assert!(Tiers::<T>::get(3).map(|t| t.open).unwrap_or(false));
    }

    #[benchmark]
    fn enable_phase2() {
        #[extrinsic_call]
        _(RawOrigin::Root);

        assert!(Phase2Enabled::<T>::get());
    }

    impl_benchmark_test_suite!(Pallet, crate::mock::new_test_ext(false), crate::mock::Test);
}
