//! # ICT swap pallet (Incentium Swap Technical Specification v0.1, Section 7)
//!
//! ICT side of ICT <-> USDC atomic swaps with Base. Mirrors the `IncentiumSwap` contract:
//!
//! - **Sell orders** hold ICT in the maker's own account and commit 16 SHA-256 hashes. A buyer
//!   (or a keeper) reserves fill `i` for the buyer's account; the buyer then locks USDC on Base
//!   under hash `i`. The seller confirms by revealing secret `i` on Base, and anyone then claims
//!   the reservation here with the same secret.
//! - **Locks** are made by sellers who fill a buy order that lives on Base: ICT is held under that
//!   order's hash and paid to the buy maker's account when the secret is revealed here.
//!
//! Funds only ever move to an account fixed when the order, reservation or lock was made, and only
//! with the secret (before the deadline) or after the deadline. Claims, expiries and refunds can be
//! sent unsigned, so nobody needs ICT to finish a swap.
//!
//! **Deadlines across a halt (review finding H1).** Deadlines use chain time (`UnixTime`). While the
//! chain is halted no blocks are made, but the first block afterwards carries the real wall-clock
//! time, so every deadline that fell inside the halt would pass at once. For a reservation on a sell
//! order the claim here is the *second* leg: the seller may already have revealed the secret on Base.
//! Reservation deadlines are therefore extended by the chain's downtime since the reservation was
//! made (`Downtime`, gaps between blocks above `MaxBlockGap`). Locks are the *first* leg (the secret
//! is revealed here, then used on Base), so their deadlines stay on plain chain time.
//!
//! **Reservation deposits (review finding H2).** Whoever reserves a fill holds a deposit
//! (`ReservationDepositPpm` of the quantity, at least `MinReservationDeposit`). It is returned when
//! the reservation is claimed and burned when it expires unclaimed. Nobody gains from a trade that
//! did not complete, so no one has to judge which side failed: there is no arbiter and no
//! privileged account in this pallet. The `IncentiumSwap` contract on Base (version 2) applies the
//! same rule to its USDC deposits. One open reservation per recipient per order.
#![cfg_attr(not(feature = "std"), no_std)]

pub use pallet::*;
pub mod weights;
pub use weights::WeightInfo;

#[cfg(test)]
mod mock;
#[cfg(test)]
mod tests;

/// Fills per order (committed hashes).
pub const MAX_FILLS: usize = 16;
/// ICT has 9 decimals.
pub const ICT_UNIT: u128 = 1_000_000_000;

#[polkadot_sdk::frame_support::pallet]
pub mod pallet {
    use super::{WeightInfo, ICT_UNIT, MAX_FILLS};
    use codec::{Decode, Encode, MaxEncodedLen};
    use polkadot_sdk::frame_support::{
        pallet_prelude::*,
        traits::{
            fungible::{InspectHold, Mutate, MutateHold},
            tokens::{Fortitude, Precision, Restriction},
            UnixTime,
        },
    };
    use polkadot_sdk::frame_system::{self, pallet_prelude::*};
    use polkadot_sdk::sp_core::{H160, H256};
    use polkadot_sdk::sp_io::hashing::{blake2_256, sha2_256};
    use polkadot_sdk::sp_runtime::{traits::SaturatedConversion, DispatchError};
    use scale_info::TypeInfo;
    extern crate alloc;

    #[pallet::pallet]
    pub struct Pallet<T>(_);

    #[pallet::composite_enum]
    pub enum HoldReason {
        /// ICT held for an open sell order, a reservation on it, or a taker lock.
        Swap,
    }

