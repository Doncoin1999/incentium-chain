#![allow(deprecated)] // ValidateUnsigned, see lib.rs
use crate::{mock::*, Error, HoldReason, Locks, Reservations, SellOrders, State, MAX_FILLS};
use polkadot_sdk::frame_support::pallet_prelude::ValidateUnsigned;
use polkadot_sdk::frame_support::{
    assert_noop, assert_ok,
    pallet_prelude::{InvalidTransaction, TransactionSource},
    traits::fungible::{Inspect, InspectHold},
};
use polkadot_sdk::sp_core::{H160, H256};
use polkadot_sdk::sp_io::hashing::blake2_256;

const PRICE: u128 = 250_000; // 0.25 USDC per ICT
const BASE_RECIPIENT: H160 = H160([0x11; 20]);
const DAY: u64 = 86_400;

fn secret(i: u8) -> H256 {
    H256(blake2_256(&[b"test secret".as_slice(), &[i]].concat()))
}

fn hashes() -> Box<[H256; MAX_FILLS]> {
    Box::new(core::array::from_fn(|i| Swap::hash_of(&secret(i as u8))))
}

fn place(qty: u128) -> H256 {
    assert_ok!(Swap::place_sell(
        RuntimeOrigin::signed(SELLER),
        1,
        qty,
        PRICE,
        BASE_RECIPIENT,
        T0 + DAY,
        hashes()
    ));
    Swap::order_id(&SELLER, 1)
}

fn held(who: u64) -> u128 {
    Balances::balance_on_hold(&HoldReason::Swap.into(), &who)
}

#[test]
fn sha256_vector_shared_with_base_contract() {
    // Same vector as IncentiumSwap.t.sol: sha256 of the 32 bytes 0x01..0x20.
    let s = H256(core::array::from_fn(|i| i as u8 + 1));
    assert_eq!(
        Swap::hash_of(&s),
        H256(hex32(
            "ae216c2ef5247a3782c135efa279a3e4cdc61094270f5d2be58c6204b7a612c9"
        ))
    );
}

fn hex32(h: &str) -> [u8; 32] {
    core::array::from_fn(|i| u8::from_str_radix(&h[2 * i..2 * i + 2], 16).unwrap())
}

#[test]
fn place_holds_ict_in_the_makers_account() {
    new_test_ext().execute_with(|| {
        let order = place(100 * ICT);
        assert_eq!(held(SELLER), 100 * ICT);
        assert_eq!(Balances::total_balance(&SELLER), 10_000 * ICT);
        let o = SellOrders::<Test>::get(order).unwrap();
        assert_eq!(o.remaining, 100 * ICT);
        assert!(o.open);
    });
}

#[test]
fn caps_follow_the_day() {
    new_test_ext().execute_with(|| {
        assert_eq!(Swap::max_order_qty(), 100 * ICT);
        assert_noop!(
            Swap::place_sell(
                RuntimeOrigin::signed(SELLER),
                1,
                101 * ICT,
                PRICE,
                BASE_RECIPIENT,
                T0 + 3600,
                hashes()
            ),
            Error::<Test>::OverCap
        );
        System::set_block_number(90 * 7_200);
        assert_eq!(Swap::max_order_qty(), 1_000 * ICT);
        System::set_block_number(120 * 7_200 - 1);
        assert_eq!(Swap::max_order_qty(), 1_000 * ICT);
        System::set_block_number(120 * 7_200);
        assert_eq!(Swap::max_order_qty(), 5_000 * ICT);
        place(5_000 * ICT);
    });
}

#[test]
fn rejects_bad_orders() {
    new_test_ext().execute_with(|| {
        let s = RuntimeOrigin::signed(SELLER);
        assert_noop!(
            Swap::place_sell(s.clone(), 1, 1, PRICE, BASE_RECIPIENT, T0 + 3600, hashes()),
            Error::<Test>::BadQuantity
        );
        assert_noop!(
            Swap::place_sell(
                s.clone(),
                1,
                50 * ICT,
                PRICE,
                BASE_RECIPIENT,
                T0 + DAY + 1,
                hashes()
            ),
            Error::<Test>::BadExpiry
        );
        assert_noop!(
            Swap::place_sell(
                s.clone(),
                1,
                10 * ICT,
                PRICE,
                BASE_RECIPIENT,
                T0 + 3600,
                hashes()
            ),
            Error::<Test>::TooSmall
        );
        assert_ok!(Swap::place_sell(
            s.clone(),
            1,
            50 * ICT,
            PRICE,
            BASE_RECIPIENT,
            T0 + 3600,
            hashes()
        ));
        assert_noop!(
            Swap::place_sell(s, 1, 50 * ICT, PRICE, BASE_RECIPIENT, T0 + 3600, hashes()),
            Error::<Test>::OrderExists
        );
    });
}

