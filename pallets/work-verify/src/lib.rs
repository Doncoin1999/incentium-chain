//! # Work-verify pallet (spec Section 5, SN0)
//!
//! - Miners and pools post a bond.
//! - Validators report failed spot checks. When reporters hold at least `QuorumPpm` (50 %)
//!   of the active validator stake for the same (epoch, uid), the miner gets no incentive
//!   for that epoch; three penalties inside the window ban the hotkey. When they hold at
//!   least `SlashQuorumPpm` (2/3), 10 % of its bond is also slashed (half burned, half to
//!   the insurance fund). Decided 25 Nov 2026.
//! - Pools receive their miners' rewards in this pallet and pay sub-workers pro rata to
//!   verified work units, minus a public fee (max 5 %), via `distribute`. The receipt
//!   root is published so anyone can recompute the payout.
#![cfg_attr(not(feature = "std"), no_std)]

pub use pallet::*;
pub mod weights;
pub use weights::WeightInfo;
#[cfg(feature = "runtime-benchmarks")]
mod benchmarking;

#[cfg(test)]
mod mock;
#[cfg(test)]
mod tests;

/// Validator stake as seen by work-verify (implemented by the consensus pallet).
pub trait StakeOracle<AccountId> {
    fn stake_of(who: &AccountId) -> u128;
    fn total_stake() -> u128;
    /// Stake of the validators whose weights counted in the last settled SN0 epoch (the base
    /// of the report quorums; falls back to the total before any epoch is settled).
    fn active_stake() -> u128;
    /// Give `who` validator stake (benchmark setup only).
    #[cfg(feature = "runtime-benchmarks")]
    fn set_stake(who: &AccountId, amount: u128);
}

#[polkadot_sdk::frame_support::pallet]
pub mod pallet {
    use super::StakeOracle;
    use crate::WeightInfo;
    use ict_economics::{mul_div, PPM};
    use pallet_subnet_registry::{Neurons, Uid, SN0};
    use pallet_weights::Epoch;
    use polkadot_sdk::frame_support::{
        pallet_prelude::*,
        traits::{
            fungible::Mutate,
            tokens::{Fortitude, Precision, Preservation},
        },
        PalletId,
    };
    use polkadot_sdk::frame_system::pallet_prelude::*;
    use polkadot_sdk::sp_core::H256;
    use polkadot_sdk::sp_runtime::traits::AccountIdConversion;
    extern crate alloc;
    use alloc::vec::Vec;

