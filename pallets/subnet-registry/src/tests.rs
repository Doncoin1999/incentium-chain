use crate::{
    mock::*, AuctionStart, Error, MaxSubnets, Metadata, MinSpacing, MinerHistory, NeuronCount,
    Neurons, Phase2Enabled, Sales, SubnetCount, Subnets, SwapAttestation, SwapAttested,
    IMMUNITY_BLOCKS, SN0, UNLOCK_MIN_BLOCK,
};
use ict_economics::UNIT;
use polkadot_sdk::frame_support::{
    assert_noop, assert_ok, pallet_prelude::BoundedVec, traits::Hooks,
};
use polkadot_sdk::sp_runtime::DispatchError;

#[test]
fn sn0_exists_at_genesis_without_owner() {
    new_test_ext(false).execute_with(|| {
        let sn0 = Subnets::<Test>::get(SN0).unwrap();
        assert_eq!(sn0.owner, None);
        assert_eq!(SubnetCount::<Test>::get(), 0);
    });
}

#[test]
fn neuron_registration_burns_fee() {
    new_test_ext(false).execute_with(|| {
        let issuance = Balances::total_issuance();
        assert_ok!(Registry::register_neuron(
            RuntimeOrigin::signed(ALICE),
            SN0,
            10
        ));
        assert_eq!(Neurons::<Test>::get(SN0, 0), Some(10));
        assert_eq!(Balances::total_issuance(), issuance - UNIT);
        assert_noop!(
            Registry::register_neuron(RuntimeOrigin::signed(ALICE), SN0, 10),
            Error::<Test>::AlreadyRegistered
        );
    });
}

#[test]
fn subnet_full() {
    new_test_ext(false).execute_with(|| {
        for hk in 10..14 {
            assert_ok!(Registry::register_neuron(
                RuntimeOrigin::signed(ALICE),
                SN0,
                hk
            ));
        }
        assert_eq!(NeuronCount::<Test>::get(SN0), 4);
        assert_noop!(
            Registry::register_neuron(RuntimeOrigin::signed(ALICE), SN0, 99),
            Error::<Test>::SubnetFull
        );
    });
}

fn reg(who: u64, max: u128) -> polkadot_sdk::sp_runtime::DispatchResult {
    Registry::register_subnet(
        RuntimeOrigin::signed(who),
        max,
        b"Inference".to_vec(),
        b"Open LLM inference".to_vec(),
        [7u8; 32],
    )
}

fn go(block: u32) {
    System::set_block_number(block as u64);
}

fn all_criteria_but(skip: u8) {
    if skip != 1 {
        go(UNLOCK_MIN_BLOCK);
    } else {
        go(UNLOCK_MIN_BLOCK - 1);
    }
    let miners = if skip == 2 { 299 } else { 300 };
    MinerHistory::<Test>::put(BoundedVec::truncate_from(vec![miners; 30]));
    SudoRemoved::set(skip != 3);
    ApprovedReferenda::set(if skip == 4 { 2 } else { 3 });
    if skip != 5 {
        SwapAttested::<Test>::put(SwapAttestation {
            contract: [1; 20],
            audit_hash: [2; 32],
        });
    }
    AlphaInstalled::set(skip != 6);
}

#[test]
fn unlock_needs_all_five_criteria() {
    for skip in 1..=6u8 {
        new_test_ext(false).execute_with(|| {
            all_criteria_but(skip);
            assert_noop!(
                Registry::try_unlock(RuntimeOrigin::signed(ALICE)),
                Error::<Test>::CriteriaNotMet
            );
            assert_noop!(reg(ALICE, u128::MAX), Error::<Test>::Phase2NotEnabled);
        });
    }
    new_test_ext(false).execute_with(|| {
        all_criteria_but(0);
        assert_ok!(Registry::try_unlock(RuntimeOrigin::signed(ALICE)));
        assert!(Phase2Enabled::<Test>::get());
        assert_eq!(AuctionStart::<Test>::get(), Some(UNLOCK_MIN_BLOCK));
        assert_eq!(Registry::current_price(), Some(50_000 * UNIT));
        assert_noop!(
            Registry::try_unlock(RuntimeOrigin::signed(ALICE)),
            Error::<Test>::AlreadyUnlocked
        );
    });
}

#[test]
fn unlocks_by_itself_at_the_daily_check() {
    new_test_ext(false).execute_with(|| {
        all_criteria_but(0);
        go(UNLOCK_MIN_BLOCK + 5);
        Registry::on_initialize((UNLOCK_MIN_BLOCK + 5) as u64);
        assert!(!Phase2Enabled::<Test>::get()); // not a day boundary
        let next_day = UNLOCK_MIN_BLOCK + DAY;
        go(next_day);
        Registry::on_initialize(next_day as u64);
        assert!(Phase2Enabled::<Test>::get());
        assert_eq!(AuctionStart::<Test>::get(), Some(next_day));
    });
}

#[test]
fn miner_average_rolls_daily_over_30_days() {
    new_test_ext(false).execute_with(|| {
        for day in 0..31u32 {
            go(day * DAY + 10);
            Registry::note_sn0_epoch(280);
            go(day * DAY + 400);
            Registry::note_sn0_epoch(320);
            if day < 30 {
                assert_eq!(Registry::miners_average(), None);
            }
        }
        // Day 30's tally rolls in on day 31.
        assert_eq!(MinerHistory::<Test>::get().len(), 30);
        assert_eq!(Registry::miners_average(), Some(300));
        go(31 * DAY + 1);
        Registry::note_sn0_epoch(0);
        assert_eq!(MinerHistory::<Test>::get().len(), 30);
        assert_eq!(Registry::miners_average(), Some(300));
        go(32 * DAY + 1);
        Registry::note_sn0_epoch(0);
        // One day of 0 replaced a day of 300.
        assert_eq!(Registry::miners_average(), Some(290));
    });
}

