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
fn launch_rewards_quarter_paid_rest_locked_then_released_ten_percent_per_step() {
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
        go_to(20); // settle epoch 0 (launch work): 25 % paid, 75 % into the vault
        assert_eq!(Balances::free_balance(2), m2 + 7_500);
        assert_eq!(crate::LaunchLocked::<Test>::get(2), 22_500);
        assert_eq!(crate::LaunchLocked::<Test>::get(1), 22_500);
        assert_eq!(Balances::free_balance(Consensus::launch_vault()), 67_500);
        let m2 = m2 + 7_500;
        // nothing during the period or the 20-block cliff
        assert_noop!(
            Consensus::claim_launch_rewards(RuntimeOrigin::signed(2)),
            Error::<Test>::NothingUnlocked
        );
        go_to(25 + 19);
        assert_noop!(
            Consensus::claim_launch_rewards(RuntimeOrigin::signed(2)),
            Error::<Test>::NothingUnlocked
        );
        go_to(25 + 20); // end of the cliff: first 10 % of the locked part
        assert_ok!(Consensus::claim_launch_rewards(RuntimeOrigin::signed(2)));
        assert_eq!(Balances::free_balance(2), m2 + 2_250);
        go_to(25 + 39);
        assert_noop!(
            Consensus::claim_launch_rewards(RuntimeOrigin::signed(2)),
            Error::<Test>::NothingUnlocked
        );
        go_to(25 + 200); // cliff + 9 steps: everything
        assert_ok!(Consensus::claim_launch_rewards(RuntimeOrigin::signed(2)));
        assert_eq!(Balances::free_balance(2), m2 + 22_500);
        assert_noop!(
            Consensus::claim_launch_rewards(RuntimeOrigin::signed(2)),
            Error::<Test>::NothingUnlocked
        );
    });
}

// ---------------------------------------------------------------- after the SN1 unlock

use crate::{SubnetStake, SubnetTotalStake, Unstaking, EMISSION_RAMP_BLOCKS};
use pallet_subnet_registry::{Phase2Enabled, SubnetCount, SubnetInfo, Subnets, UnlockedAt};

const OWNER: u64 = 9;

fn unlock_at(block: u32) {
    Phase2Enabled::<Test>::put(true);
    UnlockedAt::<Test>::put(block);
}

/// SN`netuid` owned by OWNER, created at block 0.
fn add_subnet(netuid: u16) {
    Subnets::<Test>::insert(
        netuid,
        SubnetInfo {
            owner: Some(OWNER),
            deposit: 0,
            created: 0,
        },
    );
    SubnetCount::<Test>::mutate(|c| *c += 1);
}

fn commit_on(net: u16, v: u64, uids: Vec<u16>, w: Vec<u16>) {
    let h: H256 = Weights::commit_hash(net, &uids, &w, &SALT);
    assert_ok!(Weights::commit_weights(RuntimeOrigin::signed(v), net, h));
}

fn reveal_on(net: u16, v: u64, uids: Vec<u16>, w: Vec<u16>) {
    assert_ok!(Weights::reveal_weights(
        RuntimeOrigin::signed(v),
        net,
        uids,
        w,
        SALT
    ));
}

fn close(a: u128, b: u128) -> bool {
    a.abs_diff(b) <= 2
}

#[test]
fn before_the_unlock_sn0_gets_the_whole_pot() {
    new_test_ext().execute_with(|| {
        add_subnet(1);
        SubnetTotalStake::<Test>::insert(1, 5_000);
        assert_eq!(Consensus::others_share_ppm(), 0);
        assert_eq!(Consensus::allocation(90_000), vec![(0, 90_000)]);
    });
}

