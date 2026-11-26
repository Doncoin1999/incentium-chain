use crate::mock::*;
use crate::pallet::{
    Error, HotkeyStake, NextBonusPpm, OwnerVesting, Pools as PoolStore, Positions, PriceDays, Takes, TotalStake,
};
use crate::PoolsInterface;
use ict_economics::{alpha, BLOCKS_PER_DAY};
use polkadot_sdk::frame_support::{
    assert_noop, assert_ok,
    traits::{fungible::Inspect, Hooks},
};

type P = crate::Pallet<Test>;

fn register(netuid: u16, reserved: u128) {
    assert_ok!(<P as PoolsInterface<u64>>::register_pool(netuid, &SELLER, reserved));
}

/// `n` epochs of 360 blocks with one miner and one validator paid, advancing the clock and
/// running the per-block hook.
fn epochs(netuid: u16, n: u32) {
    for _ in 0..n {
        <P as PoolsInterface<u64>>::on_epoch(netuid, 360, &[(MINER, 50)], &[(VALI, 38)]);
        advance(360);
    }
}

fn advance(blocks: u64) {
    for _ in 0..blocks {
        let b = System::block_number() + 1;
        System::set_block_number(b);
        P::on_initialize(b);
    }
}

/// Register, run the 10 closed days, and return once the pool is open.
fn open_pool(netuid: u16, reserved: u128) {
    register(netuid, reserved);
    epochs(netuid, 200);
    assert!(PoolStore::<Test>::get(netuid).unwrap().trading);
}

fn pos(netuid: u16, hotkey: u64, who: u64) -> u128 {
    Positions::<Test>::get((netuid, hotkey, who))
}

fn ict_of(who: u64) -> u128 {
    Balances::balance(&who)
}

fn price(netuid: u16) -> u128 {
    PoolStore::<Test>::get(netuid).unwrap().pool().price()
}

#[test]
fn a_pool_registers_closed_holding_the_auction_ict() {
    new_test_ext().execute_with(|| {
        register(1, 500 * UNIT);
        let st = PoolStore::<Test>::get(1).unwrap();
        assert!(!st.trading);
        assert_eq!(st.reserved_ict, 500 * UNIT);
        assert_eq!(ict_of(P::pool_account(1)), 500 * UNIT);
        assert_eq!(<P as PoolsInterface<u64>>::stake_ict(1), 500 * UNIT);
        assert_noop!(<P as PoolsInterface<u64>>::register_pool(1, &SELLER, UNIT), Error::<Test>::PoolExists);
    });
}

#[test]
fn nothing_can_be_bought_or_sold_in_the_first_ten_days() {
    new_test_ext().execute_with(|| {
        register(1, 500 * UNIT);
        epochs(1, 100);
        assert!(pos(1, MINER, MINER) > 0);
        assert_noop!(P::stake(RuntimeOrigin::signed(ALICE), 1, VALI, UNIT, 0), Error::<Test>::NotTrading);
        assert_noop!(P::unstake(RuntimeOrigin::signed(MINER), 1, MINER, UNIT, 0), Error::<Test>::NotTrading);
        assert!(P::quote_stake(1, UNIT).is_none());
        // stock collects outside the pool
        let st = PoolStore::<Test>::get(1).unwrap();
        assert_eq!(st.y, 0);
        assert!(st.waiting > 0);
    });
}

#[test]
fn the_pool_opens_after_ten_days_at_the_valuation_price() {
    new_test_ext().execute_with(|| {
        register(1, 500 * UNIT);
        epochs(1, 199);
        assert!(!PoolStore::<Test>::get(1).unwrap().trading);
        epochs(1, 1);
        let st = PoolStore::<Test>::get(1).unwrap();
        assert!(st.trading);
        let p0 = alpha::start_price(288 * UNIT, 1_280, alpha::DEFAULT_REQUIRED_RETURN_PPM);
        // opened with all 500 ICT and 500 / p0 Alpha; the price has moved only by released stock since
        assert!(st.x == 500 * UNIT);
        assert!(st.pool().price() <= p0 && st.pool().price() * 100 >= p0 * 95);
        assert_eq!(<P as PoolsInterface<u64>>::stake_ict(1), 500 * UNIT);
        assert_eq!(PriceDays::<Test>::get(1).len(), 1);
    });
}