#[test]
fn dutch_auction_price_payment_and_metadata() {
    new_test_ext(true).execute_with(|| {
        AuctionStart::<Test>::put(1);
        assert_eq!(Registry::current_price(), Some(50_000 * UNIT));
        go(1 + DAY);
        assert_eq!(Registry::current_price(), Some(37_500 * UNIT));
        assert_noop!(reg(BOB, 37_000 * UNIT), Error::<Test>::DepositAboveLimit);
        assert_noop!(
            Registry::register_subnet(
                RuntimeOrigin::signed(BOB),
                u128::MAX,
                vec![],
                vec![],
                [0; 32]
            ),
            Error::<Test>::EmptyName
        );
        assert_noop!(
            Registry::register_subnet(
                RuntimeOrigin::signed(BOB),
                u128::MAX,
                vec![b'n'; 65],
                vec![],
                [0; 32]
            ),
            Error::<Test>::NameTooLong
        );

        let alice = Balances::free_balance(ALICE);
        let treasury = Balances::free_balance(TREASURY);
        let issuance = Balances::total_issuance();
        assert_ok!(reg(ALICE, 37_500 * UNIT));
        assert_eq!(Balances::free_balance(ALICE), alice - 37_500 * UNIT);
        assert_eq!(Balances::free_balance(TREASURY), treasury + 18_750 * UNIT);
        assert_eq!(Balances::total_issuance(), issuance - 18_750 * UNIT);

        let info = Subnets::<Test>::get(1).unwrap();
        assert_eq!(info.owner, Some(ALICE));
        assert_eq!(info.deposit, 37_500 * UNIT);
        let meta = Metadata::<Test>::get(1).unwrap();
        assert_eq!(meta.name.to_vec(), b"Inference".to_vec());
        assert_eq!(meta.code_hash, [7u8; 32]);
        assert_eq!(Sales::<Test>::get(), 1);

        // The next auction waits 60 days and starts at 4 x 37,500.
        let next = 1 + DAY + 60 * DAY;
        assert_eq!(AuctionStart::<Test>::get(), Some(next));
        assert_eq!(Registry::current_price(), None);
        assert_noop!(reg(BOB, u128::MAX), Error::<Test>::NoAuction);
        go(next);
        assert_eq!(Registry::current_price(), Some(150_000 * UNIT));
        go(next + 400 * DAY);
        assert_eq!(Registry::current_price(), Some(500 * UNIT));
    });
}

#[test]
fn floors_by_sale_number_and_cap_of_eight() {
    new_test_ext(true).execute_with(|| {
        AuctionStart::<Test>::put(1);
        let floors = [1_000u128, 500, 500, 500, 300, 300, 300, 300];
        for (i, floor) in floors.iter().enumerate() {
            let at = AuctionStart::<Test>::get().unwrap() + 100 * DAY;
            go(at);
            assert_eq!(
                Registry::current_price(),
                Some(floor * UNIT),
                "sale {}",
                i + 1
            );
            assert_ok!(reg(ALICE, floor * UNIT));
        }
        assert_eq!(SubnetCount::<Test>::get(), 8);
        go(AuctionStart::<Test>::get().unwrap() + DAY);
        assert_eq!(Registry::current_price(), None);
        assert_noop!(reg(BOB, u128::MAX), Error::<Test>::TooManySubnets);

        // Governance raises the cap: the waiting auction starts now, and sale 9 floors at 200.
        assert_ok!(Registry::set_max_subnets(RuntimeOrigin::root(), 9));
        let now = AuctionStart::<Test>::get().unwrap();
        assert_eq!(Registry::current_price(), Some(1_200 * UNIT)); // 4 x 300
        go(now + 100 * DAY);
        assert_eq!(Registry::current_price(), Some(200 * UNIT));
    });
}

#[test]
fn governance_can_close_a_subnet_any_time_without_refund_and_the_slot_reopens() {
    new_test_ext(true).execute_with(|| {
        AuctionStart::<Test>::put(1);
        assert_ok!(Registry::set_max_subnets(RuntimeOrigin::root(), 1));
        assert_ok!(reg(ALICE, u128::MAX));
        assert_eq!(crate::FullSince::<Test>::get(), Some(1));
        assert_ok!(Registry::register_neuron(RuntimeOrigin::signed(BOB), 1, 20));
        assert_noop!(
            Registry::deregister_subnet(RuntimeOrigin::root(), SN0),
            Error::<Test>::CannotDeregisterSn0
        );
        assert_noop!(
            Registry::deregister_subnet(RuntimeOrigin::signed(ALICE), 1),
            DispatchError::BadOrigin
        );
        // The emergency brake works inside the immunity too.
        go(1 + DAY);
        let alice = Balances::free_balance(ALICE);
        let issuance = Balances::total_issuance();
        assert_ok!(Registry::deregister_subnet(RuntimeOrigin::root(), 1));
        assert_eq!(Balances::free_balance(ALICE), alice);
        assert_eq!(Balances::total_issuance(), issuance);
        assert!(Subnets::<Test>::get(1).is_none());
        assert!(Metadata::<Test>::get(1).is_none());
        assert_eq!(NeuronCount::<Test>::get(1), 0);
        assert_eq!(SubnetCount::<Test>::get(), 0);
        assert_eq!(crate::FullSince::<Test>::get(), None);
        // The next auction still keeps its 60-day spacing after the sale, at 4 x 50,000.
        assert_eq!(AuctionStart::<Test>::get(), Some(1 + 60 * DAY));
        assert_eq!(Registry::current_price(), None);
        go(1 + 60 * DAY);
        assert_eq!(Registry::current_price(), Some(200_000 * UNIT));
    });
}