#[test]
fn other_subnets_ramp_in_over_180_days_by_stake_and_sn0_keeps_half_of_issuance() {
    new_test_ext().execute_with(|| {
        unlock_at(1);
        add_subnet(1);
        add_subnet(2);
        add_subnet(3); // no stake: no slice
        SubnetTotalStake::<Test>::insert(1, 3_000);
        SubnetTotalStake::<Test>::insert(2, 1_000);
        let pot = 900_000u128; // 90 % of 1,000,000 issued; the fund took 100,000
        assert_eq!(Consensus::allocation(pot), vec![(0, pot)]); // day 0

        System::set_block_number(1 + EMISSION_RAMP_BLOCKS as u64 / 2);
        let half = Consensus::allocation(pot);
        assert_eq!(half.iter().map(|(_, x)| x).sum::<u128>(), pot);
        assert_eq!(half.len(), 3);
        // Half of the 444,444 ppm maximum.
        let others = half[1].1 + half[2].1;
        assert!(close(others, 199_999), "{half:?}");
        let of = |n: u16| half.iter().find(|(k, _)| *k == n).map(|x| x.1).unwrap();
        assert!(close(of(1), 3 * of(2)));

        for days in [180u64, 400, 2_000] {
            System::set_block_number(1 + days * 7_200);
            let full = Consensus::allocation(pot);
            assert_eq!(full[0].0, 0);
            // SN0 keeps at least 500,000 of the 1,000,000 issued.
            assert!(full[0].1 >= 500_000, "day {days}: {}", full[0].1);
            assert!(close(full[0].1, 500_000));
        }
    });
}

#[test]
fn subnet_stake_rules() {
    new_test_ext().execute_with(|| {
        add_subnet(1);
        assert_noop!(
            Consensus::stake_subnet(RuntimeOrigin::signed(5), 0, 1_000),
            Error::<Test>::UseSn0Calls
        );
        assert_noop!(
            Consensus::stake_subnet(RuntimeOrigin::signed(5), 1, 1_000),
            Error::<Test>::NotSubnetNeuron
        );
        assert_ok!(Registry::register_neuron(RuntimeOrigin::signed(5), 1, 5));
        assert_noop!(
            Consensus::stake_subnet(RuntimeOrigin::signed(5), 1, 10),
            Error::<Test>::BelowMinimumStake
        );
        assert_ok!(Consensus::stake_subnet(RuntimeOrigin::signed(5), 1, 1_000));
        assert_eq!(SubnetStake::<Test>::get(1, 5), 1_000);
        assert_eq!(SubnetTotalStake::<Test>::get(1), 1_000);
        // SN0 stake is separate.
        assert_eq!(Stake::<Test>::get(5), 0);
        assert_ok!(Consensus::unstake_subnet(RuntimeOrigin::signed(5), 1, 400));
        assert_eq!(SubnetTotalStake::<Test>::get(1), 600);
        assert_eq!(Unstaking::<Test>::get(5).map(|u| u.0), Some(400));
        assert_noop!(
            Consensus::unstake_subnet(RuntimeOrigin::signed(5), 1, 601),
            Error::<Test>::InsufficientStake
        );
    });
}

#[test]
fn subnet_pays_owner_12_miners_50_validators_38_percent() {
    new_test_ext().execute_with(|| {
        add_subnet(1);
        // validator 5 (uid 0), miners 6 and 7 (uids 1, 2) on SN1
        for a in 5..=7u64 {
            assert_ok!(Registry::register_neuron(RuntimeOrigin::signed(a), 1, a));
        }
        assert_ok!(Consensus::stake_subnet(RuntimeOrigin::signed(5), 1, 10_000));
        unlock_at(0);
        // Well past the ramp: SN1 gets the full 444,444 ppm of the pot.
        let b = 200 * 7_200u64;
        System::set_block_number(b);
        let epoch = Weights::current_epoch();
        commit_on(1, 5, vec![1, 2], vec![3, 1]);
        System::set_block_number(b + 10);
        reveal_on(1, 5, vec![1, 2], vec![3, 1]);
        assert_ok!(Balances::force_set_balance(
            RuntimeOrigin::root(),
            POT,
            900_000
        ));
        let (o, m6, m7, v5) = (
            Balances::free_balance(OWNER),
            Balances::free_balance(6),
            Balances::free_balance(7),
            Balances::free_balance(5),
        );
        Consensus::settle(epoch);
        let slice = 900_000u128 * 444_444 / 1_000_000; // 399,999
        let got = |a: u64, before: u128| Balances::free_balance(a) - before;
        assert_eq!(got(OWNER, o), slice * 120_000 / 1_000_000);
        let miners = got(6, m6) + got(7, m7);
        assert!(close(miners, slice / 2), "miners {miners}");
        assert!(close(got(6, m6), 3 * got(7, m7)));
        assert!(close(got(5, v5), slice * 380_000 / 1_000_000), "validator");
        // SN0 had no weights: its slice stays in the pot.
        assert!(close(Balances::free_balance(POT), 900_000 - slice));
    });
}