#[test]
fn auction_ict_the_pool_cannot_match_goes_to_the_treasury() {
    new_test_ext().execute_with(|| {
        let before = ict_of(TREASURY);
        open_pool(1, 5_000 * UNIT);
        let st = PoolStore::<Test>::get(1).unwrap();
        let p0 = alpha::start_price(288 * UNIT, 1_280, alpha::DEFAULT_REQUIRED_RETURN_PPM);
        let cap = ict_economics::mul_div(alpha::OPEN_DEPTH, p0, UNIT);
        assert_eq!(st.x, cap);
        assert_eq!(ict_of(TREASURY) - before, 5_000 * UNIT - cap);
        assert_eq!(ict_of(P::pool_account(1)), cap);
    });
}

#[test]
fn staking_buys_alpha_with_a_price_limit() {
    new_test_ext().execute_with(|| {
        open_pool(1, 500 * UNIT);
        let q = P::quote_stake(1, 100 * UNIT).unwrap();
        assert_noop!(
            P::stake(RuntimeOrigin::signed(ALICE), 1, VALI, 100 * UNIT, q + 1),
            Error::<Test>::SlippageExceeded
        );
        let p_before = price(1);
        let pool_before = ict_of(P::pool_account(1));
        assert_ok!(P::stake(RuntimeOrigin::signed(ALICE), 1, VALI, 100 * UNIT, q));
        assert_eq!(pos(1, VALI, ALICE), q);
        assert!(price(1) > p_before);
        // the burned quarter of a percent is gone; the rest is in the pool
        assert_eq!(ict_of(P::pool_account(1)) - pool_before, 100 * UNIT - 100 * UNIT / 400);
        assert_noop!(P::stake(RuntimeOrigin::signed(ALICE), 1, 77, UNIT, 0), Error::<Test>::NotNeuron);
        assert_noop!(P::stake(RuntimeOrigin::signed(ALICE), 3, VALI, UNIT, 0), Error::<Test>::NotNeuron);
    });
}

#[test]
fn unstaking_sells_alpha_back_for_ict() {
    new_test_ext().execute_with(|| {
        open_pool(1, 500 * UNIT);
        assert_ok!(P::stake(RuntimeOrigin::signed(ALICE), 1, VALI, 100 * UNIT, 0));
        let a = pos(1, VALI, ALICE);
        let before = ict_of(ALICE);
        let q = P::quote_unstake(1, a).unwrap();
        assert_ok!(P::unstake(RuntimeOrigin::signed(ALICE), 1, VALI, a, q));
        assert_eq!(ict_of(ALICE) - before, q);
        assert!(q < 100 * UNIT && q > 98 * UNIT);
        assert_eq!(pos(1, VALI, ALICE), 0);
        assert_noop!(P::unstake(RuntimeOrigin::signed(ALICE), 1, VALI, UNIT, 0), Error::<Test>::NotEnoughStake);
    });
}

#[test]
fn miners_can_sell_their_bonus_once_the_pool_is_open() {
    new_test_ext().execute_with(|| {
        open_pool(1, 500 * UNIT);
        let a = pos(1, MINER, MINER) / 10;
        let before = ict_of(MINER);
        assert_ok!(P::unstake(RuntimeOrigin::signed(MINER), 1, MINER, a, 0));
        assert!(ict_of(MINER) > before);
    });
}

