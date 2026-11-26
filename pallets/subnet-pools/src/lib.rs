//! # Subnet pools (spec Section 7, decided 25 and 26 Nov 2026)
//!
//! Every subnet other than SN0 has an Alpha token with ICT's tokenomics and one pool in
//! which staking and trading are the same thing. Alpha pays its holders ICT; nothing in
//! this pallet buys Alpha back.
//!
//! - **Issuance.** No Alpha exists when a subnet opens; then exactly ICT's schedule
//!   (7,200 a day, halving, 21,000,000) counted from the subnet's own first block.
//! - **Bonus and stock.** Each epoch's Alpha is split by the subnet's `bonus_ppm`: the
//!   bonus goes to its miners, validators and owner 50/38/12 in proportion to the ICT they
//!   were paid (the owner's part vests over 180 days); the rest is stock, which waits.
//! - **Closed for 10 days.** The pool holds the auction ICT and collects stock; nothing can
//!   be bought or sold. The held ICT counts as the subnet's stake.
//! - **Opening.** After 10 days the pool opens with real reserves only, at the start price
//!   of the valuation formula: ICT = min(held ICT, 36,000 Alpha at that price), Alpha =
//!   ICT / price. Held ICT the pool cannot match goes to the treasury.
//! - **Refilling.** Stock is released block by block, held back while the price is more
//!   than 20 % under its 30-day average.
//! - **Staking is buying.** `stake` swaps ICT for Alpha and credits it as stake on a
//!   hotkey; `unstake` sells it back. Both carry a price limit; no partial fills.
//! - **Dividends.** The validators' ICT is paid through `pay_validator_ict`: the
//!   validator keeps its take (0–18 %), the rest goes to the Alpha staked on it, pro rata,
//!   with a per-Alpha accumulator so the cost does not grow with the number of stakers.
//! - **Nobody owns the pool.** Its ICT sits on an account derived from the netuid with no
//!   key. Only trades move it.
//! - **Removal.** No more Alpha; waiting stock and the owner's unvested Alpha are burned;
//!   trading goes on.
#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

pub use pallet::*;

#[cfg(test)]
mod mock;
#[cfg(test)]
mod tests;

pub type NetUid = u16;

/// What the registry tells this pallet about subnets.
pub trait SubnetInfo<AccountId> {
    /// The subnet exists and is not SN0.
    fn exists(netuid: NetUid) -> bool;
    /// The account that receives the owner's Alpha (owner key, multisig or DAO account).
    fn owner(netuid: NetUid) -> Option<AccountId>;
    /// `hotkey` is registered on the subnet.
    fn is_neuron(netuid: NetUid, hotkey: &AccountId) -> bool;
    /// Published name of the subnet.
    fn name(netuid: NetUid) -> alloc::vec::Vec<u8>;
}

/// What the start-price formula needs to know about ICT issuance.
pub trait IssuanceInfo {
    /// ICT a day the average subnet other than SN0 receives now (base units).
    fn subnet_ict_per_day() -> u128;
    /// Days until ICT's next halving.
    fn days_to_ict_halving() -> u64;
}

/// What the registry and consensus call on this pallet.
pub trait PoolsInterface<AccountId> {
    /// A subnet was sold: register its pool, closed, holding `reserved_ict` moved from
    /// `from` (the treasury half of the sale price).
    fn register_pool(netuid: NetUid, from: &AccountId, reserved_ict: u128) -> polkadot_sdk::sp_runtime::DispatchResult;
    /// The subnet is removed: no more Alpha; trading goes on.
    fn close_pool(netuid: NetUid);
    /// One settled epoch of `blocks` blocks: issue Alpha and pay the bonus in proportion
    /// to the ICT paid to each miner and validator.
    fn on_epoch(
        netuid: NetUid,
        blocks: u32,
        miners: &[(AccountId, u128)],
        validators: &[(AccountId, u128)],
    );
    /// Pay `amount` ICT from `source` for validator `hotkey`: its take to the hotkey,
    /// the rest to the Alpha staked on it.
    fn pay_validator_ict(
        netuid: NetUid,
        source: &AccountId,
        hotkey: &AccountId,
        amount: u128,
    ) -> polkadot_sdk::sp_runtime::DispatchResult;
    /// ICT that counts as the subnet's stake: the pool's ICT (the held ICT while closed).
    fn stake_ict(netuid: NetUid) -> u128;
    /// Alpha staked on `hotkey` in `netuid` (validator weight).
    fn alpha_stake(netuid: NetUid, hotkey: &AccountId) -> u128;
    /// All Alpha staked in `netuid`.
    fn total_alpha_stake(netuid: NetUid) -> u128;
}

#[polkadot_sdk::frame_support::pallet]
pub mod pallet {
    use super::*;
    use alloc::vec::Vec;
    use ict_economics::{
        alpha::{self, AlphaPool, Fees, Issuance, PoolError},
        emission::{self, EmissionState},
        mul_div, BLOCKS_PER_DAY, PPM,
    };
    use polkadot_sdk::frame_support::{
        pallet_prelude::*,
        traits::{
            fungible::Mutate,
            tokens::{Fortitude, Precision, Preservation},
        },
        PalletId,
    };
    use polkadot_sdk::frame_system::pallet_prelude::*;
    use polkadot_sdk::sp_runtime::traits::{AccountIdConversion, SaturatedConversion};

