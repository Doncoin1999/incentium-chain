//! Benchmarks for the subnet registry. Each call is measured in its most expensive path.
#![cfg(feature = "runtime-benchmarks")]

use super::*;
use alloc::vec;
use polkadot_sdk::frame_benchmarking::v2::*;
use polkadot_sdk::frame_support::{
    pallet_prelude::{BoundedVec, ConstU32, Get},
    traits::fungible::Mutate,
};
use polkadot_sdk::frame_system::RawOrigin;

extern crate alloc;

const SEED: u32 = 0;

fn funded<T: Config>(name: &'static str, i: u32) -> T::AccountId {
    let who: T::AccountId = account(name, i, SEED);
    let amount = 100_000_000u128 * 1_000_000_000 + T::NeuronFee::get() * 100;
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
        AuctionStart::<T>::put(0);
        let max = MaxSubnets::<T>::get();
        for n in 1..max {
            add_subnet::<T>(n, None, 0);
        }
        SubnetCount::<T>::put(max - 1);
        let _ = T::Currency::mint_into(&T::TreasuryAccount::get(), 1_000_000_000);
        let caller = funded::<T>("caller", 0);

        #[extrinsic_call]
        _(
            RawOrigin::Signed(caller.clone()),
            u128::MAX,
            vec![b'n'; 64],
            vec![b'd'; 512],
            [7u8; 32],
        );

        assert_eq!(SubnetCount::<T>::get(), max);
    }

    /// `n` neurons, each with an endpoint and a declared model, all removed.
    #[benchmark]
    fn deregister_subnet(n: Linear<0, { T::MaxNeurons::get() as u32 }>) {
        let owner = funded::<T>("owner", 0);
        add_subnet::<T>(1, Some(owner), 0);
        SubnetCount::<T>::put(1);
        for i in 0..n {
            let who: T::AccountId = account("neuron", i, SEED);
            add_neuron::<T>(1, &who);
            Endpoints::<T>::insert(1, &who, BoundedVec::truncate_from(vec![b'x'; 128]));
            NeuronModel::<T>::insert(1, &who, 1);
        }
        polkadot_sdk::frame_system::Pallet::<T>::set_block_number(
            polkadot_sdk::sp_runtime::SaturatedConversion::saturated_into(
                IMMUNITY_BLOCKS as u64 + 1,
            ),
        );

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
    fn declare_model() {
        let caller = funded::<T>("caller", 0);
        add_neuron::<T>(SN0, &caller);

        #[extrinsic_call]
        _(RawOrigin::Signed(caller.clone()), SN0, 0);

        assert!(NeuronModel::<T>::contains_key(SN0, &caller));
    }

    /// Opening a proposed model with every SN0 slot holding a declaration (worst case for
    /// counting holders); the deposit goes back to the proposer.
    #[benchmark]
    fn set_model() {
        let proposer = funded::<T>("proposer", 0);
        Pallet::<T>::propose_model(
            RawOrigin::Signed(proposer).into(),
            vec![b'm'; 64],
            [7u8; 32],
            vec![b'e'; 64],
            1_000,
            1_000_000,
            350_000,
        )
        .unwrap();
        let id = NextModelId::<T>::get() - 1;
        for i in 0..T::MaxNeurons::get() as u32 {
            let who: T::AccountId = account("validator", i, SEED);
            add_neuron::<T>(SN0, &who);
            T::StakeInfo::set_sn0_stake(&who, T::ModelDeclarerStake::get());
            let models: BoundedVec<ModelId, ConstU32<MAX_MODELS>> =
                BoundedVec::truncate_from(vec![id]);
            ValidatorModels::<T>::insert(&who, models);
        }

        #[extrinsic_call]
        _(RawOrigin::Root, id, 1_000_000u32, 350_000u32, ModelStatus::Open, None);

        assert_eq!(Models::<T>::get(id).map(|m| m.status), Some(ModelStatus::Open));
    }

    #[benchmark]
    fn propose_model() {
        let caller = funded::<T>("caller", 0);

        #[extrinsic_call]
        _(
            RawOrigin::Signed(caller),
            vec![b'm'; 64],
            [7u8; 32],
            vec![b'e'; 64],
            1_000u32,
            1_000_000u32,
            350_000u32,
        );

        assert!(Models::<T>::contains_key(NextModelId::<T>::get() - 1));
    }

    #[benchmark]
    fn withdraw_model() {
        let caller = funded::<T>("caller", 0);
        Pallet::<T>::propose_model(
            RawOrigin::Signed(caller.clone()).into(),
            vec![b'm'; 64],
            [7u8; 32],
            vec![b'e'; 64],
            1_000,
            1_000_000,
            350_000,
        )
        .unwrap();
        let id = NextModelId::<T>::get() - 1;

        #[extrinsic_call]
        _(RawOrigin::Signed(caller), id);

        assert_eq!(Models::<T>::get(id).map(|m| m.status), Some(ModelStatus::Closed));
    }

    #[benchmark]
    fn declare_validator_models(n: Linear<1, 6>) {
        let caller = funded::<T>("caller", 0);
        add_neuron::<T>(SN0, &caller);
        T::StakeInfo::set_sn0_stake(&caller, T::ModelDeclarerStake::get());
        let models: Vec<ModelId> = (0..n as ModelId).filter(|m| *m != 3).collect();

        #[extrinsic_call]
        _(RawOrigin::Signed(caller.clone()), models);
    }

    impl_benchmark_test_suite!(Pallet, crate::mock::new_test_ext(false), crate::mock::Test);
}