#[test]
fn stock_is_released_block_by_block_and_held_back_after_a_fall() {
    new_test_ext().execute_with(|| {
        open_pool(1, 500 * UNIT);
        let w = PoolStore::<Test>::get(1).unwrap().waiting;
        advance(10);
        let st = PoolStore::<Test>::get(1).unwrap();
        // at most two blocks' stock (2 x 0.5 Alpha) a block
        assert!(w - st.waiting <= 10 * UNIT && w > st.waiting);
        // a crash: everyone sells; the price falls more than 20 % below its average
        epochs(1, 1);
        let sells = [MINER, VALI];
        for who in sells {
            let a = pos(1, who, who);
            let _ = P::unstake(RuntimeOrigin::signed(who), 1, who, a, 0);
        }
        let avg = {
            let d = PriceDays::<Test>::get(1);
            d.iter().sum::<u128>() / d.len() as u128
        };
        assert!(price(1) < avg * 8 / 10, "price {} avg {}", price(1), avg);
        let st = PoolStore::<Test>::get(1).unwrap();
        advance(10);
        let st2 = PoolStore::<Test>::get(1).unwrap();
        assert_eq!(st2.y, st.y, "stock held back");
        assert_eq!(st2.waiting, st.waiting);
    });
}

#[test]
fn the_price_is_sampled_once_a_day_for_thirty_days() {
    new_test_ext().execute_with(|| {
        open_pool(1, 500 * UNIT);
        epochs(1, 20 * 40);
        assert_eq!(PriceDays::<Test>::get(1).len() as u32, alpha::AVG_DAYS);
    });
}

#[test]
fn validators_keep_their_take_and_stakers_get_the_rest_in_ict() {
    new_test_ext().execute_with(|| {
        open_pool(1, 500 * UNIT);
        assert_ok!(P::stake(RuntimeOrigin::signed(ALICE), 1, VALI, 100 * UNIT, 0));
        assert_ok!(P::stake(RuntimeOrigin::signed(BOB), 1, VALI, 100 * UNIT, 0));
        let (va, al, bo) = (pos(1, VALI, VALI), pos(1, VALI, ALICE), pos(1, VALI, BOB));
        let total = HotkeyStake::<Test>::get(1, VALI);
        assert_eq!(total, va + al + bo);
        let vali_before = ict_of(VALI);
        assert_ok!(<P as PoolsInterface<u64>>::pay_validator_ict(1, &SELLER, &VALI, 1_000 * UNIT));
        // default take 18 %
        assert_eq!(ict_of(VALI) - vali_before, 180 * UNIT);
        let alice_due = P::pending_dividends(1, &VALI, &ALICE);
        let expect = ict_economics::mul_div(820 * UNIT, al, total);
        assert!(alice_due <= expect && alice_due + 2 >= expect);
        let before = ict_of(ALICE);
        assert_ok!(P::claim_dividends(RuntimeOrigin::signed(ALICE), 1, VALI));
        assert_eq!(ict_of(ALICE) - before, alice_due);
        assert_noop!(P::claim_dividends(RuntimeOrigin::signed(ALICE), 1, VALI), Error::<Test>::NothingToClaim);
        // the validator's own bonus Alpha earns dividends too
        assert!(P::pending_dividends(1, &VALI, &VALI) > 0);
    });
}

#[test]
fn dividends_follow_the_stake_held_when_they_were_paid() {
    new_test_ext().execute_with(|| {
        open_pool(1, 500 * UNIT);
        assert_ok!(P::stake(RuntimeOrigin::signed(ALICE), 1, VALI, 100 * UNIT, 0));
        assert_ok!(<P as PoolsInterface<u64>>::pay_validator_ict(1, &SELLER, &VALI, 1_000 * UNIT));
        // Bob comes in after the payment: nothing of it is his
        assert_ok!(P::stake(RuntimeOrigin::signed(BOB), 1, VALI, 100 * UNIT, 0));
        assert_eq!(P::pending_dividends(1, &VALI, &BOB), 0);
        // Alice's due is paid out automatically when she changes her stake
        let due = P::pending_dividends(1, &VALI, &ALICE);
        let before = ict_of(ALICE);
        assert!(due > UNIT);
        assert_ok!(P::stake(RuntimeOrigin::signed(ALICE), 1, VALI, UNIT, 0));
        assert_eq!(ict_of(ALICE) + UNIT, before + due);
        assert_eq!(P::pending_dividends(1, &VALI, &ALICE), 0);
    });
}

