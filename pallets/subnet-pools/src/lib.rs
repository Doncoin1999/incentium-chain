//! # Subnet pools (spec Section 7, decided 25 Nov 2026, pool rules revised after simulation)
//!
//! Every subnet other than SN0 has an Alpha token with ICT's tokenomics and one pool in
//! which staking and trading are the same thing:
//!
//! - **Issuance.** No Alpha exists when a subnet opens; then exactly ICT's schedule
//!   (7,200 a day, halving, 21,000,000) counted from the subnet's own first block.
//! - **Bonus and stock.** Each epoch's Alpha is split by the subnet's `bonus_ppm`, fixed
//!   when it opens: the bonus goes to its miners, validators and owner 50/38/12 in
//!   proportion to the ICT they were paid that epoch (the owner's part vests over 180
//!   days); the rest goes straight into the pool. An epoch that pays nobody sends its
//!   bonus to the pool too.
//! - **Staking is buying.** `stake` swaps ICT for Alpha in the pool and credits the Alpha
//!   as stake on a hotkey of the subnet; `unstake` sells it back. A trade larger than the
//!   real reserve fills partially.
//! - **Nobody owns the pool.** Its ICT sits on an account derived from the netuid with no
//!   key. Only trades move it, and only from real reserves; the start curve is virtual.
//! - **Removal.** A removed subnet's pool gets no more Alpha; trading goes on.
//!
//! Alpha exists only as stake positions `(netuid, hotkey, staker)`, kept in this pallet.
//! The registry opens and closes pools; consensus reports each epoch's ICT payouts.
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