#[test]
fn sn0_splits_50_to_38_after_the_unlock() {
    new_test_ext().execute_with(|| {
        setup();
        unlock_at(1);
        commit(1, vec![2, 3], vec![3, 1]);
        commit(2, vec![2, 3], vec![3, 1]);
        go_to(10);
        reveal(1, vec![2, 3], vec![3, 1]);
        reveal(2, vec![2, 3], vec![3, 1]);
        let (m3, m4, v1, v2) = (
            Balances::free_balance(3),
            Balances::free_balance(4),
            Balances::free_balance(1),
            Balances::free_balance(2),
        );
        go_to(20);
        let miners = Balances::free_balance(3) - m3 + Balances::free_balance(4) - m4;
        let vals = Balances::free_balance(1) - v1 + Balances::free_balance(2) - v2;
        // 90,000 x 50/88 to miners, the rest to validators; nothing to an owner.
        assert!(close(miners, 51_136), "miners {miners}");
        assert!(close(vals, 38_864), "validators {vals}");
        assert!(Balances::free_balance(POT) <= 2);
    });
}

#[test]
fn closing_a_subnet_releases_its_stake() {
    new_test_ext().execute_with(|| {
        add_subnet(1);
        for a in 5..=6u64 {
            assert_ok!(Registry::register_neuron(RuntimeOrigin::signed(a), 1, a));
            assert_ok!(Consensus::stake_subnet(RuntimeOrigin::signed(a), 1, 2_000));
        }
        System::set_block_number(pallet_subnet_registry::IMMUNITY_BLOCKS as u64);
        assert_ok!(Registry::deregister_subnet(RuntimeOrigin::root(), 1));
        assert_eq!(SubnetTotalStake::<Test>::get(1), 0);
        assert_eq!(SubnetStake::<Test>::get(1, 5), 0);
        assert_eq!(Unstaking::<Test>::get(5).map(|u| u.0), Some(2_000));
        assert_eq!(Unstaking::<Test>::get(6).map(|u| u.0), Some(2_000));
        let at = Unstaking::<Test>::get(5).unwrap().1;
        System::set_block_number(at * 10);
        let before = Balances::free_balance(5);
        assert_ok!(Consensus::withdraw_unstaked(RuntimeOrigin::signed(5)));
        assert_eq!(Balances::free_balance(5), before + 2_000);
    });
}

#[test]
fn subnet_penalties_and_bans_cut_a_miner_out() {
    new_test_ext().execute_with(|| {
        add_subnet(1);
        for a in 5..=7u64 {
            assert_ok!(Registry::register_neuron(RuntimeOrigin::signed(a), 1, a));
        }
        assert_ok!(Consensus::stake_subnet(RuntimeOrigin::signed(5), 1, 10_000));
        unlock_at(0);
        let run = |b: u64| -> (u128, u128) {
            System::set_block_number(b);
            let epoch = Weights::current_epoch();
            commit_on(1, 5, vec![1, 2], vec![1, 1]);
            System::set_block_number(b + 10);
            reveal_on(1, 5, vec![1, 2], vec![1, 1]);
            assert_ok!(Balances::force_set_balance(
                RuntimeOrigin::root(),
                POT,
                900_000
            ));
            let (m6, m7) = (Balances::free_balance(6), Balances::free_balance(7));
            Consensus::settle(epoch);
            (
                Balances::free_balance(6) - m6,
                Balances::free_balance(7) - m7,
            )
        };
        let b = 200 * 7_200u64;
        // Miner 6 (uid 1) penalised for that epoch: miner 7 gets the miners' half.
        pallet_subnet_registry::WorkPenalized::<Test>::insert((1u16, b / 10, 1u16), ());
        let (g6, g7) = run(b);
        assert_eq!(g6, 0);
        assert!(close(g7, 399_999 / 2), "{g7}");
        // Miner 7 banned from SN1: no payout there, even with weight.
        pallet_subnet_registry::SubnetBanned::<Test>::insert(1u16, 7u64, ());
        let (g6, g7) = run(b + 100);
        assert_eq!(g7, 0);
        assert!(g6 > 0);
    });
}