    #[pallet::config]
    pub trait Config: polkadot_sdk::frame_system::Config {
        #[allow(deprecated)]
        type RuntimeEvent: From<Event<Self>>
            + IsType<<Self as polkadot_sdk::frame_system::Config>::RuntimeEvent>;
        type WeightInfo: WeightInfo;
        type RuntimeHoldReason: From<HoldReason>;
        type Currency: Mutate<Self::AccountId, Balance = u128>
            + MutateHold<Self::AccountId, Balance = u128, Reason = Self::RuntimeHoldReason>
            + InspectHold<Self::AccountId, Balance = u128>;
        /// Chain time (seconds) for deadlines.
        type UnixTime: UnixTime;
        /// Blocks per day, for the order size caps (7,200 at 12-second blocks).
        #[pallet::constant]
        type BlocksPerDay: Get<u32>;
        /// Smallest fill: 5 USDC (6 decimals).
        #[pallet::constant]
        type MinFillUsdc: Get<u128>;
        /// Fill quantities are multiples of this (0.01 ICT).
        #[pallet::constant]
        type QtyStep: Get<u128>;
        /// Open reservations per order (reserved, neither claimed nor expired).
        #[pallet::constant]
        type MaxOpenReservations: Get<u8>;
        /// Longest order lifetime in seconds (24 h).
        #[pallet::constant]
        type OrderTtlMax: Get<u64>;
        /// Reservation lifetime in seconds (19 h = 1 h to lock + 12 h to confirm + 6 h margin).
        #[pallet::constant]
        type ReservationTtl: Get<u64>;
        /// Longest taker lock in seconds (12 h).
        #[pallet::constant]
        type LockTtlMax: Get<u64>;
        /// Reservation deposit as a share of the reserved quantity, in ppm (20,000 = 2 %).
        #[pallet::constant]
        type ReservationDepositPpm: Get<u32>;
        /// Smallest reservation deposit in ICT base units (2 ICT).
        #[pallet::constant]
        type MinReservationDeposit: Get<u128>;
        /// Gaps between blocks longer than this many seconds count as downtime (120 s).
        #[pallet::constant]
        type MaxBlockGap: Get<u64>;
    }

    #[derive(Encode, Decode, MaxEncodedLen, TypeInfo, Clone, Copy, PartialEq, Eq, Debug)]
    pub enum State {
        Active,
        Claimed,
        Expired,
    }

    #[derive(Encode, Decode, MaxEncodedLen, TypeInfo, Clone, PartialEq, Eq, Debug)]
    pub struct SellOrder<AccountId> {
        pub maker: AccountId,
        /// Base address that receives the USDC.
        pub base_recipient: H160,
        pub max_qty: u128,
        /// USDC base units per 1 ICT.
        pub price: u128,
        pub remaining: u128,
        pub expiry: u64,
        pub open: bool,
        pub open_reservations: u8,
        /// Bit i set: hash i has been taken by a reservation.
        pub used_fills: u16,
    }

    #[derive(Encode, Decode, MaxEncodedLen, TypeInfo, Clone, PartialEq, Eq, Debug)]
    pub struct Reservation<AccountId> {
        pub state: State,
        /// The buyer's account, receives the ICT.
        pub recipient: AccountId,
        pub qty: u128,
        /// USDC the buyer owes on Base (for display and checks off chain).
        pub usdc: u128,
        pub expiry: u64,
    }

    #[derive(Encode, Decode, MaxEncodedLen, TypeInfo, Clone, PartialEq, Eq, Debug)]
    pub struct LockInfo<AccountId> {
        pub state: State,
        /// Seller, refunded after expiry.
        pub taker: AccountId,
        /// The buy maker's account, receives the ICT.
        pub payee: AccountId,
        pub qty: u128,
        pub hash: H256,
        /// Buy order on Base and its fill index.
        pub base_order: H256,
        pub fill: u8,
        pub expiry: u64,
    }

    #[pallet::storage]
    pub type SellOrders<T: Config> =
        StorageMap<_, Identity, H256, SellOrder<T::AccountId>, OptionQuery>;

    #[pallet::storage]
    pub type OrderHashes<T> = StorageMap<_, Identity, H256, [H256; MAX_FILLS], OptionQuery>;

    #[pallet::storage]
    pub type Reservations<T: Config> = StorageDoubleMap<
        _,
        Identity,
        H256,
        Twox64Concat,
        u8,
        Reservation<T::AccountId>,
        OptionQuery,
    >;

    #[pallet::storage]
    pub type Locks<T: Config> = StorageMap<_, Identity, H256, LockInfo<T::AccountId>, OptionQuery>;