    /// Fixed point of the dividend accumulator (ICT base units per Alpha base unit).
    pub const ACC_SCALE: u128 = 1_000_000_000_000_000_000;
    /// A validator may raise its take once per 30 days.
    pub const TAKE_RAISE_BLOCKS: u64 = 30 * BLOCKS_PER_DAY;

    #[pallet::pallet]
    pub struct Pallet<T>(_);

    #[pallet::config]
    pub trait Config: polkadot_sdk::frame_system::Config {
        #[allow(deprecated)]
        type RuntimeEvent: From<Event<Self>>
            + IsType<<Self as polkadot_sdk::frame_system::Config>::RuntimeEvent>;
        type Currency: Mutate<Self::AccountId, Balance = u128>;
        /// Pool accounts are derived from this id and the netuid.
        type PalletId: Get<PalletId>;
        type Subnets: SubnetInfo<Self::AccountId>;
        type Issuance: IssuanceInfo;
        /// Receives held auction ICT a pool cannot match when it opens.
        type Treasury: Get<Self::AccountId>;
        /// Trading fee, parts per million of the input (5,000 = 0.5 %).
        type FeePpm: Get<u32>;
        /// Share of the fee that is burned (500,000 = half).
        type FeeBurnPpm: Get<u32>;
        /// Smallest stake or unstake, so dust cannot clog the pool.
        type MinTrade: Get<u128>;
    }

    /// Pool state, kept in SCALE-friendly form.
    #[derive(Clone, Copy, Encode, Decode, DecodeWithMemTracking, MaxEncodedLen, TypeInfo, Debug, PartialEq, Eq, Default)]
    pub struct PoolState {
        /// ICT in the pool.
        pub x: u128,
        /// Alpha in the pool.
        pub y: u128,
        /// Issued stock waiting to be released.
        pub waiting: u128,
        /// Auction ICT held until the pool opens.
        pub reserved_ict: u128,
        pub trading: bool,
        pub removed: bool,
        /// Alpha issued so far and the halvings it has passed.
        pub issued: u128,
        pub halvings: u8,
        /// Share of each epoch's Alpha paid as a bonus, fixed when the subnet opened.
        pub bonus_ppm: u32,
        /// Block the subnet's pool was registered; the 10 days, vesting and days count from here.
        pub opened: u64,
        /// Bonus paid in total.
        pub bonus_minted: u128,
    }

    impl PoolState {
        pub fn pool(&self) -> AlphaPool {
            AlphaPool {
                x: self.x,
                y: self.y,
                waiting: self.waiting,
                reserved_ict: self.reserved_ict,
                trading: self.trading,
                removed: self.removed,
            }
        }
        pub fn set_pool(&mut self, p: AlphaPool) {
            self.x = p.x;
            self.y = p.y;
            self.waiting = p.waiting;
            self.reserved_ict = p.reserved_ict;
            self.trading = p.trading;
            self.removed = p.removed;
        }
        fn issuance(&self) -> Issuance {
            Issuance {
                state: EmissionState {
                    total_issued: self.issued,
                    halvings: self.halvings,
                },
            }
        }
    }

    /// Per-day figures for the pools page.
    #[derive(Clone, Copy, Encode, Decode, DecodeWithMemTracking, MaxEncodedLen, TypeInfo, Debug, PartialEq, Eq, Default)]
    pub struct DayStats {
        pub day: u64,
        pub bonus: u128,
        /// Stock issued today (to the waiting stock).
        pub stock: u128,
        /// Stock released into the pool today.
        pub released: u128,
    }

    /// One day's owner Alpha, released linearly over 180 days.
    #[derive(Clone, Copy, Encode, Decode, DecodeWithMemTracking, MaxEncodedLen, TypeInfo, Debug, PartialEq, Eq, Default)]
    pub struct Tranche {
        pub start: u64,
        pub total: u128,
        pub claimed: u128,
    }

    /// A validator's take on its stakers' dividends.
    #[derive(Clone, Copy, Encode, Decode, DecodeWithMemTracking, MaxEncodedLen, TypeInfo, Debug, PartialEq, Eq)]
    pub struct TakeInfo {
        pub ppm: u32,
        /// Block of the last raise.
        pub raised_at: u64,
    }

    impl Default for TakeInfo {
        fn default() -> Self {
            TakeInfo { ppm: alpha::MAX_TAKE_PPM as u32, raised_at: 0 }
        }
    }

    /// 180 days of tranches plus a margin.
    pub type MaxTranches = ConstU32<200>;
    pub type AvgDays = ConstU32<{ alpha::AVG_DAYS }>;

    #[pallet::storage]
    pub type Pools<T> = StorageMap<_, Twox64Concat, NetUid, PoolState, OptionQuery>;

