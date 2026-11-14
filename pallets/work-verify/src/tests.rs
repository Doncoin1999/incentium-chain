use crate::{mock::*, Banned, Bonds, Error, Penalized, Pools};
use polkadot_sdk::frame_support::{assert_noop, assert_ok};

fn reg(a: u64) {
    assert_ok!(Registry::register_neuron(RuntimeOrigin::signed(a), 0, a));
}

#[test]
fn pool_needs_sn0_neuron_and_bond() {
    new_test_ext().execute_with(|| {
        assert_noop!(
            Work::register_pool(RuntimeOrigin::signed(1), 10_000),
            Error::<Test>::NotSn0Neuron
        );
        reg(1);
        assert_noop!(
            Work::register_pool(RuntimeOrigin::signed(1), 10_000),
            Error::<Test>::BondTooLow
        );
        assert_ok!(Work::bond(RuntimeOrigin::signed(1), 5_000));
        assert_noop!(
            Work::register_pool(RuntimeOrigin::signed(1), 60_000),
            Error::<Test>::FeeTooHigh
        );
        assert_ok!(Work::register_pool(RuntimeOrigin::signed(1), 20_000));
        assert!(Pools::<Test>::get(1).is_some());
    });
}

#[test]
fn pool_credit_and_pro_rata_distribution() {
    new_test_ext().execute_with(|| {
        reg(1);
        assert_ok!(Work::bond(RuntimeOrigin::signed(1), 5_000));
        assert_ok!(Work::register_pool(RuntimeOrigin::signed(1), 20_000)); // 2 %
                                                                           // consensus pays 10,000 from account 9 to the pool hotkey
        assert_ok!(Work::credit(&9, &1, 10_000));
        assert_eq!(Pools::<Test>::get(1).unwrap().balance, 10_000);
        let before = (
            Balances::free_balance(5),
            Balances::free_balance(6),
            Balances::free_balance(1),
        );
        assert_ok!(Work::distribute(
            RuntimeOrigin::signed(1),
            0,
            Default::default(),
            vec![(5, 3), (6, 1)]
        ));
        assert_eq!(Balances::free_balance(5) - before.0, 7_350); // 9,800 x 3/4
        assert_eq!(Balances::free_balance(6) - before.1, 2_450);
        assert_eq!(Balances::free_balance(1) - before.2, 200); // fee
        assert_eq!(Pools::<Test>::get(1).unwrap().balance, 0);
    });
}

#[test]
fn non_pool_credit_goes_to_hotkey() {
    new_test_ext().execute_with(|| {
        let b = Balances::free_balance(4);
        assert_ok!(Work::credit(&9, &4, 1_000));
        assert_eq!(Balances::free_balance(4), b + 1_000);
    });
}

#[test]
fn quorum_of_reports_slashes_and_three_strikes_ban() {
    new_test_ext().execute_with(|| {
        reg(1);
        reg(2); // miner uid 1
        assert_ok!(Work::bond(RuntimeOrigin::signed(2), 10_000));
        set_stake(7, 100);
        set_stake(8, 100);
        set_stake(10, 100);
        assert_noop!(
            Work::report_failed_check(RuntimeOrigin::signed(3), 0, 1, Default::default()),
            Error::<Test>::NotAValidator
        );
        let issuance = Balances::total_issuance();
        // 1/3 of stake is not yet above the 333,334 ppm quorum
        assert_ok!(Work::report_failed_check(
            RuntimeOrigin::signed(7),
            0,
            1,
            Default::default()
        ));
        assert!(!Penalized::<Test>::get(0, 1));
        assert_noop!(
            Work::report_failed_check(RuntimeOrigin::signed(7), 0, 1, Default::default()),
            Error::<Test>::AlreadyReported
        );
        assert_ok!(Work::report_failed_check(
            RuntimeOrigin::signed(8),
            0,
            1,
            Default::default()
        ));
        assert!(Penalized::<Test>::get(0, 1));
        assert_eq!(Bonds::<Test>::get(2), 9_000);
        assert_eq!(Balances::total_issuance(), issuance - 500);
        assert_eq!(Balances::free_balance(InsuranceFund::get()), 500);
        for e in 1..=2u64 {
            assert_ok!(Work::report_failed_check(
                RuntimeOrigin::signed(7),
                e,
                1,
                Default::default()
            ));
            assert_ok!(Work::report_failed_check(
                RuntimeOrigin::signed(8),
                e,
                1,
                Default::default()
            ));
        }
        assert!(Banned::<Test>::contains_key(2));
    });
}

#[test]
fn unbonding_waits() {
    new_test_ext().execute_with(|| {
        assert_ok!(Work::bond(RuntimeOrigin::signed(3), 1_000));
        assert_ok!(Work::unbond(RuntimeOrigin::signed(3), 400));
        assert_noop!(
            Work::withdraw_unbonded(RuntimeOrigin::signed(3)),
            Error::<Test>::NotYetUnbonded
        );
        System::set_block_number(31); // epoch 3
        let b = Balances::free_balance(3);
        assert_ok!(Work::withdraw_unbonded(RuntimeOrigin::signed(3)));
        assert_eq!(Balances::free_balance(3), b + 400);
        assert_eq!(Bonds::<Test>::get(3), 600);
    });
}
