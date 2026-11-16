use crate::{mock::*, Error, LastSettled, Stake};
use polkadot_sdk::frame_support::{assert_noop, assert_ok, traits::Hooks};
use polkadot_sdk::sp_core::H256;

const SALT: [u8; 32] = [1u8; 32];

fn go_to(block: u64) {
    while System::block_number() < block {
        let b = System::block_number() + 1;
        System::set_block_number(b);
        Consensus::on_initialize(b);
    }
}

/// validators 1 and 2 (uids 0, 1); miners 3 and 4 (uids 2, 3); pot funded with 90,000.
fn setup() {
    for a in 1..=4u64 {
        assert_ok!(Registry::register_neuron(RuntimeOrigin::signed(a), 0, a));
    }
    assert_ok!(Consensus::stake(RuntimeOrigin::signed(1), 10_000));
    assert_ok!(Consensus::stake(RuntimeOrigin::signed(2), 10_000));
    assert_ok!(Balances::force_set_balance(
        RuntimeOrigin::root(),
        POT,
        90_000
    ));
}

fn commit(v: u64, uids: Vec<u16>, w: Vec<u16>) {
    let h: H256 = Weights::commit_hash(0, &uids, &w, &SALT);
    assert_ok!(Weights::commit_weights(RuntimeOrigin::signed(v), 0, h));
}

fn reveal(v: u64, uids: Vec<u16>, w: Vec<u16>) {
    assert_ok!(Weights::reveal_weights(
        RuntimeOrigin::signed(v),
        0,
        uids,
        w,
        SALT
    ));
}

#[test]
fn only_sn0_neurons_stake_with_minimum() {
    new_test_ext().execute_with(|| {
        assert_noop!(
            Consensus::stake(RuntimeOrigin::signed(5), 1_000),
            Error::<Test>::NotSn0Neuron
        );
        assert_ok!(Registry::register_neuron(RuntimeOrigin::signed(5), 0, 5));
        assert_noop!(
            Consensus::stake(RuntimeOrigin::signed(5), 10),
            Error::<Test>::BelowMinimumStake
        );
        assert_ok!(Consensus::stake(RuntimeOrigin::signed(5), 1_000));
        assert_eq!(Stake::<Test>::get(5), 1_000);
    });
}

#[test]
fn full_epoch_cycle_pays_miners_and_validators() {
    new_test_ext().execute_with(|| {
        setup();
        // epoch 0: both validators score miner 3 at 3x miner 4
        commit(1, vec![2, 3], vec![3, 1]);
        commit(2, vec![2, 3], vec![3, 1]);
        go_to(10); // epoch 1: reveal
        reveal(1, vec![2, 3], vec![3, 1]);
        reveal(2, vec![2, 3], vec![3, 1]);
        let (m3, m4, v1) = (
            Balances::free_balance(3),
            Balances::free_balance(4),
            Balances::free_balance(1),
        );
        go_to(20); // epoch 2 starts: settle epoch 0
        assert_eq!(LastSettled::<Test>::get(), Some(0));
        // miners get 2/3 of 90,000 = 60,000 split 3:1
        assert_eq!(Balances::free_balance(3) - m3, 45_000);
        assert_eq!(Balances::free_balance(4) - m4, 15_000);
        // validators split the remaining 30,000 equally
        assert_eq!(Balances::free_balance(1) - v1, 15_000);
        assert_eq!(Balances::free_balance(POT), 0);
    });
}

#[test]
fn penalised_miner_gets_nothing_and_pot_carries_over_without_weights() {
    new_test_ext().execute_with(|| {
        setup();
        commit(1, vec![2, 3], vec![1, 1]);
        commit(2, vec![2, 3], vec![1, 1]);
        // both validators report miner uid 3 for epoch 0 -> penalised
        assert_ok!(Work::report_failed_check(
            RuntimeOrigin::signed(1),
            0,
            3,
            H256::zero()
        ));
        go_to(10);
        reveal(1, vec![2, 3], vec![1, 1]);
        reveal(2, vec![2, 3], vec![1, 1]);
        let m4 = Balances::free_balance(4);
        go_to(20);
        assert_eq!(Balances::free_balance(4), m4); // penalised: no reward
        assert_eq!(Balances::free_balance(3), 1_000_000 - 1 + 60_000);
        // epoch 1 had no weights: pot refilled is kept
        assert_ok!(Balances::force_set_balance(
            RuntimeOrigin::root(),
            POT,
            9_000
        ));
        go_to(30);
        assert_eq!(Balances::free_balance(POT), 9_000);
    });
}

#[test]
fn unstake_waits_and_reduces_total() {
    new_test_ext().execute_with(|| {
        setup();
        assert_ok!(Consensus::unstake(RuntimeOrigin::signed(1), 4_000));
        assert_eq!(crate::TotalStake::<Test>::get(), 16_000);
        assert_noop!(
            Consensus::withdraw_unstaked(RuntimeOrigin::signed(1)),
            Error::<Test>::NotYetUnbonded
        );
        go_to(30);
        assert_ok!(Consensus::withdraw_unstaked(RuntimeOrigin::signed(1)));
    });
}

#[test]
fn launch_rewards_locked_then_released_ten_percent_per_step() {
    new_launch_ext().execute_with(|| {
        for a in 1..=3u64 {
            assert_ok!(Registry::register_neuron(
                RuntimeOrigin::signed(OPERATOR),
                0,
                a
            ));
        }
        assert_ok!(Consensus::stake(RuntimeOrigin::signed(1), 10)); // below minimum: allowed
        assert_ok!(Balances::force_set_balance(
            RuntimeOrigin::root(),
            POT,
            90_000
        ));
        commit(1, vec![1, 2], vec![1, 1]);
        go_to(10);
        reveal(1, vec![1, 2], vec![1, 1]);
        let m2 = Balances::free_balance(2);
        go_to(20); // settle epoch 0 (launch work): paid into the vault
        assert_eq!(Balances::free_balance(2), m2);
        assert_eq!(crate::LaunchLocked::<Test>::get(2), 30_000);
        assert_eq!(crate::LaunchLocked::<Test>::get(1), 30_000);
        assert_eq!(Balances::free_balance(Consensus::launch_vault()), 90_000);
        // nothing during the period, the 20-block cliff, or the first step
        assert_noop!(
            Consensus::claim_launch_rewards(RuntimeOrigin::signed(2)),
            Error::<Test>::NothingUnlocked
        );
        go_to(25 + 39);
        assert_noop!(
            Consensus::claim_launch_rewards(RuntimeOrigin::signed(2)),
            Error::<Test>::NothingUnlocked
        );
        go_to(25 + 40); // cliff + one step: 10 %
        assert_ok!(Consensus::claim_launch_rewards(RuntimeOrigin::signed(2)));
        assert_eq!(Balances::free_balance(2), m2 + 3_000);
        go_to(25 + 220); // cliff + 10 steps: everything
        assert_ok!(Consensus::claim_launch_rewards(RuntimeOrigin::signed(2)));
        assert_eq!(Balances::free_balance(2), m2 + 30_000);
        assert_noop!(
            Consensus::claim_launch_rewards(RuntimeOrigin::signed(2)),
            Error::<Test>::NothingUnlocked
        );
    });
}