    #[pallet::storage]
    pub type Today<T> = StorageMap<_, Twox64Concat, NetUid, DayStats, ValueQuery>;

    /// One price sample a day for the last 30 days, oldest first.
    #[pallet::storage]
    pub type PriceDays<T> = StorageMap<_, Twox64Concat, NetUid, BoundedVec<u128, AvgDays>, ValueQuery>;

    /// Alpha staked by `staker` on `hotkey` in `netuid`.
    #[pallet::storage]
    pub type Positions<T: Config> = StorageNMap<
        _,
        (
            NMapKey<Twox64Concat, NetUid>,
            NMapKey<Blake2_128Concat, T::AccountId>,
            NMapKey<Blake2_128Concat, T::AccountId>,
        ),
        u128,
        ValueQuery,
    >;

    /// Alpha staked on each hotkey (validator weight).
    #[pallet::storage]
    pub type HotkeyStake<T: Config> =
        StorageDoubleMap<_, Twox64Concat, NetUid, Blake2_128Concat, T::AccountId, u128, ValueQuery>;

    /// All Alpha staked in a subnet.
    #[pallet::storage]
    pub type TotalStake<T> = StorageMap<_, Twox64Concat, NetUid, u128, ValueQuery>;

    /// Dividends per Alpha staked on a hotkey, accumulated (ICT base units x ACC_SCALE).
    #[pallet::storage]
    pub type AccPerAlpha<T: Config> =
        StorageDoubleMap<_, Twox64Concat, NetUid, Blake2_128Concat, T::AccountId, u128, ValueQuery>;

    /// What each position has already been credited from the accumulator.
    #[pallet::storage]
    pub type Debt<T: Config> = StorageNMap<
        _,
        (
            NMapKey<Twox64Concat, NetUid>,
            NMapKey<Blake2_128Concat, T::AccountId>,
            NMapKey<Blake2_128Concat, T::AccountId>,
        ),
        u128,
        ValueQuery,
    >;

    /// Validator takes, by hotkey (default 18 %).
    #[pallet::storage]
    pub type Takes<T: Config> = StorageMap<_, Blake2_128Concat, T::AccountId, TakeInfo, ValueQuery>;

    /// Owner Alpha still vesting.
    #[pallet::storage]
    pub type OwnerVesting<T> =
        StorageMap<_, Twox64Concat, NetUid, BoundedVec<Tranche, MaxTranches>, ValueQuery>;

    /// Bonus share for subnets opened from now on (25 %–75 %; default 50 %).
    #[pallet::storage]
    pub type NextBonusPpm<T> = StorageValue<_, u32, ValueQuery, DefaultBonus>;

    #[pallet::type_value]
    pub fn DefaultBonus() -> u32 {
        alpha::DEFAULT_BONUS_PPM as u32
    }

    /// Required return in the start-price formula (default 20 %).
    #[pallet::storage]
    pub type RequiredReturnPpm<T> = StorageValue<_, u32, ValueQuery, DefaultReturn>;

    #[pallet::type_value]
    pub fn DefaultReturn() -> u32 {
        alpha::DEFAULT_REQUIRED_RETURN_PPM as u32
    }

    #[pallet::event]
    #[pallet::generate_deposit(pub(super) fn deposit_event)]
    pub enum Event<T: Config> {
        PoolRegistered { netuid: NetUid, reserved_ict: u128, bonus_ppm: u32 },
        PoolOpened { netuid: NetUid, price: u128, ict: u128, alpha: u128, to_treasury: u128 },
        PoolClosed { netuid: NetUid, burned_stock: u128 },
        Staked { netuid: NetUid, hotkey: T::AccountId, staker: T::AccountId, ict: u128, alpha: u128 },
        Unstaked { netuid: NetUid, hotkey: T::AccountId, staker: T::AccountId, alpha: u128, ict: u128 },
        StakeMoved { netuid: NetUid, staker: T::AccountId, from: T::AccountId, to: T::AccountId, alpha: u128 },
        StakeTransferred { netuid: NetUid, hotkey: T::AccountId, from: T::AccountId, to: T::AccountId, alpha: u128 },
        AlphaIssued { netuid: NetUid, bonus: u128, stock: u128 },
        OwnerAlphaClaimed { netuid: NetUid, owner: T::AccountId, alpha: u128 },
        ValidatorPaid { netuid: NetUid, hotkey: T::AccountId, take: u128, to_stakers: u128 },
        DividendsPaid { netuid: NetUid, hotkey: T::AccountId, staker: T::AccountId, ict: u128 },
        TakeSet { hotkey: T::AccountId, ppm: u32 },
        NextBonusSet { ppm: u32 },
        RequiredReturnSet { ppm: u32 },
    }

