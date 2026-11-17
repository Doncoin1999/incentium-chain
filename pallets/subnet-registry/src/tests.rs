use crate::{mock::*, Error, NeuronCount, Neurons, SubnetCount, Subnets, SN0};
use ict_economics::UNIT;
use polkadot_sdk::frame_support::{assert_noop, assert_ok};
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

#[test]
fn subnets_only_in_phase2() {
    new_test_ext(false).execute_with(|| {
        assert_noop!(
            Registry::register_subnet(RuntimeOrigin::signed(ALICE), 10_000 * UNIT),
            Error::<Test>::Phase2NotEnabled
        );
        assert_noop!(
            Registry::enable_phase2(RuntimeOrigin::signed(ALICE)),
            DispatchError::BadOrigin
        );
        assert_ok!(Registry::enable_phase2(RuntimeOrigin::root()));
        assert_ok!(Registry::register_subnet(
            RuntimeOrigin::signed(ALICE),
            10_000 * UNIT
        ));
    });
}

#[test]
fn deposit_doubles_then_decays() {
    new_test_ext(true).execute_with(|| {
        assert_eq!(Registry::current_deposit_price(), 1_000 * UNIT);
        assert_ok!(Registry::register_subnet(
            RuntimeOrigin::signed(ALICE),
            1_000 * UNIT
        ));
        assert_eq!(Registry::current_deposit_price(), 2_000 * UNIT);
        assert_noop!(
            Registry::register_subnet(RuntimeOrigin::signed(BOB), 1_500 * UNIT),
            Error::<Test>::DepositAboveLimit
        );
        System::set_block_number((1 + 7 * DAY) as u64);
        assert_eq!(Registry::current_deposit_price(), 1_000 * UNIT); // 2,000 x 1/2 = floor
        System::set_block_number((1 + 14 * DAY) as u64);
        assert_eq!(Registry::current_deposit_price(), 1_000 * UNIT);
    });
}

#[test]
fn deregistration_returns_ninety_percent_and_burns_ten() {
    new_test_ext(true).execute_with(|| {
        let start = Balances::free_balance(ALICE);
        assert_ok!(Registry::register_subnet(
            RuntimeOrigin::signed(ALICE),
            1_000 * UNIT
        ));
        assert_ok!(Registry::register_neuron(RuntimeOrigin::signed(BOB), 1, 20));
        let issuance = Balances::total_issuance();
        assert_noop!(
            Registry::deregister_subnet(RuntimeOrigin::root(), SN0),
            Error::<Test>::CannotDeregisterSn0
        );
        assert_ok!(Registry::deregister_subnet(RuntimeOrigin::root(), 1));
        assert_eq!(Balances::free_balance(ALICE), start - 100 * UNIT);
        assert_eq!(Balances::total_issuance(), issuance - 100 * UNIT);
        assert!(Subnets::<Test>::get(1).is_none());
        assert_eq!(NeuronCount::<Test>::get(1), 0);
        assert_eq!(SubnetCount::<Test>::get(), 0);
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
        assert_eq!(Registry::on_chain_storage_version(), 1);
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