#[test]
fn a_validator_without_stakers_keeps_everything() {
    new_test_ext().execute_with(|| {
        register(1, 500 * UNIT);
        let before = ict_of(VALI);
        assert_ok!(<P as PoolsInterface<u64>>::pay_validator_ict(1, &SELLER, &VALI, 10 * UNIT));
        assert_eq!(ict_of(VALI) - before, 10 * UNIT);
    });
}

#[test]
fn takes_are_0_to_18_percent_and_rise_at_most_once_a_month() {
    new_test_ext().execute_with(|| {
        assert_noop!(P::set_take(RuntimeOrigin::signed(VALI), 180_001), Error::<Test>::TakeTooHigh);
        assert_ok!(P::set_take(RuntimeOrigin::signed(VALI), 50_000));
        assert_ok!(P::set_take(RuntimeOrigin::signed(VALI), 100_000));
        assert_noop!(P::set_take(RuntimeOrigin::signed(VALI), 120_000), Error::<Test>::TakeRaisedTooSoon);
        assert_ok!(P::set_take(RuntimeOrigin::signed(VALI), 0));
        System::set_block_number(System::block_number() + 30 * BLOCKS_PER_DAY);
        assert_ok!(P::set_take(RuntimeOrigin::signed(VALI), 180_000));
        assert_eq!(Takes::<Test>::get(VALI).ppm, 180_000);
    });
}

#[test]
fn stake_can_move_between_hotkeys_and_to_other_accounts() {
    new_test_ext().execute_with(|| {
        open_pool(1, 500 * UNIT);
        assert_ok!(P::stake(RuntimeOrigin::signed(ALICE), 1, VALI, 100 * UNIT, 0));
        let a = pos(1, VALI, ALICE);
        assert_ok!(P::move_stake(RuntimeOrigin::signed(ALICE), 1, VALI, MINER, a / 2));
        assert_ok!(P::transfer_stake(RuntimeOrigin::signed(ALICE), 1, MINER, BOB, a / 4));
        assert_eq!(pos(1, VALI, ALICE) + pos(1, MINER, ALICE) + pos(1, MINER, BOB), a);
        assert_noop!(P::move_stake(RuntimeOrigin::signed(ALICE), 1, VALI, VALI, 1), Error::<Test>::SameHotkey);
    });
}

#[test]
fn the_owners_alpha_vests_over_180_days_and_goes_to_the_current_owner() {
    new_test_ext().execute_with(|| {
        register(1, 500 * UNIT);
        epochs(1, 20);
        assert!(!OwnerVesting::<Test>::get(1).is_empty());
        assert_ok!(P::claim_owner_alpha(RuntimeOrigin::signed(ALICE), 1, OWNER));
        let part = pos(1, OWNER, OWNER);
        assert!(part > 0);
        System::set_block_number(System::block_number() + 200 * BLOCKS_PER_DAY);
        assert_ok!(P::claim_owner_alpha(RuntimeOrigin::signed(ALICE), 1, OWNER));
        assert!(pos(1, OWNER, OWNER) > part);
        assert!(OwnerVesting::<Test>::get(1).is_empty());
    });
}

#[test]
fn without_an_owner_the_owners_part_goes_to_the_stock() {
    new_test_ext().execute_with(|| {
        HAS_OWNER.with(|h| *h.borrow_mut() = false);
        register(1, 500 * UNIT);
        epochs(1, 1);
        let st = PoolStore::<Test>::get(1).unwrap();
        // 360 Alpha: miner 90 + validator 68.4 as bonus, the rest (owner 21.6 + stock 180) waits
        assert_eq!(st.waiting, 360 * UNIT - 90 * UNIT - 684 * UNIT / 10);
        HAS_OWNER.with(|h| *h.borrow_mut() = true);
    });
}