    #[pallet::error]
    pub enum Error<T> {
        NoPool,
        /// The pool is not open for trading (first 10 days, or nothing in it).
        NotTrading,
        NotNeuron,
        TooSmall,
        SlippageExceeded,
        NotEnoughStake,
        NoOwner,
        NothingToClaim,
        BonusOutOfRange,
        SameHotkey,
        TakeTooHigh,
        /// A take can be raised only once per 30 days.
        TakeRaisedTooSoon,
        ReturnOutOfRange,
        PoolExists,
    }

    #[pallet::hooks]
    impl<T: Config> Hooks<BlockNumberFor<T>> for Pallet<T> {
        /// Open pools whose 10 days are up, release stock, sample prices once a day.
        fn on_initialize(_n: BlockNumberFor<T>) -> Weight {
            let now = Self::now();
            let mut n = 0u64;
            for (netuid, mut st) in Pools::<T>::iter() {
                n += 1;
                if st.removed {
                    continue;
                }
                let mut pool = st.pool();
                let mut changed = false;
                if !pool.trading && now >= st.opened + alpha::OPEN_AFTER_BLOCKS {
                    let p0 = alpha::start_price(
                        T::Issuance::subnet_ict_per_day(),
                        T::Issuance::days_to_ict_halving(),
                        RequiredReturnPpm::<T>::get() as u128,
                    );
                    if let Some(o) = pool.open(p0) {
                        if o.to_treasury > 0 {
                            let _ = T::Currency::transfer(
                                &Self::pool_account(netuid),
                                &T::Treasury::get(),
                                o.to_treasury,
                                Preservation::Preserve,
                            );
                        }
                        Self::deposit_event(Event::PoolOpened {
                            netuid,
                            price: pool.price(),
                            ict: o.ict,
                            alpha: o.alpha,
                            to_treasury: o.to_treasury,
                        });
                        PriceDays::<T>::insert(netuid, BoundedVec::truncate_from(alloc::vec![pool.price()]));
                        changed = true;
                    }
                } else if pool.trading {
                    let per_block = mul_div(
                        emission::block_reward(st.halvings),
                        PPM - st.bonus_ppm as u128,
                        PPM,
                    );
                    let days = PriceDays::<T>::get(netuid);
                    let avg = if days.is_empty() {
                        None
                    } else {
                        Some(days.iter().sum::<u128>() / days.len() as u128)
                    };
                    let r = pool.release(per_block, avg);
                    if r > 0 {
                        changed = true;
                        Today::<T>::mutate(netuid, |d| {
                            Self::roll(d, &st);
                            d.released += r;
                        });
                    }
                    if now > st.opened && (now - st.opened) % BLOCKS_PER_DAY == 0 {
                        PriceDays::<T>::mutate(netuid, |v| {
                            if v.len() as u32 >= alpha::AVG_DAYS {
                                v.remove(0);
                            }
                            let _ = v.try_push(pool.price());
                        });
                    }
                }
                if changed {
                    st.set_pool(pool);
                    Pools::<T>::insert(netuid, st);
                }
            }
            T::DbWeight::get().reads_writes(1 + 2 * n, 2 * n)
        }
    }

    #[pallet::call]
    impl<T: Config> Pallet<T> {
        /// Stake ICT on `hotkey` in `netuid`: buy Alpha in the subnet's pool. Fails if
        /// fewer than `min_alpha_out` would come out.
        #[pallet::call_index(0)]
        #[pallet::weight(Weight::from_parts(70_000_000, 7_000))]
        pub fn stake(
            origin: OriginFor<T>,
            netuid: NetUid,
            hotkey: T::AccountId,
            ict_in: u128,
            min_alpha_out: u128,
        ) -> DispatchResult {
            let who = ensure_signed(origin)?;
            ensure!(ict_in >= T::MinTrade::get(), Error::<T>::TooSmall);
            ensure!(T::Subnets::is_neuron(netuid, &hotkey), Error::<T>::NotNeuron);
            let mut st = Pools::<T>::get(netuid).ok_or(Error::<T>::NoPool)?;
            let mut pool = st.pool();
            let t = pool.buy(ict_in, min_alpha_out, Self::fees()).map_err(Self::err)?;
            T::Currency::burn_from(&who, t.burned, Preservation::Preserve, Precision::Exact, Fortitude::Polite)?;
            T::Currency::transfer(&who, &Self::pool_account(netuid), t.amount_in - t.burned, Preservation::Preserve)?;
            st.set_pool(pool);
            Pools::<T>::insert(netuid, st);
            Self::credit(netuid, &hotkey, &who, t.amount_out);
            Self::deposit_event(Event::Staked { netuid, hotkey, staker: who, ict: t.amount_in, alpha: t.amount_out });
            Ok(())
        }

