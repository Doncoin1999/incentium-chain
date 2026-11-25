use crate::mock::*;
use crate::pallet::{Error, HotkeyStake, NextBonusPpm, OwnerVesting, Pools as PoolStore, Positions, TotalStake};
use crate::PoolsInterface;
use ict_economics::BLOCKS_PER_DAY;
use polkadot_sdk::frame_support::{assert_noop, assert_ok, traits::fungible::Inspect};
use polkadot_sdk::sp_runtime::DispatchError;

fn open(netuid: u16, sale: u128) {
    <Pools as PoolsInterface<u64>>::open_pool(netuid, sale);
}

/// One day of epochs (20 x 360 blocks) with one miner and one validator paid.
fn epochs(netuid: u16, n: u32) {
    for _ in 0..n {
        <Pools as PoolsInterface<u64>>::on_epoch(netuid, 360, &[(MINER, 50)], &[(VALI, 38)]);
    }
}

fn pos(netuid: u16, hotkey: u64, who: u64) -> u128 {
    Positions::<Test>::get((netuid, hotkey, who))
}

fn ict_of(who: u64) -> u128 {
    Balances::balance(&who)
}

#[test]
fn a_pool_opens_at_sale_price_over_a_million_and_only_once() {
    new_test_ext().execute_with(|| {
        open(1, 1_000 * UNIT);
        let st = PoolStore::<Test>::get(1).unwrap();
        assert_eq!(st.pool().price(), UNIT / 1_000);
        assert_eq!(st.bonus_ppm, 500_000);
        assert_eq!(st.issued, 0);
        open(1, 50_000 * UNIT);
        assert_eq!(PoolStore::<Test>::get(1).unwrap().pool().price(), UNIT / 1_000);
    });
}

#[test]
fn nothing_can_be_bought_before_any_alpha_exists() {
    new_test_ext().execute_with(|| {
        open(1, 1_000 * UNIT);
        assert_noop!(
            crate::Pallet::<Test>::stake(RuntimeOrigin::signed(ALICE), 1, VALI, UNIT, 0),
            Error::<Test>::Insufficient
        );
    });
}

#[test]
fn an_epoch_issues_icts_schedule_and_splits_bonus_and_stock() {
    new_test_ext().execute_with(|| {
        open(1, 1_000 * UNIT);
        epochs(1, 1);
        let st = PoolStore::<Test>::get(1).unwrap();
        // 360 blocks at 1 Alpha: 180 bonus, 180 to the pool
        assert_eq!(st.issued, 360 * UNIT);
        assert_eq!(st.ry, 180 * UNIT);
        // bonus 50/38/12: miner 90, validator 68.4, owner 21.6 (vesting)
        assert_eq!(pos(1, MINER, MINER), 90 * UNIT);
        assert_eq!(pos(1, VALI, VALI), 68_400_000_000);
        assert_eq!(OwnerVesting::<Test>::get(1)[0].total, 21_600_000_000);
        assert_eq!(st.bonus_minted, 180 * UNIT);
        assert_eq!(TotalStake::<Test>::get(1), 158_400_000_000);
    });
}

#[test]
fn an_epoch_that_pays_nobody_sends_its_bonus_to_the_pool() {
    new_test_ext().execute_with(|| {
        open(1, 1_000 * UNIT);
        <Pools as PoolsInterface<u64>>::on_epoch(1, 360, &[], &[]);
        let st = PoolStore::<Test>::get(1).unwrap();
        assert_eq!(st.ry, 360 * UNIT);
        assert_eq!(st.bonus_minted, 0);
        assert!(OwnerVesting::<Test>::get(1).is_empty());
    });
}

#[test]
fn staking_buys_alpha_and_moves_the_icts_into_the_pool() {
    new_test_ext().execute_with(|| {
        open(1, 1_000 * UNIT);
        epochs(1, 20);
        let issuance = Balances::total_issuance();
        let before = ict_of(ALICE);
        assert_ok!(crate::Pallet::<Test>::stake(RuntimeOrigin::signed(ALICE), 1, VALI, UNIT, 0));
        let a = pos(1, VALI, ALICE);
        assert!(a > 0);
        let st = PoolStore::<Test>::get(1).unwrap();
        assert_eq!(before - ict_of(ALICE), UNIT);
        // 0.5 % fee, half burned
        assert_eq!(Balances::total_issuance(), issuance - UNIT / 400);
        assert_eq!(ict_of(crate::Pallet::<Test>::pool_account(1)), st.rx);
        assert_eq!(HotkeyStake::<Test>::get(1, VALI), a + pos(1, VALI, VALI));
        assert_eq!(<Pools as PoolsInterface<u64>>::stake_ict(1), st.rx);
    });
}