    /// What happens to a reservation's deposit.
    #[derive(Encode, Decode, MaxEncodedLen, TypeInfo, Clone, Copy, PartialEq, Eq, Debug)]
    pub enum DepositState {
        /// Held while the reservation is active.
        Held,
        /// Returned to its payer: the reservation was claimed.
        Returned,
        /// Burned: the reservation expired unclaimed.
        Burned,
    }

    /// Terms of a reservation added by review fixes H1 and H2. Reservations made before the upgrade
    /// have none: no deposit, and no downtime credit before the upgrade.
    #[derive(Encode, Decode, MaxEncodedLen, TypeInfo, Clone, PartialEq, Eq, Debug)]
    pub struct ReservationTerms<AccountId> {
        /// Who paid the deposit (the taker, or a keeper on the taker's behalf).
        pub depositor: AccountId,
        pub deposit: u128,
        pub deposit_state: DepositState,
        /// `Downtime` when the reservation was made.
        pub downtime_at_start: u64,
    }

    #[pallet::storage]
    pub type Terms<T: Config> = StorageDoubleMap<
        _,
        Identity,
        H256,
        Twox64Concat,
        u8,
        ReservationTerms<T::AccountId>,
        OptionQuery,
    >;

    /// Open (active) reservation per (order, recipient): at most one.
    #[pallet::storage]
    pub type OpenByRecipient<T: Config> =
        StorageDoubleMap<_, Identity, H256, Blake2_128Concat, T::AccountId, u8, OptionQuery>;

    /// Seconds the chain has been down in total (gaps between blocks above `MaxBlockGap`, minus
    /// `MaxBlockGap`), counted from the upgrade that introduced it.
    #[pallet::storage]
    pub type Downtime<T> = StorageValue<_, u64, ValueQuery>;

    /// Chain time of the last block, for `Downtime`.
    #[pallet::storage]
    pub type LastBlockTime<T> = StorageValue<_, u64, ValueQuery>;

    #[pallet::event]
    #[pallet::generate_deposit(pub(super) fn deposit_event)]
    pub enum Event<T: Config> {
        SellPlaced {
            order: H256,
            maker: T::AccountId,
            base_recipient: H160,
            max_qty: u128,
            price: u128,
            expiry: u64,
        },
        SellClosed {
            order: H256,
            released: u128,
        },
        Reserved {
            order: H256,
            fill: u8,
            recipient: T::AccountId,
            qty: u128,
            usdc: u128,
            expiry: u64,
        },
        ReservationClaimed {
            order: H256,
            fill: u8,
            secret: H256,
        },
        ReservationExpired {
            order: H256,
            fill: u8,
        },
        Locked {
            lock: H256,
            base_order: H256,
            fill: u8,
            taker: T::AccountId,
            payee: T::AccountId,
            hash: H256,
            qty: u128,
            expiry: u64,
        },
        LockClaimed {
            lock: H256,
            secret: H256,
        },
        LockRefunded {
            lock: H256,
        },
        /// A reservation deposit was held from `depositor`.
        DepositHeld {
            order: H256,
            fill: u8,
            depositor: T::AccountId,
            deposit: u128,
        },
        /// A deposit went back to its payer (the reservation was claimed).
        DepositReturned {
            order: H256,
            fill: u8,
            amount: u128,
        },
        /// The deposit of a reservation that expired unclaimed was burned.
        DepositBurned {
            order: H256,
            fill: u8,
            amount: u128,
        },
    }

    #[pallet::error]
    pub enum Error<T> {
        BadQuantity,
        BadPrice,
        BadExpiry,
        OverCap,
        TooSmall,
        OrderExists,
        OrderNotOpen,
        OrderExpired,
        NotMaker,
        FillTaken,
        TooManyOpenReservations,
        NotActive,
        WrongSecret,
        TooLate,
        TooEarly,
        LockExists,
        UnknownItem,
        /// The recipient already has an open reservation on this order.
        RecipientHasOpenReservation,
    }

    // ------------------------------------------------------------------ hooks