#[test]
fn reserve_and_claim_moves_ict_to_the_buyer() {
    new_test_ext().execute_with(|| {
        let order = place(100 * ICT);
        assert_ok!(Swap::reserve(
            RuntimeOrigin::signed(KEEPER),
            order,
            3,
            60 * ICT,
            BUYER
        ));
        let r = Reservations::<Test>::get(order, 3).unwrap();
        assert_eq!(r.usdc, 15_000_000);
        assert_noop!(
            Swap::claim_reservation(RuntimeOrigin::none(), order, 3, secret(4)),
            Error::<Test>::WrongSecret
        );
        assert_ok!(Swap::claim_reservation(
            RuntimeOrigin::none(),
            order,
            3,
            secret(3)
        ));
        assert_eq!(Balances::total_balance(&BUYER), 10_060 * ICT);
        assert_eq!(Balances::total_balance(&SELLER), 9_940 * ICT);
        assert_eq!(held(SELLER), 40 * ICT);
        assert_noop!(
            Swap::claim_reservation(RuntimeOrigin::none(), order, 3, secret(3)),
            Error::<Test>::NotActive
        );
    });
}

#[test]
fn cancel_keeps_reservations_alive() {
    new_test_ext().execute_with(|| {
        let order = place(100 * ICT);
        assert_ok!(Swap::reserve(
            RuntimeOrigin::signed(KEEPER),
            order,
            0,
            40 * ICT,
            BUYER
        ));
        assert_noop!(
            Swap::cancel_sell(RuntimeOrigin::signed(BUYER), order),
            Error::<Test>::NotMaker
        );
        assert_ok!(Swap::cancel_sell(RuntimeOrigin::signed(SELLER), order));
        assert_eq!(held(SELLER), 40 * ICT);
        assert_ok!(Swap::claim_reservation(
            RuntimeOrigin::none(),
            order,
            0,
            secret(0)
        ));
        assert_eq!(held(SELLER), 0);
        assert_eq!(Balances::total_balance(&BUYER), 10_040 * ICT);
    });
}

#[test]
fn fill_slots_are_used_once_even_after_expiry() {
    new_test_ext().execute_with(|| {
        let order = place(100 * ICT);
        assert_ok!(Swap::reserve(
            RuntimeOrigin::signed(KEEPER),
            order,
            0,
            20 * ICT,
            BUYER
        ));
        assert_noop!(
            Swap::reserve(RuntimeOrigin::signed(KEEPER), order, 0, 20 * ICT, BUYER),
            Error::<Test>::FillTaken
        );
        advance(19 * 3600);
        assert_ok!(Swap::expire_reservation(RuntimeOrigin::none(), order, 0));
        assert_noop!(
            Swap::reserve(RuntimeOrigin::signed(KEEPER), order, 0, 20 * ICT, BUYER),
            Error::<Test>::FillTaken
        );
        assert_noop!(
            Swap::reserve(RuntimeOrigin::signed(KEEPER), order, 16, 20 * ICT, BUYER),
            Error::<Test>::FillTaken
        );
    });
}

#[test]
fn at_most_three_open_reservations() {
    new_test_ext().execute_with(|| {
        let order = place(100 * ICT);
        for fill in 0..3 {
            assert_ok!(Swap::reserve(
                RuntimeOrigin::signed(KEEPER),
                order,
                fill,
                20 * ICT,
                BUYER
            ));
        }
        assert_noop!(
            Swap::reserve(RuntimeOrigin::signed(KEEPER), order, 3, 20 * ICT, BUYER),
            Error::<Test>::TooManyOpenReservations
        );
        assert_ok!(Swap::claim_reservation(
            RuntimeOrigin::none(),
            order,
            1,
            secret(1)
        ));
        assert_ok!(Swap::reserve(
            RuntimeOrigin::signed(KEEPER),
            order,
            3,
            20 * ICT,
            BUYER
        ));
    });
}