        /// Unstake: sell Alpha staked on `hotkey` back to the pool for ICT. Fails if less
        /// than `min_ict_out` would come out.
        #[pallet::call_index(1)]
        #[pallet::weight(Weight::from_parts(70_000_000, 7_000))]
        pub fn unstake(
            origin: OriginFor<T>,
            netuid: NetUid,
            hotkey: T::AccountId,
            alpha_in: u128,
            min_ict_out: u128,
        ) -> DispatchResult {
            let who = ensure_signed(origin)?;
            ensure!(alpha_in >= T::MinTrade::get(), Error::<T>::TooSmall);
            ensure!(
                Positions::<T>::get((netuid, &hotkey, &who)) >= alpha_in,
                Error::<T>::NotEnoughStake
            );
            let mut st = Pools::<T>::get(netuid).ok_or(Error::<T>::NoPool)?;
            let mut pool = st.pool();
            let t = pool.sell(alpha_in, min_ict_out, Self::fees()).map_err(Self::err)?;
            T::Currency::transfer(&Self::pool_account(netuid), &who, t.amount_out, Preservation::Preserve)?;
            st.set_pool(pool);
            Pools::<T>::insert(netuid, st);
            Self::debit(netuid, &hotkey, &who, t.amount_in);
            Self::deposit_event(Event::Unstaked { netuid, hotkey, staker: who, alpha: t.amount_in, ict: t.amount_out });
            Ok(())
        }

        /// Move staked Alpha from one hotkey of the subnet to another.
        #[pallet::call_index(2)]
        #[pallet::weight(Weight::from_parts(40_000_000, 5_000))]
        pub fn move_stake(
            origin: OriginFor<T>,
            netuid: NetUid,
            from: T::AccountId,
            to: T::AccountId,
            alpha: u128,
        ) -> DispatchResult {
            let who = ensure_signed(origin)?;
            ensure!(from != to, Error::<T>::SameHotkey);
            ensure!(T::Subnets::is_neuron(netuid, &to), Error::<T>::NotNeuron);
            ensure!(Positions::<T>::get((netuid, &from, &who)) >= alpha, Error::<T>::NotEnoughStake);
            Self::debit(netuid, &from, &who, alpha);
            Self::credit(netuid, &to, &who, alpha);
            Self::deposit_event(Event::StakeMoved { netuid, staker: who, from, to, alpha });
            Ok(())
        }

        /// Give staked Alpha to another account; it stays on the same hotkey.
        #[pallet::call_index(3)]
        #[pallet::weight(Weight::from_parts(40_000_000, 5_000))]
        pub fn transfer_stake(
            origin: OriginFor<T>,
            netuid: NetUid,
            hotkey: T::AccountId,
            dest: T::AccountId,
            alpha: u128,
        ) -> DispatchResult {
            let who = ensure_signed(origin)?;
            ensure!(Positions::<T>::get((netuid, &hotkey, &who)) >= alpha, Error::<T>::NotEnoughStake);
            Self::debit(netuid, &hotkey, &who, alpha);
            Self::credit(netuid, &hotkey, &dest, alpha);
            Self::deposit_event(Event::StakeTransferred { netuid, hotkey, from: who, to: dest, alpha });
            Ok(())
        }

        /// Release the owner's vested Alpha to the subnet's current owner, staked on
        /// `hotkey`. Anyone may call it; the Alpha always goes to the owner.
        #[pallet::call_index(4)]
        #[pallet::weight(Weight::from_parts(40_000_000, 6_000))]
        pub fn claim_owner_alpha(origin: OriginFor<T>, netuid: NetUid, hotkey: T::AccountId) -> DispatchResult {
            ensure_signed(origin)?;
            let owner = T::Subnets::owner(netuid).ok_or(Error::<T>::NoOwner)?;
            ensure!(T::Subnets::is_neuron(netuid, &hotkey), Error::<T>::NotNeuron);
            let now = Self::now();
            let mut total = 0u128;
            OwnerVesting::<T>::mutate(netuid, |v| {
                for t in v.iter_mut() {
                    let due = alpha::vested(t.total, t.start, now).saturating_sub(t.claimed);
                    t.claimed += due;
                    total += due;
                }
                v.retain(|t| t.claimed < t.total);
            });
            ensure!(total > 0, Error::<T>::NothingToClaim);
            Self::credit(netuid, &hotkey, &owner, total);
            Self::deposit_event(Event::OwnerAlphaClaimed { netuid, owner, alpha: total });
            Ok(())
        }

        /// Root: bonus share for subnets opened from now on, 25 %–75 %. Existing subnets
        /// keep theirs.
        #[pallet::call_index(5)]
        #[pallet::weight(Weight::from_parts(10_000_000, 1_000))]
        pub fn set_next_bonus_ppm(origin: OriginFor<T>, ppm: u32) -> DispatchResult {
            ensure_root(origin)?;
            ensure!(
                (alpha::MIN_BONUS_PPM..=alpha::MAX_BONUS_PPM).contains(&(ppm as u128)),
                Error::<T>::BonusOutOfRange
            );
            NextBonusPpm::<T>::put(ppm);
            Self::deposit_event(Event::NextBonusSet { ppm });
            Ok(())
        }

        /// Pay out the ICT dividends due on the caller's Alpha staked on `hotkey`.
        #[pallet::call_index(6)]
        #[pallet::weight(Weight::from_parts(30_000_000, 5_000))]
        pub fn claim_dividends(origin: OriginFor<T>, netuid: NetUid, hotkey: T::AccountId) -> DispatchResult {
            let who = ensure_signed(origin)?;
            let paid = Self::settle(netuid, &hotkey, &who);
            ensure!(paid > 0, Error::<T>::NothingToClaim);
            Ok(())
        }

