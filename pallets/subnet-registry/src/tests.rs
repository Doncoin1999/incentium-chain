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
}

#[test]
fn unlock_needs_all_four_criteria() {
    for skip in 1..=5u8 {
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
fn deregistration_after_immunity_refunds_nothing_and_reopens_the_slot() {
    new_test_ext(true).execute_with(|| {
        AuctionStart::<Test>::put(1);
        assert_ok!(Registry::set_max_subnets(RuntimeOrigin::root(), 1));
        assert_ok!(reg(ALICE, u128::MAX));
        assert_ok!(Registry::register_neuron(RuntimeOrigin::signed(BOB), 1, 20));
        let created = 1u32;
        assert_noop!(
            Registry::deregister_subnet(RuntimeOrigin::root(), SN0),
            Error::<Test>::CannotDeregisterSn0
        );
        go(created + IMMUNITY_BLOCKS - 1);
        assert_noop!(
            Registry::deregister_subnet(RuntimeOrigin::root(), 1),
            Error::<Test>::StillImmune
        );
        let alice = Balances::free_balance(ALICE);
        let issuance = Balances::total_issuance();
        go(created + IMMUNITY_BLOCKS);
        assert_noop!(
            Registry::deregister_subnet(RuntimeOrigin::signed(ALICE), 1),
            DispatchError::BadOrigin
        );
        assert_ok!(Registry::deregister_subnet(RuntimeOrigin::root(), 1));
        assert_eq!(Balances::free_balance(ALICE), alice);
        assert_eq!(Balances::total_issuance(), issuance);
        assert!(Subnets::<Test>::get(1).is_none());
        assert!(Metadata::<Test>::get(1).is_none());
        assert_eq!(NeuronCount::<Test>::get(1), 0);
        assert_eq!(SubnetCount::<Test>::get(), 0);
        // The auction that waited for a free slot starts now, at 4 x 50,000.
        assert_eq!(AuctionStart::<Test>::get(), Some(created + IMMUNITY_BLOCKS));
        assert_eq!(Registry::current_price(), Some(200_000 * UNIT));
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

#[test]
fn genesis_has_default_tiers_with_only_tier_0_open() {
    new_test_ext(false).execute_with(|| {
        let t0 = crate::Tiers::<Test>::get(0).unwrap();
        assert!(t0.open);
        assert_eq!(t0.weight_ppm, 150_000);
        assert_eq!(t0.model.to_vec(), b"Qwen3.5-2B".to_vec());
        for tier in 1..=3u8 {
            assert!(!crate::Tiers::<Test>::get(tier).unwrap().open);
        }
        assert_eq!(crate::Tiers::<Test>::get(1).unwrap().weight_ppm, 1_000_000);
        assert!(crate::Tiers::<Test>::get(4).is_none());
    });
}

#[test]
fn neurons_declare_open_tiers_only() {
    new_test_ext(false).execute_with(|| {
        assert_ok!(Registry::register_neuron(
            RuntimeOrigin::signed(ALICE),
            SN0,
            BOB
        ));
        // Undeclared neurons count as tier 0.
        assert_eq!(crate::NeuronTier::<Test>::get(SN0, BOB), 0);
        assert_noop!(
            Registry::declare_tier(RuntimeOrigin::signed(ALICE), SN0, 0),
            Error::<Test>::NotRegistered
        );
        assert_noop!(
            Registry::declare_tier(RuntimeOrigin::signed(BOB), SN0, 1),
            Error::<Test>::TierClosed
        );
        assert_noop!(
            Registry::declare_tier(RuntimeOrigin::signed(BOB), SN0, 9),
            Error::<Test>::UnknownTier
        );
        // Governance opens tier 1 (day 90) and locks its model hash.
        assert_noop!(
            Registry::set_tier(
                RuntimeOrigin::signed(ALICE),
                1,
                1_000_000,
                350_000,
                b"Qwen3.5-9B".to_vec(),
                [7; 32],
                true
            ),
            polkadot_sdk::sp_runtime::DispatchError::BadOrigin
        );
        assert_noop!(
            Registry::set_tier(RuntimeOrigin::root(), 1, 0, 350_000, vec![], [0; 32], true),
            Error::<Test>::BadTierParams
        );
        assert_ok!(Registry::set_tier(
            RuntimeOrigin::root(),
            1,
            1_000_000,
            350_000,
            b"Qwen3.5-9B".to_vec(),
            [7; 32],
            true
        ));
        assert_eq!(crate::Tiers::<Test>::get(1).unwrap().model_hash, [7; 32]);
        assert_ok!(Registry::declare_tier(RuntimeOrigin::signed(BOB), SN0, 1));
        assert_eq!(crate::NeuronTier::<Test>::get(SN0, BOB), 1);
    });
}

#[test]
fn upgrade_writes_default_tiers_once() {
    use polkadot_sdk::frame_support::traits::{GetStorageVersion, Hooks, StorageVersion};
    new_test_ext(false).execute_with(|| {
        // Simulate a chain started before tiers existed.
        let _ = crate::Tiers::<Test>::clear(u32::MAX, None);
        StorageVersion::new(0).put::<Registry>();
        Registry::on_runtime_upgrade();
        assert_eq!(Registry::on_chain_storage_version(), 2);
        assert!(crate::Tiers::<Test>::get(0).unwrap().open);
        // A later governance change survives a second upgrade.
        assert_ok!(Registry::set_tier(
            RuntimeOrigin::root(),
            0,
            200_000,
            350_000,
            b"Qwen3.5-2B".to_vec(),
            [1; 32],
            true
        ));
        Registry::on_runtime_upgrade();
        assert_eq!(crate::Tiers::<Test>::get(0).unwrap().weight_ppm, 200_000);
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
        assert_eq!(Registry::on_chain_storage_version(), 2);
        assert!(get_storage_value::<u128>(b"Registry", b"LastRegPrice", b"").is_none());
        assert!(get_storage_value::<u32>(b"Registry", b"LastRegBlock", b"").is_none());
        // Defaults of the new storage apply without any write.
        assert_eq!(MaxSubnets::<Test>::get(), 8);
        assert!(!Phase2Enabled::<Test>::get());
        assert_eq!(AuctionStart::<Test>::get(), None);
    });
}