#[test]
fn reservation_deadlines() {
    new_test_ext().execute_with(|| {
        let order = place(100 * ICT);
        assert_ok!(Swap::reserve(
            RuntimeOrigin::signed(KEEPER),
            order,
            0,
            40 * ICT,
            BUYER
        ));
        assert_noop!(
            Swap::expire_reservation(RuntimeOrigin::none(), order, 0),
            Error::<Test>::TooEarly
        );
        advance(19 * 3600);
        assert_noop!(
            Swap::claim_reservation(RuntimeOrigin::none(), order, 0, secret(0)),
            Error::<Test>::TooLate
        );
        assert_ok!(Swap::expire_reservation(RuntimeOrigin::none(), order, 0));
        // order still open (24 h): the quantity goes back to it, the ICT stays held
        assert_eq!(SellOrders::<Test>::get(order).unwrap().remaining, 100 * ICT);
        assert_eq!(held(SELLER), 100 * ICT);
        assert_eq!(
            Reservations::<Test>::get(order, 0).unwrap().state,
            State::Expired
        );
    });
}

#[test]
fn expired_reservation_after_order_end_releases_to_maker() {
    new_test_ext().execute_with(|| {
        let order = place(100 * ICT);
        advance(20 * 3600);
        assert_ok!(Swap::reserve(
            RuntimeOrigin::signed(KEEPER),
            order,
            0,
            40 * ICT,
            BUYER
        ));
        advance(5 * 3600);
        assert_ok!(Swap::refund_expired(RuntimeOrigin::none(), order));
        assert_eq!(held(SELLER), 40 * ICT);
        advance(15 * 3600);
        assert_ok!(Swap::expire_reservation(RuntimeOrigin::none(), order, 0));
        assert_eq!(held(SELLER), 0);
        assert_eq!(Balances::total_balance(&SELLER), 10_000 * ICT);
    });
}

#[test]
fn no_reserve_after_expiry_or_over_remaining() {
    new_test_ext().execute_with(|| {
        let order = place(100 * ICT);
        assert_noop!(
            Swap::reserve(RuntimeOrigin::signed(KEEPER), order, 0, 101 * ICT, BUYER),
            Error::<Test>::BadQuantity
        );
        assert_noop!(
            Swap::reserve(RuntimeOrigin::signed(KEEPER), order, 0, 10 * ICT, BUYER),
            Error::<Test>::TooSmall
        );
        assert_noop!(
            Swap::refund_expired(RuntimeOrigin::none(), order),
            Error::<Test>::TooEarly
        );
        advance(DAY);
        assert_noop!(
            Swap::reserve(RuntimeOrigin::signed(KEEPER), order, 0, 40 * ICT, BUYER),
            Error::<Test>::OrderExpired
        );
    });
}

// ---------------------------------------------------------------- locks

fn lock_for(ttl: u64) -> H256 {
    let base_order = H256([0xab; 32]);
    assert_ok!(Swap::lock(
        RuntimeOrigin::signed(SELLER),
        base_order,
        5,
        Swap::hash_of(&secret(5)),
        BUYER,
        80 * ICT,
        T0 + ttl
    ));
    Swap::lock_id(&SELLER, base_order, 5)
}

#[test]
fn lock_claim_pays_the_buy_maker() {
    new_test_ext().execute_with(|| {
        let lock = lock_for(12 * 3600);
        assert_eq!(held(SELLER), 80 * ICT);
        assert_noop!(
            Swap::claim_lock(RuntimeOrigin::none(), lock, secret(6)),
            Error::<Test>::WrongSecret
        );
        assert_ok!(Swap::claim_lock(RuntimeOrigin::none(), lock, secret(5)));
        assert_eq!(Balances::total_balance(&BUYER), 10_080 * ICT);
        assert_eq!(held(SELLER), 0);
        assert_noop!(
            Swap::refund_lock(RuntimeOrigin::none(), lock),
            Error::<Test>::NotActive
        );
    });
}

#[test]
fn lock_refund_after_expiry() {
    new_test_ext().execute_with(|| {
        let lock = lock_for(12 * 3600);
        assert_noop!(
            Swap::refund_lock(RuntimeOrigin::none(), lock),
            Error::<Test>::TooEarly
        );
        advance(12 * 3600);
        assert_noop!(
            Swap::claim_lock(RuntimeOrigin::none(), lock, secret(5)),
            Error::<Test>::TooLate
        );
        assert_ok!(Swap::refund_lock(RuntimeOrigin::none(), lock));
        assert_eq!(held(SELLER), 0);
        assert_eq!(Balances::total_balance(&SELLER), 10_000 * ICT);
        assert_eq!(Locks::<Test>::get(lock).unwrap().state, State::Expired);
    });
}