    #[pallet::hooks]
    impl<T: Config> Hooks<BlockNumberFor<T>> for Pallet<T> {
        fn on_initialize(_n: BlockNumberFor<T>) -> Weight {
            // Work done in `on_finalize`: two reads, two writes.
            T::DbWeight::get().reads_writes(2, 2)
        }

        fn on_finalize(_n: BlockNumberFor<T>) {
            Downtime::<T>::put(Self::downtime_now());
            LastBlockTime::<T>::put(Self::now());
        }
    }

    // ------------------------------------------------------------------ calls

    #[pallet::call]
    impl<T: Config> Pallet<T> {
        /// Place a sell order: holds `max_qty` ICT and commits one hash per possible fill.
        #[pallet::call_index(0)]
        #[pallet::weight(T::WeightInfo::place_sell())]
        pub fn place_sell(
            origin: OriginFor<T>,
            nonce: u64,
            max_qty: u128,
            price: u128,
            base_recipient: H160,
            expiry: u64,
            hashes: alloc::boxed::Box<[H256; MAX_FILLS]>,
        ) -> DispatchResult {
            let maker = ensure_signed(origin)?;
            let now = Self::now();
            ensure!(
                max_qty > 0 && max_qty.is_multiple_of(T::QtyStep::get()),
                Error::<T>::BadQuantity
            );
            ensure!(max_qty <= Self::max_order_qty(), Error::<T>::OverCap);
            ensure!(price > 0, Error::<T>::BadPrice);
            ensure!(
                expiry > now && expiry <= now + T::OrderTtlMax::get(),
                Error::<T>::BadExpiry
            );
            ensure!(
                Self::usdc_for(max_qty, price) >= T::MinFillUsdc::get(),
                Error::<T>::TooSmall
            );
            let order = Self::order_id(&maker, nonce);
            ensure!(
                !SellOrders::<T>::contains_key(order),
                Error::<T>::OrderExists
            );

            T::Currency::hold(&HoldReason::Swap.into(), &maker, max_qty)?;
            SellOrders::<T>::insert(
                order,
                SellOrder {
                    maker: maker.clone(),
                    base_recipient,
                    max_qty,
                    price,
                    remaining: max_qty,
                    expiry,
                    open: true,
                    open_reservations: 0,
                    used_fills: 0,
                },
            );
            OrderHashes::<T>::insert(order, *hashes);
            Self::deposit_event(Event::SellPlaced {
                order,
                maker,
                base_recipient,
                max_qty,
                price,
                expiry,
            });
            Ok(())
        }

        /// Cancel: releases the unreserved remainder. Open reservations keep their deadlines.
        #[pallet::call_index(1)]
        #[pallet::weight(T::WeightInfo::close())]
        pub fn cancel_sell(origin: OriginFor<T>, order: H256) -> DispatchResult {
            let who = ensure_signed(origin)?;
            let o = SellOrders::<T>::get(order).ok_or(Error::<T>::UnknownItem)?;
            ensure!(o.maker == who, Error::<T>::NotMaker);
            ensure!(o.open, Error::<T>::OrderNotOpen);
            Self::close(order, o)
        }

        /// After expiry anyone (signed or unsigned) can release the unreserved remainder.
        #[pallet::call_index(2)]
        #[pallet::weight(T::WeightInfo::close())]
        pub fn refund_expired(origin: OriginFor<T>, order: H256) -> DispatchResult {
            Self::ensure_any(origin)?;
            let o = Self::check_refund_expired(order)?;
            Self::close(order, o)
        }