    #[derive(
        Clone,
        Encode,
        Decode,
        DecodeWithMemTracking,
        PartialEq,
        Eq,
        Debug,
        TypeInfo,
        MaxEncodedLen,
        Default,
    )]
    pub struct PoolInfo {
        pub fee_ppm: u32,
        /// Scheduled fee change and the epoch it takes effect.
        pub next_fee: Option<(u32, Epoch)>,
        /// ICT held for sub-workers, not yet distributed.
        pub balance: u128,
    }

    #[pallet::pallet]
    pub struct Pallet<T>(_);

    #[pallet::config]
    pub trait Config: polkadot_sdk::frame_system::Config + pallet_weights::Config {
        #[allow(deprecated)]
        type RuntimeEvent: From<Event<Self>>
            + IsType<<Self as polkadot_sdk::frame_system::Config>::RuntimeEvent>;
        /// Weights of this pallet's calls (benchmarked in the runtime).
        type WeightInfo: WeightInfo;
        type WorkCurrency: Mutate<Self::AccountId, Balance = u128>;
        type Stake: StakeOracle<Self::AccountId>;
        #[pallet::constant]
        type WorkPalletId: Get<PalletId>;
        #[pallet::constant]
        type InsuranceFund: Get<Self::AccountId>;
        #[pallet::constant]
        type PoolBond: Get<u128>;
        #[pallet::constant]
        type MaxPoolFeePpm: Get<u32>;
        #[pallet::constant]
        type FeeChangeDelay: Get<Epoch>;
        #[pallet::constant]
        type SlashPpm: Get<u32>;
        /// Reported share of active stake that takes the miner's incentive (50 %).
        #[pallet::constant]
        type QuorumPpm: Get<u32>;
        /// Reported share of active stake that also slashes the bond (2/3).
        #[pallet::constant]
        type SlashQuorumPpm: Get<u32>;
        #[pallet::constant]
        type MaxFailures: Get<u32>;
        #[pallet::constant]
        type FailureWindow: Get<Epoch>;
        #[pallet::constant]
        type UnbondingEpochs: Get<Epoch>;
        #[pallet::constant]
        type MaxReporters: Get<u32>;
        #[pallet::constant]
        type MaxPayees: Get<u32>;
    }

    #[pallet::storage]
    pub type Bonds<T: Config> = StorageMap<_, Blake2_128Concat, T::AccountId, u128, ValueQuery>;

    #[pallet::storage]
    pub type Unbonding<T: Config> =
        StorageMap<_, Blake2_128Concat, T::AccountId, (u128, Epoch), OptionQuery>;

    #[pallet::storage]
    pub type Pools<T: Config> =
        StorageMap<_, Blake2_128Concat, T::AccountId, PoolInfo, OptionQuery>;

    #[pallet::storage]
    pub type Reporters<T: Config> = StorageDoubleMap<
        _,
        Twox64Concat,
        Epoch,
        Twox64Concat,
        Uid,
        BoundedVec<T::AccountId, T::MaxReporters>,
        ValueQuery,
    >;

    #[pallet::storage]
    pub type ReportedStake<T> =
        StorageDoubleMap<_, Twox64Concat, Epoch, Twox64Concat, Uid, u128, ValueQuery>;

    #[pallet::storage]
    pub type Penalized<T> =
        StorageDoubleMap<_, Twox64Concat, Epoch, Twox64Concat, Uid, bool, ValueQuery>;

    /// Bond already slashed for (epoch, uid).
    #[pallet::storage]
    pub type Slashed<T> =
        StorageDoubleMap<_, Twox64Concat, Epoch, Twox64Concat, Uid, bool, ValueQuery>;

    #[pallet::storage]
    pub type Failures<T: Config> =
        StorageMap<_, Blake2_128Concat, T::AccountId, BoundedVec<Epoch, ConstU32<8>>, ValueQuery>;

    #[pallet::storage]
    pub type Banned<T: Config> = StorageMap<_, Blake2_128Concat, T::AccountId, (), OptionQuery>;

    #[pallet::event]
    #[pallet::generate_deposit(pub(super) fn deposit_event)]
    pub enum Event<T: Config> {
        Bonded {
            who: T::AccountId,
            amount: u128,
        },
        UnbondRequested {
            who: T::AccountId,
            amount: u128,
            available_at: Epoch,
        },
        Withdrawn {
            who: T::AccountId,
            amount: u128,
        },
        PoolRegistered {
            pool: T::AccountId,
            fee_ppm: u32,
        },
        PoolFeeScheduled {
            pool: T::AccountId,
            fee_ppm: u32,
            from_epoch: Epoch,
        },
        FailedCheckReported {
            epoch: Epoch,
            uid: Uid,
            reporter: T::AccountId,
            evidence: H256,
        },
        /// The miner gets no incentive for `epoch` (`slashed` is always 0; see `BondSlashed`).
        MinerPenalized {
            epoch: Epoch,
            uid: Uid,
            hotkey: T::AccountId,
            slashed: u128,
        },
        /// Reporters with 2/3 of active stake agreed: part of the bond was slashed.
        BondSlashed {
            epoch: Epoch,
            uid: Uid,
            hotkey: T::AccountId,
            slashed: u128,
        },
        HotkeyBanned {
            hotkey: T::AccountId,
        },
        PoolCredited {
            pool: T::AccountId,
            amount: u128,
        },
        PoolDistributed {
            pool: T::AccountId,
            epoch: Epoch,
            receipts_root: H256,
            paid: u128,
            fee: u128,
        },
    }

    #[pallet::error]
    pub enum Error<T> {
        NotSn0Neuron,
        BondTooLow,
        PoolExists,
        NotAPool,
        FeeTooHigh,
        NotAValidator,
        UnknownUid,
        AlreadyReported,
        TooManyReporters,
        NothingToWithdraw,
        NotYetUnbonded,
        InsufficientBond,
        EmptyDistribution,
        TooManyPayees,
    }

    #[pallet::call]
    impl<T: Config> Pallet<T> {
        #[pallet::call_index(0)]
        #[pallet::weight(<T as Config>::WeightInfo::bond())]
        pub fn bond(origin: OriginFor<T>, amount: u128) -> DispatchResult {
            let who = ensure_signed(origin)?;
            T::WorkCurrency::transfer(&who, &Self::account_id(), amount, Preservation::Preserve)?;
            Bonds::<T>::mutate(&who, |b| *b += amount);
            Self::deposit_event(Event::Bonded { who, amount });
            Ok(())
        }

        #[pallet::call_index(1)]
        #[pallet::weight(<T as Config>::WeightInfo::unbond())]
        pub fn unbond(origin: OriginFor<T>, amount: u128) -> DispatchResult {
            let who = ensure_signed(origin)?;
            let bond = Bonds::<T>::get(&who);
            ensure!(bond >= amount, Error::<T>::InsufficientBond);
            if Pools::<T>::contains_key(&who) {
                ensure!(bond - amount >= T::PoolBond::get(), Error::<T>::BondTooLow);
            }
            Bonds::<T>::insert(&who, bond - amount);
            let at = pallet_weights::Pallet::<T>::current_epoch() + T::UnbondingEpochs::get();
            Unbonding::<T>::mutate(&who, |u| {
                let prev = u.map(|x| x.0).unwrap_or(0);
                *u = Some((prev + amount, at));
            });
            Self::deposit_event(Event::UnbondRequested {
                who,
                amount,
                available_at: at,
            });
            Ok(())
        }

        #[pallet::call_index(2)]
        #[pallet::weight(<T as Config>::WeightInfo::withdraw_unbonded())]
        pub fn withdraw_unbonded(origin: OriginFor<T>) -> DispatchResult {
            let who = ensure_signed(origin)?;
            let (amount, at) = Unbonding::<T>::get(&who).ok_or(Error::<T>::NothingToWithdraw)?;
            ensure!(
                pallet_weights::Pallet::<T>::current_epoch() >= at,
                Error::<T>::NotYetUnbonded
            );
            Unbonding::<T>::remove(&who);
            T::WorkCurrency::transfer(&Self::account_id(), &who, amount, Preservation::Expendable)?;
            Self::deposit_event(Event::Withdrawn { who, amount });
            Ok(())
        }

        /// Register the caller's SN0 hotkey as a pool. Requires a bond of at least `PoolBond`.
        #[pallet::call_index(3)]
        #[pallet::weight(<T as Config>::WeightInfo::register_pool())]
        pub fn register_pool(origin: OriginFor<T>, fee_ppm: u32) -> DispatchResult {
            let who = ensure_signed(origin)?;
            ensure!(
                pallet_subnet_registry::NeuronUid::<T>::contains_key(SN0, &who),
                Error::<T>::NotSn0Neuron
            );
            ensure!(!Pools::<T>::contains_key(&who), Error::<T>::PoolExists);
            ensure!(fee_ppm <= T::MaxPoolFeePpm::get(), Error::<T>::FeeTooHigh);
            ensure!(
                Bonds::<T>::get(&who) >= T::PoolBond::get(),
                Error::<T>::BondTooLow
            );
            Pools::<T>::insert(
                &who,
                PoolInfo {
                    fee_ppm,
                    next_fee: None,
                    balance: 0,
                },
            );
            Self::deposit_event(Event::PoolRegistered { pool: who, fee_ppm });
            Ok(())
        }

        /// Schedule a fee change; it applies `FeeChangeDelay` epochs later.
        #[pallet::call_index(4)]
        #[pallet::weight(<T as Config>::WeightInfo::set_pool_fee())]
        pub fn set_pool_fee(origin: OriginFor<T>, fee_ppm: u32) -> DispatchResult {
            let who = ensure_signed(origin)?;
            ensure!(fee_ppm <= T::MaxPoolFeePpm::get(), Error::<T>::FeeTooHigh);
            let from = pallet_weights::Pallet::<T>::current_epoch() + T::FeeChangeDelay::get();
            Pools::<T>::try_mutate(&who, |p| -> DispatchResult {
                let p = p.as_mut().ok_or(Error::<T>::NotAPool)?;
                p.next_fee = Some((fee_ppm, from));
                Ok(())
            })?;
            Self::deposit_event(Event::PoolFeeScheduled {
                pool: who,
                fee_ppm,
                from_epoch: from,
            });
            Ok(())
        }

        /// Pay the pool's balance to sub-workers pro rata to verified work units, minus the fee.
        #[pallet::call_index(5)]
        #[pallet::weight(<T as Config>::WeightInfo::distribute(shares.len() as u32))]
        pub fn distribute(
            origin: OriginFor<T>,
            epoch: Epoch,
            receipts_root: H256,
            shares: Vec<(T::AccountId, u64)>,
        ) -> DispatchResult {
            let pool = ensure_signed(origin)?;
            ensure!(!shares.is_empty(), Error::<T>::EmptyDistribution);
            ensure!(
                shares.len() as u32 <= T::MaxPayees::get(),
                Error::<T>::TooManyPayees
            );
            let mut info = Pools::<T>::get(&pool).ok_or(Error::<T>::NotAPool)?;
            let now = pallet_weights::Pallet::<T>::current_epoch();
            if let Some((f, from)) = info.next_fee {
                if now >= from {
                    info.fee_ppm = f;
                    info.next_fee = None;
                }
            }
            let total = info.balance;
            let fee = mul_div(total, info.fee_ppm as u128, PPM);
            let to_share = total - fee;
            let units: u128 = shares.iter().map(|(_, w)| *w as u128).sum();
            ensure!(units > 0, Error::<T>::EmptyDistribution);
            let me = Self::account_id();
            let mut paid = 0u128;
            for (acct, w) in shares.iter() {
                let amt = mul_div(to_share, *w as u128, units);
                if amt > 0 {
                    T::WorkCurrency::transfer(&me, acct, amt, Preservation::Expendable)?;
                    paid += amt;
                }
            }
            let fee_paid = total - paid; // fee plus rounding dust goes to the operator
            if fee_paid > 0 {
                T::WorkCurrency::transfer(&me, &pool, fee_paid, Preservation::Expendable)?;
            }
            info.balance = 0;
            Pools::<T>::insert(&pool, info);
            Self::deposit_event(Event::PoolDistributed {
                pool,
                epoch,
                receipts_root,
                paid,
                fee: fee_paid,
            });
            Ok(())
        }

        /// A validator reports that miner `uid` failed a spot check in `epoch`.
        #[pallet::call_index(6)]
        #[pallet::weight(<T as Config>::WeightInfo::report_failed_check(T::MaxReporters::get()))]
        pub fn report_failed_check(
            origin: OriginFor<T>,
            epoch: Epoch,
            uid: Uid,
            evidence: H256,
        ) -> DispatchResult {
            let who = ensure_signed(origin)?;
            let stake = T::Stake::stake_of(&who);
            ensure!(stake > 0, Error::<T>::NotAValidator);
            let hotkey = Neurons::<T>::get(SN0, uid).ok_or(Error::<T>::UnknownUid)?;
            Reporters::<T>::try_mutate(epoch, uid, |r| -> DispatchResult {
                ensure!(!r.contains(&who), Error::<T>::AlreadyReported);
                r.try_push(who.clone())
                    .map_err(|_| Error::<T>::TooManyReporters)?;
                Ok(())
            })?;
            let reported = ReportedStake::<T>::mutate(epoch, uid, |s| {
                *s += stake;
                *s
            });
            Self::deposit_event(Event::FailedCheckReported {
                epoch,
                uid,
                reporter: who,
                evidence,
            });
            // Round the quorums up so "at least half" never passes with dust missing.
            let base = T::Stake::active_stake();
            let quorum = mul_div(base, T::QuorumPpm::get() as u128, 1).div_ceil(PPM);
            let slash_quorum = mul_div(base, T::SlashQuorumPpm::get() as u128, 1).div_ceil(PPM);
            if reported >= quorum && !Penalized::<T>::get(epoch, uid) {
                Self::penalize(epoch, uid, hotkey.clone())?;
            }
            if reported >= slash_quorum && !Slashed::<T>::get(epoch, uid) {
                Self::slash(epoch, uid, hotkey)?;
            }
            Ok(())
        }
    }

    impl<T: Config> Pallet<T> {
        pub fn account_id() -> T::AccountId {
            T::WorkPalletId::get().into_account_truncating()
        }

        fn slash(epoch: Epoch, uid: Uid, hotkey: T::AccountId) -> DispatchResult {
            Slashed::<T>::insert(epoch, uid, true);
            let bond = Bonds::<T>::get(&hotkey);
            let slash = mul_div(bond, T::SlashPpm::get() as u128, PPM);
            if slash > 0 {
                Bonds::<T>::insert(&hotkey, bond - slash);
                let burn = slash / 2;
                let me = Self::account_id();
                T::WorkCurrency::burn_from(
                    &me,
                    burn,
                    Preservation::Expendable,
                    Precision::BestEffort,
                    Fortitude::Force,
                )?;
                T::WorkCurrency::transfer(
                    &me,
                    &T::InsuranceFund::get(),
                    slash - burn,
                    Preservation::Expendable,
                )?;
            }
            Self::deposit_event(Event::BondSlashed {
                epoch,
                uid,
                hotkey,
                slashed: slash,
            });
            Ok(())
        }

        fn penalize(epoch: Epoch, uid: Uid, hotkey: T::AccountId) -> DispatchResult {
            Penalized::<T>::insert(epoch, uid, true);
            Self::deposit_event(Event::MinerPenalized {
                epoch,
                uid,
                hotkey: hotkey.clone(),
                slashed: 0,
            });
            let window_start = epoch.saturating_sub(T::FailureWindow::get());
            let count = Failures::<T>::mutate(&hotkey, |f| {
                f.retain(|e| *e >= window_start);
                let _ = f.try_push(epoch);
                f.len() as u32
            });
            if count >= T::MaxFailures::get() {
                Banned::<T>::insert(&hotkey, ());
                Self::deposit_event(Event::HotkeyBanned { hotkey });
            }
            Ok(())
        }

        pub fn is_penalized(epoch: Epoch, uid: Uid) -> bool {
            Penalized::<T>::get(epoch, uid)
        }

        pub fn is_banned(who: &T::AccountId) -> bool {
            Banned::<T>::contains_key(who)
        }

        /// Pay a miner reward from `from`: to the pool balance if the hotkey is a pool,
        /// otherwise straight to the hotkey.
        pub fn credit(from: &T::AccountId, hotkey: &T::AccountId, amount: u128) -> DispatchResult {
            if amount == 0 {
                return Ok(());
            }
            if Pools::<T>::contains_key(hotkey) {
                T::WorkCurrency::transfer(
                    from,
                    &Self::account_id(),
                    amount,
                    Preservation::Expendable,
                )?;
                Pools::<T>::mutate(hotkey, |p| {
                    if let Some(p) = p {
                        p.balance += amount;
                    }
                });
                Self::deposit_event(Event::PoolCredited {
                    pool: hotkey.clone(),
                    amount,
                });
            } else {
                T::WorkCurrency::transfer(from, hotkey, amount, Preservation::Expendable)?;
            }
            Ok(())
        }
    }
}