#[test]
fn lock_limits() {
    new_test_ext().execute_with(|| {
        let s = RuntimeOrigin::signed(SELLER);
        let h = Swap::hash_of(&secret(1));
        let o = H256([1; 32]);
        assert_noop!(
            Swap::lock(s.clone(), o, 1, h, BUYER, 80 * ICT, T0 + 12 * 3600 + 1),
            Error::<Test>::BadExpiry
        );
        assert_noop!(
            Swap::lock(s.clone(), o, 1, h, BUYER, 200 * ICT, T0 + 3600),
            Error::<Test>::OverCap
        );
        assert_ok!(Swap::lock(s.clone(), o, 1, h, BUYER, 80 * ICT, T0 + 3600));
        assert_noop!(
            Swap::lock(s, o, 1, h, BUYER, 80 * ICT, T0 + 3600),
            Error::<Test>::LockExists
        );
    });
}

// ---------------------------------------------------------------- unsigned transactions

#[test]
fn unsigned_claims_are_valid_only_when_they_would_succeed() {
    new_test_ext().execute_with(|| {
        let order = place(100 * ICT);
        assert_ok!(Swap::reserve(
            RuntimeOrigin::signed(KEEPER),
            order,
            0,
            40 * ICT,
            BUYER
        ));
        let good = crate::Call::<Test>::claim_reservation {
            order,
            fill: 0,
            secret: secret(0),
        };
        let bad = crate::Call::<Test>::claim_reservation {
            order,
            fill: 0,
            secret: secret(9),
        };
        let early = crate::Call::<Test>::expire_reservation { order, fill: 0 };
        let other = crate::Call::<Test>::cancel_sell { order };
        assert!(Swap::validate_unsigned(TransactionSource::External, &good).is_ok());
        assert_eq!(
            Swap::validate_unsigned(TransactionSource::External, &bad),
            InvalidTransaction::Call.into()
        );
        assert_eq!(
            Swap::validate_unsigned(TransactionSource::External, &early),
            InvalidTransaction::Call.into()
        );
        assert_eq!(
            Swap::validate_unsigned(TransactionSource::External, &other),
            InvalidTransaction::Call.into()
        );
        advance(19 * 3600);
        assert!(Swap::validate_unsigned(TransactionSource::External, &early).is_ok());
    });
}

#[test]
fn signed_maker_calls_cannot_be_unsigned() {
    new_test_ext().execute_with(|| {
        assert!(Swap::place_sell(
            RuntimeOrigin::none(),
            1,
            50 * ICT,
            PRICE,
            BASE_RECIPIENT,
            T0 + 3600,
            hashes()
        )
        .is_err());
        let order = place(100 * ICT);
        assert!(Swap::reserve(RuntimeOrigin::none(), order, 0, 40 * ICT, BUYER).is_err());
        assert!(Swap::cancel_sell(RuntimeOrigin::none(), order).is_err());
    });
}

// ---------------------------------------------------------------- conservation

#[test]
fn ict_is_conserved_over_many_fills() {
    new_test_ext().execute_with(|| {
        let order = place(100 * ICT);
        let total_before = Balances::total_issuance();
        for fill in 0u8..5 {
            assert_ok!(Swap::reserve(
                RuntimeOrigin::signed(KEEPER),
                order,
                fill,
                20 * ICT,
                BUYER
            ));
            if fill % 2 == 0 {
                assert_ok!(Swap::claim_reservation(
                    RuntimeOrigin::none(),
                    order,
                    fill,
                    secret(fill)
                ));
            } else {
                advance(19 * 3600);
                assert_ok!(Swap::expire_reservation(RuntimeOrigin::none(), order, fill));
                set_time(T0);
            }
        }
        advance(2 * DAY);
        assert_ok!(Swap::refund_expired(RuntimeOrigin::none(), order));
        assert_eq!(held(SELLER), 0);
        assert_eq!(Balances::total_issuance(), total_before);
        assert_eq!(Balances::total_balance(&BUYER), 10_060 * ICT);
        assert_eq!(Balances::total_balance(&SELLER), 9_940 * ICT);
    });
}