#[test]
fn stake_needs_a_neuron_and_a_pool() {
    new_test_ext().execute_with(|| {
        open(1, 1_000 * UNIT);
        epochs(1, 1);
        assert_noop!(
            crate::Pallet::<Test>::stake(RuntimeOrigin::signed(ALICE), 1, 77, UNIT, 0),
            Error::<Test>::NotNeuron
        );
        assert_noop!(
            crate::Pallet::<Test>::stake(RuntimeOrigin::signed(ALICE), 2, VALI, UNIT, 0),
            Error::<Test>::NoPool
        );
        assert_noop!(
            crate::Pallet::<Test>::stake(RuntimeOrigin::signed(ALICE), 1, VALI, 1, 0),
            Error::<Test>::TooSmall
        );
    });
}

#[test]
fn a_large_stake_fills_partly_and_charges_only_for_that() {
    new_test_ext().execute_with(|| {
        open(1, 1_000 * UNIT);
        epochs(1, 1); // 180 Alpha in the pool
        let before = ict_of(ALICE);
        assert_ok!(crate::Pallet::<Test>::stake(RuntimeOrigin::signed(ALICE), 1, VALI, 10_000 * UNIT, 0));
        let got = pos(1, VALI, ALICE);
        assert!(got <= 180 * UNIT * 99 / 100);
        assert!(before - ict_of(ALICE) < 10_000 * UNIT);
    });
}

#[test]
fn unstaking_sells_alpha_back_for_ict() {
    new_test_ext().execute_with(|| {
        open(1, 1_000 * UNIT);
        epochs(1, 20);
        assert_ok!(crate::Pallet::<Test>::stake(RuntimeOrigin::signed(ALICE), 1, VALI, 2 * UNIT, 0));
        let a = pos(1, VALI, ALICE);
        let before = ict_of(ALICE);
        let quote = crate::Pallet::<Test>::quote_unstake(1, a).unwrap();
        assert_noop!(
            crate::Pallet::<Test>::unstake(RuntimeOrigin::signed(ALICE), 1, VALI, a, quote + 1),
            Error::<Test>::SlippageExceeded
        );
        assert_ok!(crate::Pallet::<Test>::unstake(RuntimeOrigin::signed(ALICE), 1, VALI, a, quote));
        assert_eq!(ict_of(ALICE) - before, quote);
        assert!(quote < 2 * UNIT, "fees on the way in and out");
        assert_eq!(pos(1, VALI, ALICE), 0);
        assert_noop!(
            crate::Pallet::<Test>::unstake(RuntimeOrigin::signed(ALICE), 1, VALI, UNIT, 0),
            Error::<Test>::NotEnoughStake
        );
    });
}

#[test]
fn a_miner_cannot_sell_bonus_until_someone_has_bought() {
    new_test_ext().execute_with(|| {
        open(1, 1_000 * UNIT);
        epochs(1, 20);
        let a = pos(1, MINER, MINER);
        assert_noop!(
            crate::Pallet::<Test>::unstake(RuntimeOrigin::signed(MINER), 1, MINER, a, 0),
            Error::<Test>::Insufficient
        );
        assert_ok!(crate::Pallet::<Test>::stake(RuntimeOrigin::signed(ALICE), 1, VALI, 5 * UNIT, 0));
        assert_ok!(crate::Pallet::<Test>::unstake(RuntimeOrigin::signed(MINER), 1, MINER, a / 10, 0));
        assert!(ict_of(MINER) > 0);
    });
}

#[test]
fn stake_can_move_between_hotkeys_and_to_other_accounts() {
    new_test_ext().execute_with(|| {
        open(1, 1_000 * UNIT);
        epochs(1, 20);
        assert_ok!(crate::Pallet::<Test>::stake(RuntimeOrigin::signed(ALICE), 1, VALI, UNIT, 0));
        let a = pos(1, VALI, ALICE);
        assert_ok!(crate::Pallet::<Test>::move_stake(RuntimeOrigin::signed(ALICE), 1, VALI, MINER, a / 2));
        assert_eq!(pos(1, MINER, ALICE), a / 2);
        assert_ok!(crate::Pallet::<Test>::transfer_stake(RuntimeOrigin::signed(ALICE), 1, MINER, BOB, a / 2));
        assert_eq!(pos(1, MINER, BOB), a / 2);
        assert_eq!(pos(1, MINER, ALICE), 0);
        assert_noop!(
            crate::Pallet::<Test>::move_stake(RuntimeOrigin::signed(ALICE), 1, VALI, VALI, 1),
            Error::<Test>::SameHotkey
        );
    });
}

#[test]
fn the_owners_alpha_vests_over_180_days_and_goes_to_the_current_owner() {
    new_test_ext().execute_with(|| {
        open(1, 1_000 * UNIT);
        epochs(1, 20); // owner tranche: 20 x 21.6 = 432 Alpha
        assert_noop!(
            crate::Pallet::<Test>::claim_owner_alpha(RuntimeOrigin::signed(BOB), 1, OWNER),
            Error::<Test>::NothingToClaim
        );
        System::set_block_number(1 + 90 * BLOCKS_PER_DAY);
        assert_ok!(crate::Pallet::<Test>::claim_owner_alpha(RuntimeOrigin::signed(BOB), 1, OWNER));
        assert_eq!(pos(1, OWNER, OWNER), 216 * UNIT);
        System::set_block_number(1 + 200 * BLOCKS_PER_DAY);
        assert_ok!(crate::Pallet::<Test>::claim_owner_alpha(RuntimeOrigin::signed(BOB), 1, OWNER));
        assert_eq!(pos(1, OWNER, OWNER), 432 * UNIT);
        assert!(OwnerVesting::<Test>::get(1).is_empty());
    });
}