#[test]
fn a_freed_slot_restarts_an_auction_that_waited_for_it() {
    new_test_ext(true).execute_with(|| {
        AuctionStart::<Test>::put(1);
        assert_ok!(Registry::set_max_subnets(RuntimeOrigin::root(), 1));
        assert_ok!(reg(ALICE, u128::MAX));
        go(100 * DAY); // the spacing is over, but the cap is reached
        assert_eq!(Registry::current_price(), None);
        assert_ok!(Registry::deregister_subnet(RuntimeOrigin::root(), 1));
        assert_eq!(AuctionStart::<Test>::get(), Some(100 * DAY));
        assert_eq!(Registry::current_price(), Some(200_000 * UNIT));
    });
}

#[test]
fn only_dead_subnets_past_immunity_can_be_removed_by_anyone() {
    new_test_ext(true).execute_with(|| {
        AuctionStart::<Test>::put(1);
        assert_ok!(reg(ALICE, u128::MAX)); // created at block 1
        let rm = || Registry::remove_dead_subnet(RuntimeOrigin::signed(BOB), 1);
        // Never paid 3 miners, but still immune.
        go(IMMUNITY_BLOCKS);
        assert_noop!(rm(), Error::<Test>::NotDead);
        // An epoch paying only 2 miners does not count as alive.
        Registry::note_subnet_epoch(1, 2);
        go(1 + IMMUNITY_BLOCKS - DAY);
        Registry::note_subnet_epoch(1, 3); // healthy at 1 + immunity - 1 day
        go(1 + IMMUNITY_BLOCKS + 10 * DAY);
        assert_noop!(rm(), Error::<Test>::NotDead);
        assert!(!Registry::is_dead(SN0));
        // 30 days after the last healthy epoch: dead.
        go(1 + IMMUNITY_BLOCKS - DAY + crate::DEAD_AFTER_BLOCKS);
        assert!(Registry::is_dead(1));
        assert_ok!(rm());
        assert!(Subnets::<Test>::get(1).is_none());
        assert!(crate::LastHealthy::<Test>::get(1).is_none());
    });
}

#[test]
fn the_cap_doubles_by_itself_when_full_busy_and_spaced() {
    use crate::{FullSince, LastCapRaise, SubnetMinerHistory, CAP_LADDER_FROM};
    new_test_ext(true).execute_with(|| {
        AuctionStart::<Test>::put(1);
        assert_ok!(Registry::set_max_subnets(RuntimeOrigin::root(), 4));
        for _ in 0..4 {
            let at = AuctionStart::<Test>::get().unwrap();
            go(at + 100 * DAY); // buy at the floor
            assert_ok!(reg(ALICE, u128::MAX));
        }
        let full = FullSince::<Test>::get().unwrap();
        // Three of four subnets busy (>= 10 paid miners on average): 75 %.
        for n in 1..=3u16 {
            SubnetMinerHistory::<Test>::insert(n, BoundedVec::truncate_from(vec![12u32; 30]));
        }
        SubnetMinerHistory::<Test>::insert(4, BoundedVec::truncate_from(vec![9u32; 30]));
        let t = full.max(CAP_LADDER_FROM) + crate::CAP_FULL_BLOCKS;
        // Not before day 775.
        assert!(!Registry::try_raise_cap(CAP_LADDER_FROM - 1));
        // Governance raised it less than 180 days ago (set_max_subnets above).
        LastCapRaise::<Test>::put(t - 10);
        assert!(!Registry::try_raise_cap(t));
        LastCapRaise::<Test>::kill();
        // Only two of four busy: 50 % < 75 %.
        SubnetMinerHistory::<Test>::insert(3, BoundedVec::truncate_from(vec![2u32; 30]));
        assert!(!Registry::try_raise_cap(t));
        SubnetMinerHistory::<Test>::insert(3, BoundedVec::truncate_from(vec![12u32; 30]));
        go(t);
        assert!(Registry::try_raise_cap(t));
        assert_eq!(MaxSubnets::<Test>::get(), 8);
        assert_eq!(LastCapRaise::<Test>::get(), Some(t));
        assert_eq!(FullSince::<Test>::get(), None);
        assert!(Registry::current_price().is_some());
        // Filled again at once: still needs 90 days full and 180 days since the raise.
        for _ in 0..4 {
            let at = AuctionStart::<Test>::get().unwrap();
            go(at + 100 * DAY);
            assert_ok!(reg(ALICE, u128::MAX));
            let n = SubnetCount::<Test>::get();
            SubnetMinerHistory::<Test>::insert(n, BoundedVec::truncate_from(vec![12u32; 30]));
        }
        let full2 = FullSince::<Test>::get().unwrap();
        let t2 = (t + crate::CAP_RAISE_SPACING).max(full2 + crate::CAP_FULL_BLOCKS);
        assert!(!Registry::try_raise_cap(t2 - 1));
        assert!(Registry::try_raise_cap(t2));
        assert_eq!(MaxSubnets::<Test>::get(), 16);
    });
}