#[test]
fn a_removed_pool_burns_its_stock_and_owner_alpha_but_lets_holders_out() {
    new_test_ext().execute_with(|| {
        open_pool(1, 500 * UNIT);
        assert_ok!(P::stake(RuntimeOrigin::signed(ALICE), 1, VALI, 100 * UNIT, 0));
        <P as PoolsInterface<u64>>::close_pool(1);
        let st = PoolStore::<Test>::get(1).unwrap();
        assert!(st.removed && st.waiting == 0);
        assert!(OwnerVesting::<Test>::get(1).is_empty());
        let issued = st.issued;
        epochs(1, 5);
        assert_eq!(PoolStore::<Test>::get(1).unwrap().issued, issued);
        let a = pos(1, VALI, ALICE);
        assert_ok!(P::unstake(RuntimeOrigin::signed(ALICE), 1, VALI, a, 0));
    });
}

#[test]
fn a_pool_removed_before_it_opened_returns_the_held_ict_to_the_treasury() {
    new_test_ext().execute_with(|| {
        let before = ict_of(TREASURY);
        register(1, 500 * UNIT);
        epochs(1, 10);
        <P as PoolsInterface<u64>>::close_pool(1);
        assert_eq!(ict_of(TREASURY) - before, 500 * UNIT);
        epochs(1, 200);
        assert!(!PoolStore::<Test>::get(1).unwrap().trading);
    });
}

#[test]
fn the_bonus_share_and_required_return_change_only_within_bounds() {
    new_test_ext().execute_with(|| {
        assert_noop!(P::set_next_bonus_ppm(RuntimeOrigin::root(), 800_000), Error::<Test>::BonusOutOfRange);
        assert_ok!(P::set_next_bonus_ppm(RuntimeOrigin::root(), 300_000));
        assert_eq!(NextBonusPpm::<Test>::get(), 300_000);
        assert_noop!(P::set_required_return(RuntimeOrigin::root(), 10_000), Error::<Test>::ReturnOutOfRange);
        assert_ok!(P::set_required_return(RuntimeOrigin::root(), 300_000));
        assert!(P::set_required_return(RuntimeOrigin::signed(ALICE), 300_000).is_err());
    });
}

#[test]
fn summaries_list_pools_in_netuid_order() {
    new_test_ext().execute_with(|| {
        register(2, 500 * UNIT);
        open_pool(1, 500 * UNIT);
        let s = P::summaries();
        assert_eq!(s.iter().map(|p| p.netuid).collect::<Vec<_>>(), vec![1, 2]);
        assert!(s[0].open && s[0].price > 0 && s[0].ict_reserve == 500 * UNIT);
        // SN2 had no epochs, so no Alpha exists for it: it stays closed
        assert!(!s[1].open && s[1].price == 0);
    });
}

#[test]
fn alpha_is_conserved() {
    new_test_ext().execute_with(|| {
        open_pool(1, 500 * UNIT);
        assert_ok!(P::stake(RuntimeOrigin::signed(ALICE), 1, VALI, 300 * UNIT, 0));
        let a = pos(1, MINER, MINER) / 3;
        assert_ok!(P::unstake(RuntimeOrigin::signed(MINER), 1, MINER, a, 0));
        let burned_fee = a / 200 / 2; // half the 0.5 % fee on the sale is burned Alpha
        epochs(1, 30);
        let st = PoolStore::<Test>::get(1).unwrap();
        let vesting: u128 = OwnerVesting::<Test>::get(1).iter().map(|t| t.total - t.claimed).sum();
        // issued = in pool + waiting + staked + vesting + the Alpha fee burned on the sale
        let accounted = st.y + st.waiting + TotalStake::<Test>::get(1) + vesting + burned_fee;
        assert!(accounted.abs_diff(st.issued) <= 2, "off by {}", accounted.abs_diff(st.issued));
    });
}