        /// The caller, as a validator hotkey, sets its take on its stakers' dividends:
        /// 0–18 %. Lowering takes effect at once; raising only once per 30 days.
        #[pallet::call_index(7)]
        #[pallet::weight(Weight::from_parts(15_000_000, 2_000))]
        pub fn set_take(origin: OriginFor<T>, ppm: u32) -> DispatchResult {
            let hotkey = ensure_signed(origin)?;
            ensure!(ppm as u128 <= alpha::MAX_TAKE_PPM, Error::<T>::TakeTooHigh);
            let now = Self::now();
            Takes::<T>::try_mutate(&hotkey, |t| -> DispatchResult {
                if ppm > t.ppm {
                    ensure!(
                        t.raised_at == 0 || now >= t.raised_at + TAKE_RAISE_BLOCKS,
                        Error::<T>::TakeRaisedTooSoon
                    );
                    t.raised_at = now.max(1);
                }
                t.ppm = ppm;
                Ok(())
            })?;
            Self::deposit_event(Event::TakeSet { hotkey, ppm });
            Ok(())
        }

        /// Root (the Parameters track): required return in the start-price formula,
        /// 5 %–100 %. Only pools that open afterwards use it.
        #[pallet::call_index(8)]
        #[pallet::weight(Weight::from_parts(10_000_000, 1_000))]
        pub fn set_required_return(origin: OriginFor<T>, ppm: u32) -> DispatchResult {
            ensure_root(origin)?;
            ensure!((50_000..=1_000_000).contains(&ppm), Error::<T>::ReturnOutOfRange);
            RequiredReturnPpm::<T>::put(ppm);
            Self::deposit_event(Event::RequiredReturnSet { ppm });
            Ok(())
        }
    }

    impl<T: Config> Pallet<T> {
        pub fn pool_account(netuid: NetUid) -> T::AccountId {
            T::PalletId::get().into_sub_account_truncating(netuid)
        }

        /// Holds stakers' dividends until they are paid out.
        pub fn dividend_account() -> T::AccountId {
            T::PalletId::get().into_sub_account_truncating(b"dividends")
        }

        fn now() -> u64 {
            polkadot_sdk::frame_system::Pallet::<T>::block_number().saturated_into::<u64>()
        }

        fn fees() -> Fees {
            Fees {
                fee_ppm: T::FeePpm::get() as u128,
                burn_ppm: T::FeeBurnPpm::get() as u128,
            }
        }

        fn err(e: PoolError) -> DispatchError {
            match e {
                PoolError::ZeroInput => Error::<T>::TooSmall.into(),
                PoolError::NotTrading => Error::<T>::NotTrading.into(),
                PoolError::SlippageExceeded => Error::<T>::SlippageExceeded.into(),
            }
        }

        fn roll(d: &mut DayStats, st: &PoolState) {
            let day = Self::now().saturating_sub(st.opened) / BLOCKS_PER_DAY;
            if d.day != day {
                *d = DayStats { day, ..Default::default() };
            }
        }

        /// Dividends due on a position now.
        pub fn pending_dividends(netuid: NetUid, hotkey: &T::AccountId, staker: &T::AccountId) -> u128 {
            let pos = Positions::<T>::get((netuid, hotkey, staker));
            let acc = AccPerAlpha::<T>::get(netuid, hotkey);
            mul_div(pos, acc, ACC_SCALE).saturating_sub(Debt::<T>::get((netuid, hotkey, staker)))
        }

        /// Pay a position's due dividends and reset its debt. Returns what was paid.
        fn settle(netuid: NetUid, hotkey: &T::AccountId, staker: &T::AccountId) -> u128 {
            let due = Self::pending_dividends(netuid, hotkey, staker);
            let mut paid = 0;
            if due > 0
                && T::Currency::transfer(&Self::dividend_account(), staker, due, Preservation::Expendable).is_ok()
            {
                paid = due;
                Self::deposit_event(Event::DividendsPaid {
                    netuid,
                    hotkey: hotkey.clone(),
                    staker: staker.clone(),
                    ict: due,
                });
            }
            if paid == due {
                let pos = Positions::<T>::get((netuid, hotkey, staker));
                Self::set_debt(netuid, hotkey, staker, pos);
            }
            paid
        }

        fn set_debt(netuid: NetUid, hotkey: &T::AccountId, staker: &T::AccountId, pos: u128) {
            let d = mul_div(pos, AccPerAlpha::<T>::get(netuid, hotkey), ACC_SCALE);
            if d == 0 {
                Debt::<T>::remove((netuid, hotkey, staker));
            } else {
                Debt::<T>::insert((netuid, hotkey, staker), d);
            }
        }