#[test]
fn governance_settings_are_bounded() {
    new_test_ext(true).execute_with(|| {
        assert_eq!(MaxSubnets::<Test>::get(), 8);
        assert_eq!(MinSpacing::<Test>::get(), 60 * DAY);
        assert_noop!(
            Registry::set_min_spacing(RuntimeOrigin::signed(ALICE), 30 * DAY),
            DispatchError::BadOrigin
        );
        assert_noop!(
            Registry::set_min_spacing(RuntimeOrigin::root(), 30 * DAY - 1),
            Error::<Test>::SpacingTooShort
        );
        assert_ok!(Registry::set_min_spacing(RuntimeOrigin::root(), 30 * DAY));
        AuctionStart::<Test>::put(1);
        assert_ok!(reg(ALICE, u128::MAX));
        assert_noop!(
            Registry::set_max_subnets(RuntimeOrigin::root(), 0),
            Error::<Test>::BadMaxSubnets
        );
        assert_noop!(
            Registry::set_max_subnets(RuntimeOrigin::root(), 257),
            Error::<Test>::BadMaxSubnets
        );
        assert_ok!(Registry::attest_swap(
            RuntimeOrigin::root(),
            [1; 20],
            [2; 32]
        ));
        assert_noop!(
            Registry::attest_swap(RuntimeOrigin::root(), [3; 20], [4; 32]),
            Error::<Test>::AlreadyAttested
        );
    });
}

#[test]
fn endpoints_only_for_registered_neurons() {
    new_test_ext(false).execute_with(|| {
        assert_noop!(
            Registry::set_endpoint(RuntimeOrigin::signed(ALICE), SN0, b"http://x".to_vec()),
            Error::<Test>::NotRegistered
        );
        assert_ok!(Registry::register_neuron(
            RuntimeOrigin::signed(ALICE),
            SN0,
            ALICE
        ));
        assert_ok!(Registry::set_endpoint(
            RuntimeOrigin::signed(ALICE),
            SN0,
            b"http://10.0.0.1:8091".to_vec()
        ));
        assert_eq!(
            crate::Endpoints::<Test>::get(SN0, ALICE)
                .unwrap()
                .into_inner(),
            b"http://10.0.0.1:8091".to_vec()
        );
        assert_noop!(
            Registry::set_endpoint(RuntimeOrigin::signed(ALICE), SN0, vec![b'a'; 200]),
            Error::<Test>::EndpointTooLong
        );
    });
}

#[test]
fn bootstrap_only_operator_registers_without_fee_then_opens() {
    new_bootstrap_ext().execute_with(|| {
        assert!(Registry::in_bootstrap());
        assert_noop!(
            Registry::register_neuron(RuntimeOrigin::signed(ALICE), SN0, ALICE),
            Error::<Test>::BootstrapRestricted
        );
        // operator has no balance at all and still registers its hotkeys
        assert_ok!(Registry::register_neuron(
            RuntimeOrigin::signed(OPERATOR),
            SN0,
            21
        ));
        assert_ok!(Registry::register_neuron(
            RuntimeOrigin::signed(OPERATOR),
            SN0,
            22
        ));
        System::set_block_number(100);
        assert!(!Registry::in_bootstrap());
        assert_ok!(Registry::register_neuron(
            RuntimeOrigin::signed(ALICE),
            SN0,
            ALICE
        ));
        assert_noop!(
            Registry::end_bootstrap(RuntimeOrigin::signed(OPERATOR)),
            Error::<Test>::NotInBootstrap
        );
    });
}

#[test]
fn bootstrap_can_end_early_but_not_restart() {
    new_bootstrap_ext().execute_with(|| {
        assert_noop!(
            Registry::end_bootstrap(RuntimeOrigin::signed(ALICE)),
            Error::<Test>::BootstrapRestricted
        );
        System::set_block_number(40);
        assert_ok!(Registry::end_bootstrap(RuntimeOrigin::signed(OPERATOR)));
        assert!(!Registry::in_bootstrap());
        assert_eq!(crate::BootstrapUntil::<Test>::get(), 40);
        assert_noop!(
            Registry::end_bootstrap(RuntimeOrigin::root()),
            Error::<Test>::NotInBootstrap
        );
    });
}

// ---------------------------------------------------------------- open model list

use crate::{
    LegacyTierInfo, ModelStatus, Models, NeuronModel, NextModelId, Tiers, ValidatorModels,
};

const HASH: [u8; 32] = [7; 32];

/// Register `who` on SN0 (ALICE pays) with `stake` SN0 stake.
fn sn0_neuron(who: u64, stake: u128) {
    assert_ok!(Registry::register_neuron(
        RuntimeOrigin::signed(ALICE),
        SN0,
        who
    ));
    set_stake(SN0, who, stake);
}

fn propose(who: u64) -> u8 {
    let id = NextModelId::<Test>::get();
    assert_ok!(Registry::propose_model(
        RuntimeOrigin::signed(who),
        b"Qwen3.5-4B-Instruct".to_vec(),
        HASH,
        b"llama.cpp b6000".to_vec(),
        3_300,
        200_000,
        350_000
    ));
    id
}

fn set(model: u8, status: ModelStatus) -> polkadot_sdk::sp_runtime::DispatchResult {
    Registry::set_model(RuntimeOrigin::root(), model, 200_000, 350_000, status, None)
}

#[test]
fn genesis_has_the_starting_list_with_only_2b_open() {
    new_test_ext(false).execute_with(|| {
        let m0 = Models::<Test>::get(0).unwrap();
        assert_eq!(m0.name.to_vec(), b"Qwen3.5-2B".to_vec());
        assert_eq!(m0.status, ModelStatus::Open);
        assert_eq!(m0.g_ppm, 100_000);
        for id in 1..6u8 {
            assert_ne!(Models::<Test>::get(id).unwrap().status, ModelStatus::Open);
        }
        assert_eq!(Models::<Test>::get(3).unwrap().status, ModelStatus::Closed);
        assert_eq!(Models::<Test>::get(5).unwrap().name.to_vec(), b"Qwen3.5-4B".to_vec());
        assert!(Models::<Test>::get(6).is_none());
        assert_eq!(NextModelId::<Test>::get(), 6);
    });
}