        /// Reserve `qty` of sell order `order` as fill `fill` for `recipient` (the buyer).
        #[pallet::call_index(3)]
        #[pallet::weight(T::WeightInfo::reserve())]
        pub fn reserve(
            origin: OriginFor<T>,
            order: H256,
            fill: u8,
            qty: u128,
            recipient: T::AccountId,
        ) -> DispatchResult {
            let depositor = ensure_signed(origin)?;
            let now = Self::now();
            let mut o = SellOrders::<T>::get(order).ok_or(Error::<T>::UnknownItem)?;
            ensure!(o.open, Error::<T>::OrderNotOpen);
            ensure!(now < o.expiry, Error::<T>::OrderExpired);
            ensure!(
                (fill as usize) < MAX_FILLS && o.used_fills & (1u16 << fill) == 0,
                Error::<T>::FillTaken
            );
            ensure!(
                o.open_reservations < T::MaxOpenReservations::get(),
                Error::<T>::TooManyOpenReservations
            );
            ensure!(
                qty > 0 && qty.is_multiple_of(T::QtyStep::get()) && qty <= o.remaining,
                Error::<T>::BadQuantity
            );
            let usdc = Self::usdc_for(qty, o.price);
            ensure!(usdc >= T::MinFillUsdc::get(), Error::<T>::TooSmall);
            ensure!(
                !OpenByRecipient::<T>::contains_key(order, &recipient),
                Error::<T>::RecipientHasOpenReservation
            );
            let deposit = Self::deposit_for(qty);
            T::Currency::hold(&HoldReason::Swap.into(), &depositor, deposit)?;

            o.used_fills |= 1u16 << fill;
            o.open_reservations += 1;
            o.remaining -= qty;
            SellOrders::<T>::insert(order, o);
            let expiry = now + T::ReservationTtl::get();
            Reservations::<T>::insert(
                order,
                fill,
                Reservation {
                    state: State::Active,
                    recipient: recipient.clone(),
                    qty,
                    usdc,
                    expiry,
                },
            );
            OpenByRecipient::<T>::insert(order, &recipient, fill);
            Terms::<T>::insert(
                order,
                fill,
                ReservationTerms {
                    depositor: depositor.clone(),
                    deposit,
                    deposit_state: DepositState::Held,
                    downtime_at_start: Self::downtime_now(),
                },
            );
            Self::deposit_event(Event::Reserved {
                order,
                fill,
                recipient,
                qty,
                usdc,
                expiry,
            });
            Self::deposit_event(Event::DepositHeld {
                order,
                fill,
                depositor,
                deposit,
            });
            Ok(())
        }

        /// Pay a reservation to the buyer with the secret the seller revealed on Base.
        #[pallet::call_index(4)]
        #[pallet::weight(T::WeightInfo::claim())]
        pub fn claim_reservation(
            origin: OriginFor<T>,
            order: H256,
            fill: u8,
            secret: H256,
        ) -> DispatchResult {
            Self::ensure_any(origin)?;
            let mut r = Self::check_claim_reservation(order, fill, &secret)?;
            let mut o = SellOrders::<T>::get(order).ok_or(Error::<T>::UnknownItem)?;
            T::Currency::transfer_on_hold(
                &HoldReason::Swap.into(),
                &o.maker,
                &r.recipient,
                r.qty,
                Precision::Exact,
                Restriction::Free,
                Fortitude::Polite,
            )?;
            r.state = State::Claimed;
            OpenByRecipient::<T>::remove(order, &r.recipient);
            Reservations::<T>::insert(order, fill, r);
            o.open_reservations = o.open_reservations.saturating_sub(1);
            SellOrders::<T>::insert(order, o);
            Self::deposit_event(Event::ReservationClaimed {
                order,
                fill,
                secret,
            });
            if let Some(mut t) = Terms::<T>::get(order, fill) {
                if t.deposit_state == DepositState::Held {
                    Self::do_return_deposit(order, fill, &mut t)?;
                }
            }
            Ok(())
        }