#[test]
fn without_an_owner_the_owners_part_goes_to_the_pool() {
    new_test_ext().execute_with(|| {
        HAS_OWNER.with(|h| *h.borrow_mut() = false);
        open(1, 1_000 * UNIT);
        epochs(1, 1);
        assert_eq!(PoolStore::<Test>::get(1).unwrap().ry, 180 * UNIT + 21_600_000_000);
        HAS_OWNER.with(|h| *h.borrow_mut() = true);
    });
}

#[test]
fn a_closed_pool_issues_nothing_but_lets_holders_out() {
    new_test_ext().execute_with(|| {
        open(1, 1_000 * UNIT);
        epochs(1, 20);
        assert_ok!(crate::Pallet::<Test>::stake(RuntimeOrigin::signed(ALICE), 1, VALI, 2 * UNIT, 0));
        <Pools as PoolsInterface<u64>>::close_pool(1);
        let issued = PoolStore::<Test>::get(1).unwrap().issued;
        epochs(1, 5);
        assert_eq!(PoolStore::<Test>::get(1).unwrap().issued, issued);
        let a = pos(1, VALI, ALICE);
        assert_ok!(crate::Pallet::<Test>::unstake(RuntimeOrigin::signed(ALICE), 1, VALI, a, 0));
    });
}

#[test]
fn the_bonus_share_can_change_only_for_new_subnets_within_bounds() {
    new_test_ext().execute_with(|| {
        open(1, 1_000 * UNIT);
        assert_noop!(
            crate::Pallet::<Test>::set_next_bonus_ppm(RuntimeOrigin::signed(ALICE), 600_000),
            DispatchError::BadOrigin
        );
        assert_noop!(
            crate::Pallet::<Test>::set_next_bonus_ppm(RuntimeOrigin::root(), 800_000),
            Error::<Test>::BonusOutOfRange
        );
        assert_ok!(crate::Pallet::<Test>::set_next_bonus_ppm(RuntimeOrigin::root(), 600_000));
        assert_eq!(NextBonusPpm::<Test>::get(), 600_000);
        open(2, 1_000 * UNIT);
        assert_eq!(PoolStore::<Test>::get(1).unwrap().bonus_ppm, 500_000);
        assert_eq!(PoolStore::<Test>::get(2).unwrap().bonus_ppm, 600_000);
    });
}

#[test]
fn summaries_list_pools_in_netuid_order_with_todays_figures() {
    new_test_ext().execute_with(|| {
        open(2, 1_000 * UNIT);
        open(1, 50_000 * UNIT);
        epochs(1, 20);
        assert_ok!(crate::Pallet::<Test>::stake(RuntimeOrigin::signed(ALICE), 1, VALI, UNIT, 0));
        let s = crate::Pallet::<Test>::summaries();
        assert_eq!(s.iter().map(|p| p.netuid).collect::<Vec<_>>(), vec![1, 2]);
        assert_eq!(s[0].name, b"SN1".to_vec());
        assert_eq!(s[0].alpha_issued, 7_200 * UNIT);
        assert_eq!(s[0].bonus_today + s[0].stock_today, 7_200 * UNIT);
        assert!(s[0].bought_today > 0);
        assert_eq!(s[0].fee_ppm, 5_000);
        assert!(crate::Pallet::<Test>::quote_stake(1, UNIT).is_some());
        assert_eq!(crate::Pallet::<Test>::quote_stake(3, UNIT), None);
    });
}

/// Every Alpha ever issued is in a position, in the pool, still vesting, or burned as a fee.
#[test]
fn alpha_is_conserved() {
    new_test_ext().execute_with(|| {
        open(1, 1_000 * UNIT);
        let mut burned = 0u128;
        for day in 0..10u64 {
            System::set_block_number(1 + day * BLOCKS_PER_DAY);
            epochs(1, 20);
            assert_ok!(crate::Pallet::<Test>::stake(RuntimeOrigin::signed(ALICE), 1, VALI, 3 * UNIT, 0));
            let a = pos(1, MINER, MINER) / 3;
            if let Ok(()) = crate::Pallet::<Test>::unstake(RuntimeOrigin::signed(MINER), 1, MINER, a, 0) {
                burned += a * 5 / 1_000 / 2;
            }
        }
        let st = PoolStore::<Test>::get(1).unwrap();
        let vesting: u128 = OwnerVesting::<Test>::get(1).iter().map(|t| t.total - t.claimed).sum();
        let staked = TotalStake::<Test>::get(1);
        let diff = st.issued.abs_diff(staked + st.ry + vesting + burned);
        assert!(diff < 100, "rounding only, got {diff}");
    });
}