        fn credit(netuid: NetUid, hotkey: &T::AccountId, staker: &T::AccountId, a: u128) {
            if a == 0 {
                return;
            }
            Self::settle(netuid, hotkey, staker);
            Positions::<T>::mutate((netuid, hotkey, staker), |p| *p += a);
            HotkeyStake::<T>::mutate(netuid, hotkey, |p| *p += a);
            TotalStake::<T>::mutate(netuid, |p| *p += a);
            Self::set_debt(netuid, hotkey, staker, Positions::<T>::get((netuid, hotkey, staker)));
        }

        fn debit(netuid: NetUid, hotkey: &T::AccountId, staker: &T::AccountId, a: u128) {
            Self::settle(netuid, hotkey, staker);
            Positions::<T>::mutate_exists((netuid, hotkey, staker), |p| {
                let v = p.unwrap_or(0).saturating_sub(a);
                *p = if v == 0 { None } else { Some(v) };
            });
            HotkeyStake::<T>::mutate_exists(netuid, hotkey, |p| {
                let v = p.unwrap_or(0).saturating_sub(a);
                *p = if v == 0 { None } else { Some(v) };
            });
            TotalStake::<T>::mutate(netuid, |p| *p = p.saturating_sub(a));
            Self::set_debt(netuid, hotkey, staker, Positions::<T>::get((netuid, hotkey, staker)));
        }

        /// Pay `amount` Alpha to `recipients` in proportion to their ICT. Returns what was paid.
        fn pay_pro_rata(netuid: NetUid, amount: u128, recipients: &[(T::AccountId, u128)]) -> u128 {
            let total: u128 = recipients.iter().map(|r| r.1).sum();
            if total == 0 || amount == 0 {
                return 0;
            }
            let mut paid = 0u128;
            for (who, ict) in recipients {
                let a = mul_div(amount, *ict, total);
                Self::credit(netuid, who, who, a);
                paid += a;
            }
            paid
        }

        pub fn summaries() -> Vec<runtime_api::PoolSummary> {
            let mut all: Vec<(NetUid, PoolState)> = Pools::<T>::iter().collect();
            all.sort_by_key(|(n, _)| *n);
            all.into_iter()
                .map(|(netuid, st)| {
                    let mut d = Today::<T>::get(netuid);
                    Self::roll(&mut d, &st);
                    runtime_api::PoolSummary {
                        netuid,
                        name: T::Subnets::name(netuid),
                        open: st.trading && !st.removed,
                        price: st.pool().price(),
                        ict_reserve: st.x,
                        alpha_reserve: st.y,
                        alpha_issued: st.issued,
                        alpha_staked: TotalStake::<T>::get(netuid),
                        bonus_today: d.bonus,
                        bonus_minted: st.bonus_minted,
                        stock: st.waiting,
                        released_today: d.released,
                        fee_ppm: T::FeePpm::get(),
                    }
                })
                .collect()
        }

        pub fn quote_stake(netuid: NetUid, ict_in: u128) -> Option<u128> {
            Pools::<T>::get(netuid)?.pool().quote_buy(ict_in, Self::fees()).map(|t| t.amount_out)
        }

        pub fn quote_unstake(netuid: NetUid, alpha_in: u128) -> Option<u128> {
            Pools::<T>::get(netuid)?.pool().quote_sell(alpha_in, Self::fees()).map(|t| t.amount_out)
        }
    }

    impl<T: Config> PoolsInterface<T::AccountId> for Pallet<T> {
        fn register_pool(netuid: NetUid, from: &T::AccountId, reserved_ict: u128) -> DispatchResult {
            ensure!(!Pools::<T>::contains_key(netuid), Error::<T>::PoolExists);
            T::Currency::transfer(from, &Self::pool_account(netuid), reserved_ict, Preservation::Expendable)?;
            let bonus_ppm = NextBonusPpm::<T>::get();
            let mut st = PoolState { bonus_ppm, opened: Self::now(), ..Default::default() };
            st.set_pool(AlphaPool::new(reserved_ict));
            Pools::<T>::insert(netuid, st);
            Self::deposit_event(Event::PoolRegistered { netuid, reserved_ict, bonus_ppm });
            Ok(())
        }

        fn close_pool(netuid: NetUid) {
            let mut burned_stock = 0;
            Pools::<T>::mutate(netuid, |s| {
                if let Some(st) = s {
                    let mut p = st.pool();
                    burned_stock = p.remove();
                    // a pool removed before it opened returns its held ICT to the treasury
                    if !p.trading && p.reserved_ict > 0 {
                        let _ = T::Currency::transfer(
                            &Self::pool_account(netuid),
                            &T::Treasury::get(),
                            p.reserved_ict,
                            Preservation::Expendable,
                        );
                        p.reserved_ict = 0;
                    }
                    st.set_pool(p);
                }
            });
            // the subnet has no owner any more: unvested owner Alpha is burned
            OwnerVesting::<T>::remove(netuid);
            Self::deposit_event(Event::PoolClosed { netuid, burned_stock });
        }