        /// After its deadline: the quantity goes back to the order, or is released to the maker
        /// if the order is closed or expired. The reservation's deposit is burned.
        #[pallet::call_index(5)]
        #[pallet::weight(T::WeightInfo::expire())]
        pub fn expire_reservation(origin: OriginFor<T>, order: H256, fill: u8) -> DispatchResult {
            Self::ensure_any(origin)?;
            let mut r = Self::check_expire_reservation(order, fill)?;
            let mut o = SellOrders::<T>::get(order).ok_or(Error::<T>::UnknownItem)?;
            o.open_reservations = o.open_reservations.saturating_sub(1);
            if o.open && Self::now() < o.expiry {
                o.remaining += r.qty;
            } else {
                T::Currency::release(&HoldReason::Swap.into(), &o.maker, r.qty, Precision::Exact)?;
            }
            r.state = State::Expired;
            OpenByRecipient::<T>::remove(order, &r.recipient);
            Reservations::<T>::insert(order, fill, r);
            SellOrders::<T>::insert(order, o);
            if let Some(mut t) = Terms::<T>::get(order, fill) {
                if t.deposit_state == DepositState::Held {
                    T::Currency::burn_held(
                        &HoldReason::Swap.into(),
                        &t.depositor,
                        t.deposit,
                        Precision::Exact,
                        Fortitude::Force,
                    )?;
                    t.deposit_state = DepositState::Burned;
                    let amount = t.deposit;
                    Terms::<T>::insert(order, fill, t);
                    Self::deposit_event(Event::DepositBurned {
                        order,
                        fill,
                        amount,
                    });
                }
            }
            Self::deposit_event(Event::ReservationExpired { order, fill });
            Ok(())
        }

        /// Fill a buy order that lives on Base: hold `qty` ICT under that fill's hash, payable to
        /// `payee` (the buy maker's account). Only lock after the reservation exists on Base.
        #[pallet::call_index(6)]
        #[pallet::weight(T::WeightInfo::lock())]
        pub fn lock(
            origin: OriginFor<T>,
            base_order: H256,
            fill: u8,
            hash: H256,
            payee: T::AccountId,
            qty: u128,
            expiry: u64,
        ) -> DispatchResult {
            let taker = ensure_signed(origin)?;
            let now = Self::now();
            ensure!(
                qty > 0 && qty.is_multiple_of(T::QtyStep::get()),
                Error::<T>::BadQuantity
            );
            ensure!(qty <= Self::max_order_qty(), Error::<T>::OverCap);
            ensure!(
                expiry > now && expiry <= now + T::LockTtlMax::get(),
                Error::<T>::BadExpiry
            );
            ensure!(hash != H256::zero(), Error::<T>::WrongSecret);
            let lock = Self::lock_id(&taker, base_order, fill);
            ensure!(!Locks::<T>::contains_key(lock), Error::<T>::LockExists);

            T::Currency::hold(&HoldReason::Swap.into(), &taker, qty)?;
            Locks::<T>::insert(
                lock,
                LockInfo {
                    state: State::Active,
                    taker: taker.clone(),
                    payee: payee.clone(),
                    qty,
                    hash,
                    base_order,
                    fill,
                    expiry,
                },
            );
            Self::deposit_event(Event::Locked {
                lock,
                base_order,
                fill,
                taker,
                payee,
                hash,
                qty,
                expiry,
            });
            Ok(())
        }

        /// The buy maker's confirmation: reveals the secret and pays the ICT to the maker.
        #[pallet::call_index(7)]
        #[pallet::weight(T::WeightInfo::claim())]
        pub fn claim_lock(origin: OriginFor<T>, lock: H256, secret: H256) -> DispatchResult {
            Self::ensure_any(origin)?;
            let mut l = Self::check_claim_lock(lock, &secret)?;
            T::Currency::transfer_on_hold(
                &HoldReason::Swap.into(),
                &l.taker,
                &l.payee,
                l.qty,
                Precision::Exact,
                Restriction::Free,
                Fortitude::Polite,
            )?;
            l.state = State::Claimed;
            Locks::<T>::insert(lock, l);
            Self::deposit_event(Event::LockClaimed { lock, secret });
            Ok(())
        }

        /// After the deadline the ICT is released back to the seller.
        #[pallet::call_index(8)]
        #[pallet::weight(T::WeightInfo::expire())]
        pub fn refund_lock(origin: OriginFor<T>, lock: H256) -> DispatchResult {
            Self::ensure_any(origin)?;
            let mut l = Self::check_refund_lock(lock)?;
            T::Currency::release(&HoldReason::Swap.into(), &l.taker, l.qty, Precision::Exact)?;
            l.state = State::Expired;
            Locks::<T>::insert(lock, l);
            Self::deposit_event(Event::LockRefunded { lock });
            Ok(())
        }
    }