#[test]
fn neurons_declare_open_models_only() {
    new_test_ext(false).execute_with(|| {
        sn0_neuron(BOB, 0);
        // Undeclared neurons run model 0.
        assert_eq!(NeuronModel::<Test>::get(SN0, BOB), 0);
        assert_noop!(
            Registry::declare_model(RuntimeOrigin::signed(ALICE), SN0, 0),
            Error::<Test>::NotRegistered
        );
        assert_noop!(
            Registry::declare_model(RuntimeOrigin::signed(BOB), SN0, 1),
            Error::<Test>::ModelNotOpen
        );
        assert_noop!(
            Registry::declare_model(RuntimeOrigin::signed(BOB), SN0, 9),
            Error::<Test>::UnknownModel
        );
        assert_ok!(Registry::declare_model(RuntimeOrigin::signed(BOB), SN0, 0));
    });
}

#[test]
fn a_model_opens_with_three_holders_a_pinned_hash_and_governance() {
    new_test_ext(false).execute_with(|| {
        let id = propose(ALICE);
        assert_eq!(Balances::free_balance(Registry::account_id()), ModelDeposit::get());
        assert_eq!(Models::<Test>::get(id).unwrap().status, ModelStatus::Proposed);
        // Validators 3, 4, 5 with enough SN0 stake; 6 with too little.
        for v in [3, 4] {
            sn0_neuron(v, 1_000 * UNIT);
            assert_ok!(Registry::declare_validator_models(
                RuntimeOrigin::signed(v),
                vec![0, id]
            ));
        }
        assert_noop!(
            Registry::declare_validator_models(RuntimeOrigin::signed(9), vec![id]),
            Error::<Test>::StakeTooLow
        );
        sn0_neuron(6, 999 * UNIT);
        assert_noop!(
            Registry::declare_validator_models(RuntimeOrigin::signed(6), vec![id]),
            Error::<Test>::StakeTooLow
        );
        assert_noop!(set(id, ModelStatus::Open), Error::<Test>::TooFewHolders);
        assert_noop!(
            Registry::set_model(
                RuntimeOrigin::signed(ALICE),
                id,
                200_000,
                350_000,
                ModelStatus::Open,
                None
            ),
            DispatchError::BadOrigin
        );
        // Third holder: 6 gets enough stake.
        set_stake(SN0, 6, 1_000 * UNIT);
        assert_noop!(
            Registry::declare_validator_models(RuntimeOrigin::signed(6), vec![id, id]),
            Error::<Test>::DuplicateModel
        );
        assert_ok!(Registry::declare_validator_models(
            RuntimeOrigin::signed(6),
            vec![id]
        ));
        assert_eq!(Registry::model_holders(id), 3);
        assert_eq!(Registry::model_holders(0), 2);
        // Stake that leaves no longer counts.
        set_stake(SN0, 6, 0);
        assert_noop!(set(id, ModelStatus::Open), Error::<Test>::TooFewHolders);
        set_stake(SN0, 6, 1_000 * UNIT);
        let before = Balances::free_balance(ALICE);
        assert_ok!(set(id, ModelStatus::Open));
        assert_eq!(Balances::free_balance(ALICE), before + ModelDeposit::get());
        assert_eq!(Models::<Test>::get(id).unwrap().deposit, 0);
        // A miner may now declare it.
        sn0_neuron(BOB, 0);
        assert_ok!(Registry::declare_model(RuntimeOrigin::signed(BOB), SN0, id));
        assert_eq!(NeuronModel::<Test>::get(SN0, BOB), id);
        // Retiring: still declared by BOB, but no new declarations.
        assert_ok!(set(id, ModelStatus::Retiring));
        assert_noop!(
            Registry::declare_model(RuntimeOrigin::signed(BOB), SN0, id),
            Error::<Test>::ModelNotOpen
        );
        assert_noop!(set(id, ModelStatus::Proposed), Error::<Test>::BadStatusChange);
        assert_ok!(set(id, ModelStatus::Closed));
        assert_noop!(set(id, ModelStatus::Retiring), Error::<Test>::BadStatusChange);
    });
}

#[test]
fn starting_list_entries_open_only_with_a_pinned_hash() {
    new_test_ext(false).execute_with(|| {
        for v in [3, 4, 5] {
            sn0_neuron(v, 1_000 * UNIT);
            assert_ok!(Registry::declare_validator_models(
                RuntimeOrigin::signed(v),
                vec![5]
            ));
        }
        assert_noop!(set(5, ModelStatus::Open), Error::<Test>::NoWeightsHash);
        assert_ok!(Registry::set_model(
            RuntimeOrigin::root(),
            5,
            200_000,
            350_000,
            ModelStatus::Open,
            Some(HASH)
        ));
        assert_eq!(Models::<Test>::get(5).unwrap().weights_hash, HASH);
        assert_noop!(
            Registry::set_model(
                RuntimeOrigin::root(),
                5,
                200_000,
                350_000,
                ModelStatus::Open,
                Some([8; 32])
            ),
            Error::<Test>::HashAlreadySet
        );
        assert_noop!(
            Registry::set_model(RuntimeOrigin::root(), 5, 0, 350_000, ModelStatus::Open, None),
            Error::<Test>::BadModelParams
        );
    });
}