        fn on_epoch(
            netuid: NetUid,
            blocks: u32,
            miners: &[(T::AccountId, u128)],
            validators: &[(T::AccountId, u128)],
        ) {
            let Some(mut st) = Pools::<T>::get(netuid) else { return };
            if st.removed {
                return;
            }
            let mut iss = st.issuance();
            let e = iss.issue(blocks as u64, st.bonus_ppm as u128);
            st.issued = iss.state.total_issued;
            st.halvings = iss.state.halvings;
            let split = alpha::split_bonus(e.bonus);
            let paid_m = Self::pay_pro_rata(netuid, split.miners, miners);
            let paid_v = Self::pay_pro_rata(netuid, split.validators, validators);
            // the owner's part vests; with no owner, or nobody paid, it goes to the stock
            let anyone_paid = paid_m + paid_v > 0;
            let owner_part = if anyone_paid && T::Subnets::owner(netuid).is_some() {
                let now = Self::now();
                let day_start = st.opened + (now.saturating_sub(st.opened) / BLOCKS_PER_DAY) * BLOCKS_PER_DAY;
                let added = OwnerVesting::<T>::mutate(netuid, |v| {
                    if let Some(last) = v.last_mut() {
                        if last.start == day_start {
                            last.total += split.owner;
                            return true;
                        }
                    }
                    v.try_push(Tranche { start: day_start, total: split.owner, claimed: 0 }).is_ok()
                });
                if added { split.owner } else { 0 }
            } else {
                0
            };
            let bonus = paid_m + paid_v + owner_part;
            let to_stock = e.issued - bonus;
            let mut pool = st.pool();
            pool.add_stock(to_stock);
            st.set_pool(pool);
            st.bonus_minted += bonus;
            Pools::<T>::insert(netuid, st);
            Today::<T>::mutate(netuid, |d| {
                Self::roll(d, &st);
                d.bonus += bonus;
                d.stock += to_stock;
            });
            Self::deposit_event(Event::AlphaIssued { netuid, bonus, stock: to_stock });
        }

        fn pay_validator_ict(
            netuid: NetUid,
            source: &T::AccountId,
            hotkey: &T::AccountId,
            amount: u128,
        ) -> DispatchResult {
            if amount == 0 {
                return Ok(());
            }
            let staked = HotkeyStake::<T>::get(netuid, hotkey);
            let take = if staked == 0 {
                amount
            } else {
                mul_div(amount, Takes::<T>::get(hotkey).ppm as u128, PPM)
            };
            let rest = amount - take;
            if take > 0 {
                T::Currency::transfer(source, hotkey, take, Preservation::Expendable)?;
            }
            if rest > 0 {
                T::Currency::transfer(source, &Self::dividend_account(), rest, Preservation::Expendable)?;
                AccPerAlpha::<T>::mutate(netuid, hotkey, |a| *a += mul_div(rest, ACC_SCALE, staked));
            }
            Self::deposit_event(Event::ValidatorPaid { netuid, hotkey: hotkey.clone(), take, to_stakers: rest });
            Ok(())
        }

        fn stake_ict(netuid: NetUid) -> u128 {
            Pools::<T>::get(netuid).map(|s| s.pool().stake_ict()).unwrap_or(0)
        }

        fn alpha_stake(netuid: NetUid, hotkey: &T::AccountId) -> u128 {
            HotkeyStake::<T>::get(netuid, hotkey)
        }

        fn total_alpha_stake(netuid: NetUid) -> u128 {
            TotalStake::<T>::get(netuid)
        }
    }
}

pub mod runtime_api {
    extern crate alloc;
    use super::NetUid;
    use alloc::vec::Vec;
    use codec::{Decode, Encode};
    use scale_info::TypeInfo;

    /// One pool, as incentium.org/pools reads it (spec Section 7).
    #[derive(Clone, Encode, Decode, TypeInfo, Debug, PartialEq, Eq)]
    pub struct PoolSummary {
        pub netuid: NetUid,
        pub name: Vec<u8>,
        /// Open for trading (false in the first 10 days and once removed).
        pub open: bool,
        /// ICT base units per whole Alpha (0 while closed).
        pub price: u128,
        /// ICT in the pool.
        pub ict_reserve: u128,
        /// Alpha in the pool.
        pub alpha_reserve: u128,
        pub alpha_issued: u128,
        pub alpha_staked: u128,
        pub bonus_today: u128,
        pub bonus_minted: u128,
        /// Issued stock waiting to be released.
        pub stock: u128,
        /// Stock released into the pool today.
        pub released_today: u128,
        pub fee_ppm: u32,
    }

    polkadot_sdk::sp_api::decl_runtime_apis! {
        pub trait SubnetPoolsApi {
            /// Every pool in netuid order.
            fn pools() -> Vec<PoolSummary>;
            /// Alpha received for staking `ict_in` now, or none.
            fn quote_stake(netuid: NetUid, ict_in: u128) -> Option<u128>;
            /// ICT received for unstaking `alpha_in` now, or none.
            fn quote_unstake(netuid: NetUid, alpha_in: u128) -> Option<u128>;
        }
    }
}