    // ------------------------------------------------------------------ unsigned claims and refunds

    // `ValidateUnsigned` is deprecated in favour of `#[pallet::authorize]` (removal after April
    // 2027). It is kept here because the runtime's transaction extensions do not include
    // `AuthorizeCall` yet; move both together.
    #[allow(deprecated)]
    #[pallet::validate_unsigned]
    impl<T: Config> ValidateUnsigned for Pallet<T> {
        type Call = Call<T>;

        fn validate_unsigned(_source: TransactionSource, call: &Self::Call) -> TransactionValidity {
            let tag: (u8, H256, u8) = match call {
                Call::claim_reservation {
                    order,
                    fill,
                    secret,
                } => {
                    Self::check_claim_reservation(*order, *fill, secret)
                        .map_err(|_| InvalidTransaction::Call)?;
                    (0, *order, *fill)
                }
                Call::expire_reservation { order, fill } => {
                    Self::check_expire_reservation(*order, *fill)
                        .map_err(|_| InvalidTransaction::Call)?;
                    (0, *order, *fill)
                }
                Call::claim_lock { lock, secret } => {
                    Self::check_claim_lock(*lock, secret).map_err(|_| InvalidTransaction::Call)?;
                    (1, *lock, 0)
                }
                Call::refund_lock { lock } => {
                    Self::check_refund_lock(*lock).map_err(|_| InvalidTransaction::Call)?;
                    (1, *lock, 0)
                }
                Call::refund_expired { order } => {
                    Self::check_refund_expired(*order).map_err(|_| InvalidTransaction::Call)?;
                    (2, *order, 0)
                }
                _ => return InvalidTransaction::Call.into(),
            };
            ValidTransaction::with_tag_prefix("IctSwap")
                .priority(1)
                .and_provides(tag)
                .longevity(64)
                .propagate(true)
                .build()
        }
    }

    // ------------------------------------------------------------------ helpers

    impl<T: Config> Pallet<T> {
        pub fn now() -> u64 {
            T::UnixTime::now().as_secs()
        }

        /// Largest order or lock now: 100 ICT until day 90, 1,000 ICT until day 120, 5,000 ICT after.
        pub fn max_order_qty() -> u128 {
            let block: u64 = frame_system::Pallet::<T>::block_number().saturated_into();
            let day = block / (T::BlocksPerDay::get().max(1) as u64);
            if day < 90 {
                100 * ICT_UNIT
            } else if day < 120 {
                1_000 * ICT_UNIT
            } else {
                5_000 * ICT_UNIT
            }
        }

        pub fn usdc_for(qty: u128, price: u128) -> u128 {
            qty.saturating_mul(price) / ICT_UNIT
        }

        /// Deposit for reserving `qty`: `ReservationDepositPpm` of it, at least the minimum.
        pub fn deposit_for(qty: u128) -> u128 {
            (qty.saturating_mul(T::ReservationDepositPpm::get() as u128) / 1_000_000)
                .max(T::MinReservationDeposit::get())
        }

        /// Downtime so far, including a gap still open since the last block (so the first block
        /// after a halt already counts it).
        pub fn downtime_now() -> u64 {
            let last = LastBlockTime::<T>::get();
            let now = Self::now();
            let gap = now.saturating_sub(last);
            let extra = if last > 0 && gap > T::MaxBlockGap::get() {
                gap - T::MaxBlockGap::get()
            } else {
                0
            };
            Downtime::<T>::get().saturating_add(extra)
        }

        /// A reservation's deadline, extended by the downtime since it was made.
        pub fn reservation_deadline(order: H256, fill: u8, expiry: u64) -> u64 {
            let credit = Terms::<T>::get(order, fill)
                .map(|t| Self::downtime_now().saturating_sub(t.downtime_at_start))
                .unwrap_or_else(Self::downtime_now);
            expiry.saturating_add(credit)
        }