#[test]
fn proposals_withdraw_or_burn_their_deposit() {
    new_test_ext(false).execute_with(|| {
        let a = propose(ALICE);
        let b = propose(ALICE);
        assert_noop!(
            Registry::withdraw_model(RuntimeOrigin::signed(BOB), a),
            Error::<Test>::NotProposer
        );
        let before = Balances::free_balance(ALICE);
        assert_ok!(Registry::withdraw_model(RuntimeOrigin::signed(ALICE), a));
        assert_eq!(Balances::free_balance(ALICE), before + ModelDeposit::get());
        assert_eq!(Models::<Test>::get(a).unwrap().status, ModelStatus::Closed);
        assert_noop!(
            Registry::withdraw_model(RuntimeOrigin::signed(ALICE), a),
            Error::<Test>::NotProposer
        );
        // Governance rejects b: its deposit is burned.
        let issuance = Balances::total_issuance();
        assert_ok!(set(b, ModelStatus::Closed));
        assert_eq!(Balances::total_issuance(), issuance - ModelDeposit::get());
        assert_noop!(
            Registry::propose_model(
                RuntimeOrigin::signed(ALICE),
                b"x".to_vec(),
                [0; 32],
                b"e".to_vec(),
                1,
                1,
                0
            ),
            Error::<Test>::NoWeightsHash
        );
        // The list is bounded.
        NextModelId::<Test>::put(crate::MAX_MODELS as u8);
        assert_noop!(
            Registry::propose_model(
                RuntimeOrigin::signed(ALICE),
                b"x".to_vec(),
                HASH,
                b"e".to_vec(),
                1,
                1,
                0
            ),
            Error::<Test>::TooManyModels
        );
    });
}

#[test]
fn upgrade_turns_tiers_into_models_and_keeps_declarations() {
    use polkadot_sdk::frame_support::storage::migration::put_storage_value;
    use polkadot_sdk::frame_support::traits::{GetStorageVersion, StorageVersion};
    new_test_ext(false).execute_with(|| {
        // A v2 chain: tier table with a pinned hash on tier 0 and G 0.15, tier 1 closed,
        // and BOB declared tier 0, 5 declared tier 1.
        let _ = Models::<Test>::clear(u32::MAX, None);
        NextModelId::<Test>::kill();
        let tier = |g: u32, name: &[u8], hash, open| LegacyTierInfo {
            weight_ppm: g,
            tolerance_ppm: 350_000,
            model: BoundedVec::truncate_from(name.to_vec()),
            model_hash: hash,
            open,
        };
        Tiers::<Test>::insert(0, tier(150_000, b"Qwen3.5-2B", [1; 32], true));
        Tiers::<Test>::insert(1, tier(1_000_000, b"Qwen3.5-9B", [0; 32], false));
        Tiers::<Test>::insert(3, tier(5_000_000, b"Qwen3.5-122B", [0; 32], false));
        use codec::Encode;
        let key = |who: u64| {
            let mut k = SN0.using_encoded(polkadot_sdk::sp_io::hashing::twox_64).to_vec();
            k.extend(SN0.encode());
            k.extend(who.using_encoded(polkadot_sdk::sp_io::hashing::blake2_128));
            k.extend(who.encode());
            k
        };
        put_storage_value(b"Registry", b"NeuronTier", &key(BOB), 0u8);
        put_storage_value(b"Registry", b"NeuronTier", &key(5), 1u8);
        StorageVersion::new(2).put::<Registry>();
        Registry::on_runtime_upgrade();
        assert_eq!(Registry::on_chain_storage_version(), 3);
        assert_eq!(Tiers::<Test>::iter().count(), 0);
        let m0 = Models::<Test>::get(0).unwrap();
        assert_eq!((m0.status, m0.g_ppm, m0.weights_hash), (ModelStatus::Open, 150_000, [1; 32]));
        assert_eq!(Models::<Test>::get(1).unwrap().status, ModelStatus::Proposed);
        assert_eq!(Models::<Test>::get(3).unwrap().status, ModelStatus::Closed);
        // Entries the tier table did not have come from the starting list.
        assert_eq!(Models::<Test>::get(2).unwrap().name.to_vec(), b"Qwen3.5-27B".to_vec());
        assert_eq!(Models::<Test>::get(4).unwrap().name.to_vec(), b"Qwen3.5-0.8B".to_vec());
        assert_eq!(NextModelId::<Test>::get(), 6);
        assert_eq!(NeuronModel::<Test>::get(SN0, 5), 1);
        assert!(NeuronModel::<Test>::contains_key(SN0, BOB));
        // Runs once.
        assert_ok!(set(1, ModelStatus::Closed));
        Registry::on_runtime_upgrade();
        assert_eq!(Models::<Test>::get(1).unwrap().status, ModelStatus::Closed);
        let _ = ValidatorModels::<Test>::iter().count();
    });
}

#[test]
fn upgrade_to_v2_drops_the_doubling_price_storage() {
    use polkadot_sdk::frame_support::storage::migration::{get_storage_value, put_storage_value};
    use polkadot_sdk::frame_support::traits::{GetStorageVersion, StorageVersion};
    new_test_ext(false).execute_with(|| {
        StorageVersion::new(1).put::<Registry>();
        put_storage_value(b"Registry", b"LastRegPrice", b"", 1_000u128);
        put_storage_value(b"Registry", b"LastRegBlock", b"", 5u32);
        Registry::on_runtime_upgrade();
        assert_eq!(Registry::on_chain_storage_version(), 3);
        assert!(get_storage_value::<u128>(b"Registry", b"LastRegPrice", b"").is_none());
        assert!(get_storage_value::<u32>(b"Registry", b"LastRegBlock", b"").is_none());
        // Defaults of the new storage apply without any write.
        assert_eq!(MaxSubnets::<Test>::get(), 8);
        assert!(!Phase2Enabled::<Test>::get());
        assert_eq!(AuctionStart::<Test>::get(), None);
    });
}

// ---------------------------------------------------------------- subnet control

use crate::{
    Control, ControlKind, Params, SubnetAction, SubnetBanned, SubnetParams, DAO_VOTE_BLOCKS,
};

/// SN1 bought by ALICE at block 1.
fn sn1() {
    AuctionStart::<Test>::put(1);
    assert_ok!(reg(ALICE, u128::MAX));
}