/// What the registry and consensus call on this pallet.
pub trait PoolsInterface<AccountId> {
    /// A slot sold for `sale_price` ICT opens its pool.
    fn open_pool(netuid: NetUid, sale_price: u128);
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
    /// ICT that counts as the subnet's stake: the pool's real ICT.
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
        emission::EmissionState,
        mul_div, BLOCKS_PER_DAY,
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
        /// Trading fee, parts per million of the input (proposed 5,000 = 0.5 %).
        type FeePpm: Get<u32>;
        /// Share of the fee that is burned (proposed 500,000 = half).
        type FeeBurnPpm: Get<u32>;
        /// Smallest stake or unstake, so dust cannot clog the pool.
        type MinTrade: Get<u128>;
    }

    /// Pool state, kept in SCALE-friendly form.
    #[derive(Clone, Copy, Encode, Decode, DecodeWithMemTracking, MaxEncodedLen, TypeInfo, Debug, PartialEq, Eq, Default)]
    pub struct PoolState {
        pub vx: u128,
        pub vy: u128,
        pub rx: u128,
        pub ry: u128,
        pub open: bool,
        /// Alpha issued so far and the halvings it has passed.
        pub issued: u128,
        pub halvings: u8,
        /// Share of each epoch's Alpha paid as a bonus, fixed when the pool opened.
        pub bonus_ppm: u32,
        /// Block the pool opened; owner vesting and day windows count from here.
        pub opened: u64,
        /// Bonus paid in total.
        pub bonus_minted: u128,
    }

    impl PoolState {
        pub fn pool(&self) -> AlphaPool {
            AlphaPool {
                vx: self.vx,
                vy: self.vy,
                rx: self.rx,
                ry: self.ry,
                open: self.open,
            }
        }
        pub fn set_pool(&mut self, p: AlphaPool) {
            self.vx = p.vx;
            self.vy = p.vy;
            self.rx = p.rx;
            self.ry = p.ry;
            self.open = p.open;
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
        pub stock: u128,
        pub bought: u128,
    }

    /// One day's owner Alpha, released linearly over 180 days.
    #[derive(Clone, Copy, Encode, Decode, DecodeWithMemTracking, MaxEncodedLen, TypeInfo, Debug, PartialEq, Eq, Default)]
    pub struct Tranche {
        pub start: u64,
        pub total: u128,
        pub claimed: u128,
    }

    /// 180 days of tranches plus a margin.
    pub type MaxTranches = ConstU32<200>;

    #[pallet::storage]
    pub type Pools<T> = StorageMap<_, Twox64Concat, NetUid, PoolState, OptionQuery>;

    #[pallet::storage]
    pub type Today<T> = StorageMap<_, Twox64Concat, NetUid, DayStats, ValueQuery>;

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

    #[pallet::event]
    #[pallet::generate_deposit(pub(super) fn deposit_event)]
    pub enum Event<T: Config> {
        PoolOpened { netuid: NetUid, start_price: u128, bonus_ppm: u32 },
        PoolClosed { netuid: NetUid },
        Staked { netuid: NetUid, hotkey: T::AccountId, staker: T::AccountId, ict: u128, alpha: u128 },
        Unstaked { netuid: NetUid, hotkey: T::AccountId, staker: T::AccountId, alpha: u128, ict: u128 },
        StakeMoved { netuid: NetUid, staker: T::AccountId, from: T::AccountId, to: T::AccountId, alpha: u128 },
        StakeTransferred { netuid: NetUid, hotkey: T::AccountId, from: T::AccountId, to: T::AccountId, alpha: u128 },
        AlphaIssued { netuid: NetUid, bonus: u128, stock: u128 },
        OwnerAlphaClaimed { netuid: NetUid, owner: T::AccountId, alpha: u128 },
        NextBonusSet { ppm: u32 },
    }

    #[pallet::error]
    pub enum Error<T> {
        NoPool,
        PoolClosed,
        NotNeuron,
        TooSmall,
        /// Not enough real reserve on the other side.
        Insufficient,
        SlippageExceeded,
        NotEnoughStake,
        NoOwner,
        NothingToClaim,
        BonusOutOfRange,
        SameHotkey,
    }

    #[pallet::call]
    impl<T: Config> Pallet<T> {
        /// Stake ICT on `hotkey` in `netuid`: buy Alpha in the subnet's pool. A purchase
        /// larger than the pool can fill is filled partly and charged only for that part.
        #[pallet::call_index(0)]
        #[pallet::weight(Weight::from_parts(60_000_000, 6_000))]
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
            // the buyer pays the ICT actually used: the burned fee is burned, the rest goes to the pool
            T::Currency::burn_from(&who, t.burned, Preservation::Preserve, Precision::Exact, Fortitude::Polite)?;
            T::Currency::transfer(&who, &Self::pool_account(netuid), t.amount_in - t.burned, Preservation::Preserve)?;
            st.set_pool(pool);
            Pools::<T>::insert(netuid, st);
            Self::credit(netuid, &hotkey, &who, t.amount_out);
            Today::<T>::mutate(netuid, |d| {
                Self::roll(d, &st);
                d.bought += t.amount_out;
            });
            Self::deposit_event(Event::Staked { netuid, hotkey, staker: who, ict: t.amount_in, alpha: t.amount_out });
            Ok(())
        }

        /// Unstake: sell Alpha staked on `hotkey` back to the pool for ICT. A sale larger
        /// than the pool's real ICT is filled partly; the rest stays staked.
        #[pallet::call_index(1)]
        #[pallet::weight(Weight::from_parts(60_000_000, 6_000))]
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
            // the pool account always keeps the existential deposit, so it is never reaped
            // and its balance always covers the real reserve
            T::Currency::transfer(&Self::pool_account(netuid), &who, t.amount_out, Preservation::Preserve)
                .map_err(|_| Error::<T>::Insufficient)?;
            st.set_pool(pool);
            Pools::<T>::insert(netuid, st);
            Self::debit(netuid, &hotkey, &who, t.amount_in);
            Self::deposit_event(Event::Unstaked { netuid, hotkey, staker: who, alpha: t.amount_in, ict: t.amount_out });
            Ok(())
        }

        /// Move staked Alpha from one hotkey of the subnet to another.
        #[pallet::call_index(2)]
        #[pallet::weight(Weight::from_parts(30_000_000, 4_000))]
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
        #[pallet::weight(Weight::from_parts(30_000_000, 4_000))]
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
    }

    impl<T: Config> Pallet<T> {
        pub fn pool_account(netuid: NetUid) -> T::AccountId {
            T::PalletId::get().into_sub_account_truncating(netuid)
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
                PoolError::Insufficient => Error::<T>::Insufficient.into(),
                PoolError::SlippageExceeded => Error::<T>::SlippageExceeded.into(),
            }
        }

        fn roll(d: &mut DayStats, st: &PoolState) {
            let day = Self::now().saturating_sub(st.opened) / BLOCKS_PER_DAY;
            if d.day != day {
                *d = DayStats { day, ..Default::default() };
            }
        }

        fn credit(netuid: NetUid, hotkey: &T::AccountId, staker: &T::AccountId, a: u128) {
            if a == 0 {
                return;
            }
            Positions::<T>::mutate((netuid, hotkey, staker), |p| *p += a);
            HotkeyStake::<T>::mutate(netuid, hotkey, |p| *p += a);
            TotalStake::<T>::mutate(netuid, |p| *p += a);
        }

        fn debit(netuid: NetUid, hotkey: &T::AccountId, staker: &T::AccountId, a: u128) {
            Positions::<T>::mutate_exists((netuid, hotkey, staker), |p| {
                let v = p.unwrap_or(0).saturating_sub(a);
                *p = if v == 0 { None } else { Some(v) };
            });
            HotkeyStake::<T>::mutate_exists(netuid, hotkey, |p| {
                let v = p.unwrap_or(0).saturating_sub(a);
                *p = if v == 0 { None } else { Some(v) };
            });
            TotalStake::<T>::mutate(netuid, |p| *p = p.saturating_sub(a));
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
                        open: st.open,
                        price: st.pool().price(),
                        ict_reserve: st.rx,
                        alpha_reserve: st.ry,
                        alpha_issued: st.issued,
                        alpha_staked: TotalStake::<T>::get(netuid),
                        bonus_today: d.bonus,
                        bonus_minted: st.bonus_minted,
                        stock_today: d.stock,
                        bought_today: d.bought,
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
        fn open_pool(netuid: NetUid, sale_price: u128) {
            if Pools::<T>::contains_key(netuid) {
                return;
            }
            let p = AlphaPool::open(sale_price);
            let bonus_ppm = NextBonusPpm::<T>::get();
            let mut st = PoolState {
                bonus_ppm,
                opened: Self::now(),
                ..Default::default()
            };
            st.set_pool(p);
            Pools::<T>::insert(netuid, st);
            Self::deposit_event(Event::PoolOpened { netuid, start_price: p.price(), bonus_ppm });
        }

        fn close_pool(netuid: NetUid) {
            Pools::<T>::mutate(netuid, |s| {
                if let Some(st) = s {
                    st.open = false;
                }
            });
            // Owner Alpha not yet vested is dropped: the subnet has no owner any more, so it is
            // in effect burned. (Open point for the owner of Incentium to confirm.)
            OwnerVesting::<T>::remove(netuid);
            Self::deposit_event(Event::PoolClosed { netuid });
        }

        fn on_epoch(
            netuid: NetUid,
            blocks: u32,
            miners: &[(T::AccountId, u128)],
            validators: &[(T::AccountId, u128)],
        ) {
            let Some(mut st) = Pools::<T>::get(netuid) else { return };
            if !st.open {
                return;
            }
            let mut iss = st.issuance();
            let e = iss.issue(blocks as u64, st.bonus_ppm as u128);
            st.issued = iss.state.total_issued;
            st.halvings = iss.state.halvings;
            let split = alpha::split_bonus(e.bonus);
            let paid_m = Self::pay_pro_rata(netuid, split.miners, miners);
            let paid_v = Self::pay_pro_rata(netuid, split.validators, validators);
            // the owner's part vests; with no owner, or nobody paid, it goes to the pool
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
            let to_pool = e.issued - bonus;
            let mut pool = st.pool();
            pool.add_stock(to_pool);
            st.set_pool(pool);
            st.bonus_minted += bonus;
            Pools::<T>::insert(netuid, st);
            Today::<T>::mutate(netuid, |d| {
                Self::roll(d, &st);
                d.bonus += bonus;
                d.stock += to_pool;
            });
            Self::deposit_event(Event::AlphaIssued { netuid, bonus, stock: to_pool });
        }

        fn stake_ict(netuid: NetUid) -> u128 {
            Pools::<T>::get(netuid).map(|s| s.rx).unwrap_or(0)
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
        pub open: bool,
        /// ICT base units per whole Alpha.
        pub price: u128,
        /// Real ICT in the pool.
        pub ict_reserve: u128,
        /// Real Alpha in the pool.
        pub alpha_reserve: u128,
        pub alpha_issued: u128,
        pub alpha_staked: u128,
        pub bonus_today: u128,
        pub bonus_minted: u128,
        /// Alpha added to the pool today.
        pub stock_today: u128,
        /// Alpha bought from the pool today.
        pub bought_today: u128,
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