        fn do_return_deposit(
            order: H256,
            fill: u8,
            t: &mut ReservationTerms<T::AccountId>,
        ) -> DispatchResult {
            T::Currency::release(
                &HoldReason::Swap.into(),
                &t.depositor,
                t.deposit,
                Precision::Exact,
            )?;
            t.deposit_state = DepositState::Returned;
            let amount = t.deposit;
            Terms::<T>::insert(order, fill, t.clone());
            Self::deposit_event(Event::DepositReturned {
                order,
                fill,
                amount,
            });
            Ok(())
        }

        pub fn order_id(maker: &T::AccountId, nonce: u64) -> H256 {
            H256(blake2_256(&(maker, nonce).encode()))
        }

        pub fn lock_id(taker: &T::AccountId, base_order: H256, fill: u8) -> H256 {
            H256(blake2_256(&(taker, base_order, fill).encode()))
        }

        pub fn hash_of(secret: &H256) -> H256 {
            H256(sha2_256(secret.as_bytes()))
        }

        fn ensure_any(origin: OriginFor<T>) -> DispatchResult {
            match origin.into() {
                Ok(frame_system::RawOrigin::Signed(_)) | Ok(frame_system::RawOrigin::None) => {
                    Ok(())
                }
                _ => Err(DispatchError::BadOrigin),
            }
        }

        fn close(order: H256, mut o: SellOrder<T::AccountId>) -> DispatchResult {
            let released = o.remaining;
            if released > 0 {
                T::Currency::release(
                    &HoldReason::Swap.into(),
                    &o.maker,
                    released,
                    Precision::Exact,
                )?;
            }
            o.open = false;
            o.remaining = 0;
            SellOrders::<T>::insert(order, o);
            Self::deposit_event(Event::SellClosed { order, released });
            Ok(())
        }

        fn check_refund_expired(order: H256) -> Result<SellOrder<T::AccountId>, DispatchError> {
            let o = SellOrders::<T>::get(order).ok_or(Error::<T>::UnknownItem)?;
            ensure!(o.open, Error::<T>::OrderNotOpen);
            ensure!(Self::now() >= o.expiry, Error::<T>::TooEarly);
            Ok(o)
        }

        fn check_claim_reservation(
            order: H256,
            fill: u8,
            secret: &H256,
        ) -> Result<Reservation<T::AccountId>, DispatchError> {
            let r = Reservations::<T>::get(order, fill).ok_or(Error::<T>::UnknownItem)?;
            ensure!(r.state == State::Active, Error::<T>::NotActive);
            ensure!(
                Self::now() < Self::reservation_deadline(order, fill, r.expiry),
                Error::<T>::TooLate
            );
            let hashes = OrderHashes::<T>::get(order).ok_or(Error::<T>::UnknownItem)?;
            ensure!(
                Self::hash_of(secret) == hashes[fill as usize],
                Error::<T>::WrongSecret
            );
            Ok(r)
        }

        fn check_expire_reservation(
            order: H256,
            fill: u8,
        ) -> Result<Reservation<T::AccountId>, DispatchError> {
            let r = Reservations::<T>::get(order, fill).ok_or(Error::<T>::UnknownItem)?;
            ensure!(r.state == State::Active, Error::<T>::NotActive);
            ensure!(
                Self::now() >= Self::reservation_deadline(order, fill, r.expiry),
                Error::<T>::TooEarly
            );
            Ok(r)
        }

        fn check_claim_lock(
            lock: H256,
            secret: &H256,
        ) -> Result<LockInfo<T::AccountId>, DispatchError> {
            let l = Locks::<T>::get(lock).ok_or(Error::<T>::UnknownItem)?;
            ensure!(l.state == State::Active, Error::<T>::NotActive);
            ensure!(Self::now() < l.expiry, Error::<T>::TooLate);
            ensure!(Self::hash_of(secret) == l.hash, Error::<T>::WrongSecret);
            Ok(l)
        }

        fn check_refund_lock(lock: H256) -> Result<LockInfo<T::AccountId>, DispatchError> {
            let l = Locks::<T>::get(lock).ok_or(Error::<T>::UnknownItem)?;
            ensure!(l.state == State::Active, Error::<T>::NotActive);
            ensure!(Self::now() >= l.expiry, Error::<T>::TooEarly);
            Ok(l)
        }
    }
}