fn params(fee: u128) -> SubnetParams {
    SubnetParams {
        neuron_fee: fee,
        max_neurons: 4,
        penalties: false,
        max_failures: 3,
        failure_window: 600,
    }
}

fn owner(n: u16) -> u64 {
    Subnets::<Test>::get(n).unwrap().owner.unwrap()
}

#[test]
fn control_only_moves_forward_key_multisig_dao() {
    new_test_ext(true).execute_with(|| {
        sn1();
        let s = |w: u64| RuntimeOrigin::signed(w);
        assert_noop!(
            Registry::transfer_ownership(s(BOB), 1, BOB),
            Error::<Test>::NotController
        );
        assert_noop!(
            Registry::transfer_ownership(s(ALICE), SN0, BOB),
            Error::<Test>::Sn0HasNoOwner
        );
        assert_ok!(Registry::transfer_ownership(s(ALICE), 1, BOB));
        assert_eq!(owner(1), BOB);

        assert_noop!(
            Registry::set_multisig(s(BOB), 1, vec![1, 2, 3], 1),
            Error::<Test>::BadSignatories
        );
        assert_noop!(
            Registry::set_multisig(s(BOB), 1, vec![3, 3], 2),
            Error::<Test>::BadSignatories
        );
        assert_ok!(Registry::set_multisig(s(BOB), 1, vec![3, 1, 2], 2));
        let ms = Registry::multi_account_id(&[1, 2, 3], 2);
        assert_eq!(owner(1), ms);
        assert_eq!(Control::<Test>::get(1), ControlKind::Multisig);
        assert_noop!(
            Registry::set_params(s(BOB), 1, params(2 * UNIT)),
            Error::<Test>::NotController
        );
        assert_ok!(Registry::set_params(s(ms), 1, params(2 * UNIT)));
        assert_noop!(
            Registry::set_multisig(s(ms), 1, vec![4, 5], 2),
            Error::<Test>::ControlCannotGoBack
        );

        assert_ok!(Registry::set_dao(s(ms), 1));
        assert_eq!(owner(1), Registry::dao_account(1));
        assert_eq!(Control::<Test>::get(1), ControlKind::Dao);
        assert_noop!(
            Registry::set_params(s(ms), 1, params(3 * UNIT)),
            Error::<Test>::DaoControlled
        );
        assert_noop!(Registry::set_dao(s(ms), 1), Error::<Test>::DaoControlled);
        assert_noop!(
            Registry::transfer_ownership(s(ms), 1, BOB),
            Error::<Test>::DaoControlled
        );
    });
}

#[test]
fn a_multisig_can_sell_the_subnet_to_one_key() {
    new_test_ext(true).execute_with(|| {
        sn1();
        assert_ok!(Registry::set_multisig(
            RuntimeOrigin::signed(ALICE),
            1,
            vec![1, 2],
            2
        ));
        let ms = Registry::multi_account_id(&[1, 2], 2);
        assert_ok!(Registry::transfer_ownership(
            RuntimeOrigin::signed(ms),
            1,
            BOB
        ));
        assert_eq!(owner(1), BOB);
        assert_eq!(Control::<Test>::get(1), ControlKind::Key);
        assert!(crate::MultisigOf::<Test>::get(1).is_none());
    });
}

#[test]
fn settings_stay_within_bounds_and_apply() {
    new_test_ext(true).execute_with(|| {
        sn1();
        let a = RuntimeOrigin::signed(ALICE);
        for bad in [
            params(UNIT - 1),
            params(1_000 * UNIT + 1),
            SubnetParams {
                max_neurons: 5,
                ..params(UNIT)
            },
            SubnetParams {
                max_failures: 0,
                ..params(UNIT)
            },
            SubnetParams {
                failure_window: 19,
                ..params(UNIT)
            },
        ] {
            assert_noop!(
                Registry::set_params(a.clone(), 1, bad),
                Error::<Test>::ParamOutOfBounds
            );
        }
        assert_ok!(Registry::set_params(a.clone(), 1, params(5 * UNIT)));
        let issuance = Balances::total_issuance();
        assert_ok!(Registry::register_neuron(RuntimeOrigin::signed(BOB), 1, 20));
        assert_eq!(Balances::total_issuance(), issuance - 5 * UNIT);
        // SN0 keeps the protocol fee.
        assert_ok!(Registry::register_neuron(
            RuntimeOrigin::signed(BOB),
            SN0,
            21
        ));
        assert_eq!(Balances::total_issuance(), issuance - 6 * UNIT);

        assert_ok!(Registry::set_metadata(
            a,
            1,
            b"New".to_vec(),
            vec![],
            [9; 32]
        ));
        let m = Metadata::<Test>::get(1).unwrap();
        assert_eq!(m.name.to_vec(), b"New".to_vec());
        assert_eq!(m.code_hash, [9; 32]);
    });
}

#[test]
fn dao_votes_by_stake_and_carries_out_the_decision() {
    new_test_ext(true).execute_with(|| {
        sn1();
        assert_ok!(Registry::set_dao(RuntimeOrigin::signed(ALICE), 1));
        set_stake(1, 10, 600);
        set_stake(1, 11, 300);
        set_stake(1, 12, 100);
        let s = |w: u64| RuntimeOrigin::signed(w);
        let action = SubnetAction::SetParams(params(7 * UNIT));
        assert_noop!(
            Registry::propose(s(BOB), 1, action.clone()),
            Error::<Test>::NotStaker
        );
        assert_noop!(
            Registry::propose(s(10), 1, SubnetAction::SetParams(params(0))),
            Error::<Test>::ParamOutOfBounds
        );
        assert_ok!(Registry::propose(s(10), 1, action.clone()));
        assert_noop!(
            Registry::propose(s(11), 1, action.clone()),
            Error::<Test>::ProposalOpen
        );
        assert_ok!(Registry::vote(s(11), 1, false));
        assert_noop!(
            Registry::close_proposal(s(BOB), 1),
            Error::<Test>::VoteStillOpen
        );
        go(1 + DAO_VOTE_BLOCKS);
        assert_noop!(Registry::vote(s(12), 1, true), Error::<Test>::VoteClosed);
        assert_ok!(Registry::close_proposal(s(BOB), 1));
        assert_eq!(Params::<Test>::get(1).unwrap().neuron_fee, 7 * UNIT);

        // Spending the DAO account: 100 of 1,000 aye is below the 1/3 quorum.
        let dao = Registry::dao_account(1);
        assert_ok!(Balances::force_set_balance(
            RuntimeOrigin::root(),
            dao,
            1_000 * UNIT
        ));
        let spend = SubnetAction::Spend {
            to: 50,
            amount: 40 * UNIT,
        };
        let t = 1 + DAO_VOTE_BLOCKS;
        assert_ok!(Registry::propose(s(12), 1, spend.clone()));
        go(t + DAO_VOTE_BLOCKS);
        assert_ok!(Registry::close_proposal(s(BOB), 1));
        assert_eq!(Balances::free_balance(50), 0);

        assert_ok!(Registry::propose(s(12), 1, spend));
        assert_ok!(Registry::vote(s(10), 1, true));
        go(t + 2 * DAO_VOTE_BLOCKS);
        assert_ok!(Registry::close_proposal(s(BOB), 1));
        assert_eq!(Balances::free_balance(50), 40 * UNIT);
    });
}

#[test]
fn subnet_penalties_need_half_of_active_stake_and_ban_repeat_failures() {
    new_test_ext(true).execute_with(|| {
        sn1();
        assert_ok!(Registry::register_neuron(RuntimeOrigin::signed(BOB), 1, 20));
        set_stake(1, 10, 500);
        set_stake(1, 11, 500); // exactly half: enough
        let s = |w: u64| RuntimeOrigin::signed(w);
        assert_noop!(
            Registry::report_failed_work(s(11), 1, 5, 0, [0; 32]),
            Error::<Test>::PenaltiesOff
        );
        assert_ok!(Registry::set_params(
            s(ALICE),
            1,
            SubnetParams {
                penalties: true,
                max_failures: 2,
                ..params(UNIT)
            }
        ));
        assert_noop!(
            Registry::report_failed_work(s(BOB), 1, 5, 0, [0; 32]),
            Error::<Test>::NotStaker
        );
        assert_noop!(
            Registry::report_failed_work(s(11), SN0, 5, 0, [0; 32]),
            Error::<Test>::Sn0HasNoOwner
        );
        assert_ok!(Registry::report_failed_work(s(11), 1, 5, 0, [1; 32]));
        assert!(Registry::is_work_penalized(1, 5, 0));
        assert!(!Registry::is_work_penalized(1, 6, 0));
        assert_noop!(
            Registry::report_failed_work(s(11), 1, 5, 0, [1; 32]),
            Error::<Test>::AlreadyReported
        );
        assert!(!Registry::is_banned_from(1, &20));
        assert_ok!(Registry::report_failed_work(s(11), 1, 6, 0, [1; 32]));
        assert!(Registry::is_banned_from(1, &20));

        // Closing the subnet clears its control state.
        go(1 + IMMUNITY_BLOCKS);
        assert_ok!(Registry::deregister_subnet(RuntimeOrigin::root(), 1));
        assert!(Params::<Test>::get(1).is_none());
        assert!(!Registry::is_work_penalized(1, 5, 0));
        assert!(SubnetBanned::<Test>::get(1, 20).is_none());
    });
}

#[test]
fn a_small_reporter_alone_does_not_penalise() {
    new_test_ext(true).execute_with(|| {
        sn1();
        assert_ok!(Registry::register_neuron(RuntimeOrigin::signed(BOB), 1, 20));
        assert_ok!(Registry::set_params(
            RuntimeOrigin::signed(ALICE),
            1,
            SubnetParams {
                penalties: true,
                ..params(UNIT)
            }
        ));
        set_stake(1, 10, 501);
        set_stake(1, 11, 499);
        assert_ok!(Registry::report_failed_work(
            RuntimeOrigin::signed(11),
            1,
            5,
            0,
            [0; 32]
        ));
        assert!(!Registry::is_work_penalized(1, 5, 0)); // 499 of 1,000 < half
    });
}

#[test]
fn with_the_alpha_module_half_the_sale_goes_to_the_pool_and_half_is_burned() {
    new_test_ext(true).execute_with(|| {
        PoolsOn::set(true);
        AuctionStart::<Test>::put(1);
        let (alice, treasury, issuance) =
            (Balances::free_balance(ALICE), Balances::free_balance(TREASURY), Balances::total_issuance());
        assert_ok!(reg(ALICE, u128::MAX));
        assert_eq!(Balances::free_balance(ALICE), alice - 50_000 * UNIT);
        assert_eq!(Balances::free_balance(POOLS), 25_000 * UNIT);
        assert_eq!(Balances::free_balance(TREASURY), treasury);
        assert_eq!(Balances::total_issuance(), issuance - 25_000 * UNIT);
        PoolsOn::set(false);
    });
}

#[test]
fn the_unlock_status_shows_the_alpha_criterion() {
    new_test_ext(false).execute_with(|| {
        AlphaInstalled::set(false);
        assert!(!Registry::unlock_status().alpha_installed);
        AlphaInstalled::set(true);
        assert!(Registry::unlock_status().alpha_installed);
    });
}
