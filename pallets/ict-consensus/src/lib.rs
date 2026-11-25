//! # Incentium consensus pallet (spec Section 6, SN0 devnet version)
//!
//! - Validators stake ICT on their SN0 hotkey (unstaking waits `UnbondingEpochs`).
//! - At the first block of epoch E the pallet takes the weights committed in epoch E-2
//!   (revealed during E-1), drops banned validators and penalised miners, runs
//!   Yuma-lite (sqrt stake, 10 % cap, kappa 0.5, clipping) and pays out the subnet pot:
//!   `MinerSharePpm` to miners by incentive (pools via work-verify), the rest to
//!   validators by dividends. With no usable weights the pot carries over.
//! - The on-chain computation is replaced by off-chain Yuma with a zk proof later.
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

#[polkadot_sdk::frame_support::pallet]
pub mod pallet {
    use crate::WeightInfo;
    use ict_economics::{mul_div, yuma::yuma_lite, PPM};
    use pallet_subnet_registry::{NeuronCount, NeuronUid, Neurons, SN0};
    use pallet_weights::Epoch;
    use polkadot_sdk::frame_support::{
        pallet_prelude::*,
        traits::{
            fungible::{Inspect, Mutate},
            tokens::{Fortitude, Preservation},
        },
        PalletId,
    };
    use polkadot_sdk::frame_system::pallet_prelude::*;
    use polkadot_sdk::sp_runtime::traits::{AccountIdConversion, SaturatedConversion};
    extern crate alloc;
    use alloc::vec::Vec;

    #[pallet::pallet]
    pub struct Pallet<T>(_);

    #[pallet::config]
    pub trait Config: polkadot_sdk::frame_system::Config + pallet_work_verify::Config {
        #[allow(deprecated)]
        type RuntimeEvent: From<Event<Self>>
            + IsType<<Self as polkadot_sdk::frame_system::Config>::RuntimeEvent>;
        /// Weights of this pallet's calls (benchmarked in the runtime).
        type WeightInfo: WeightInfo;
        type StakeCurrency: Mutate<Self::AccountId, Balance = u128>;
        #[pallet::constant]
        type StakePalletId: Get<PalletId>;
        /// Account where `ict-emission` mints subnet issuance.
        #[pallet::constant]
        type SubnetPot: Get<Self::AccountId>;
        /// Miner share of each payout in ppm (Phase 1: 60 / 90 of issuance = 666,667).
        #[pallet::constant]
        type MinerSharePpm: Get<u32>;
        #[pallet::constant]
        type KappaPpm: Get<u32>;
        #[pallet::constant]
        type MaxValidatorPpm: Get<u32>;
        #[pallet::constant]
        type MinValidatorStake: Get<u128>;
        #[pallet::constant]
        type StakeUnbondingEpochs: Get<Epoch>;
        /// Vault for locked launch-period rewards (must differ from `StakePalletId`).
        #[pallet::constant]
        type LaunchPalletId: Get<PalletId>;
        /// Launch-period rewards: blocks fully locked after the period ends (2 months).
        #[pallet::constant]
        type LaunchCliff: Get<u64>;
        /// Blocks between unlock steps (2 months).
        #[pallet::constant]
        type LaunchStep: Get<u64>;
        /// Share released per step, in ppm (10 %).
        #[pallet::constant]
        type LaunchStepPpm: Get<u32>;
        /// Share of each launch-period reward paid out at once, in ppm (25 %); the rest is
        /// locked in the launch vault.
        #[pallet::constant]
        type LaunchImmediatePpm: Get<u32>;
    }

    #[pallet::storage]
    pub type Stake<T: Config> = StorageMap<_, Blake2_128Concat, T::AccountId, u128, ValueQuery>;

    #[pallet::storage]
    pub type TotalStake<T> = StorageValue<_, u128, ValueQuery>;

    #[pallet::storage]
    pub type Unstaking<T: Config> =
        StorageMap<_, Blake2_128Concat, T::AccountId, (u128, Epoch), OptionQuery>;

    /// Last weight epoch that was settled.
    #[pallet::storage]
    pub type LastSettled<T> = StorageValue<_, Epoch, OptionQuery>;

    /// Rewards earned in the launch period, held in the launch vault per account.
    #[pallet::storage]
    pub type LaunchLocked<T: Config> =
        StorageMap<_, Blake2_128Concat, T::AccountId, u128, ValueQuery>;

    /// Launch-period rewards already released to each account.
    #[pallet::storage]
    pub type LaunchClaimed<T: Config> =
        StorageMap<_, Blake2_128Concat, T::AccountId, u128, ValueQuery>;

    #[pallet::event]
    #[pallet::generate_deposit(pub(super) fn deposit_event)]
    pub enum Event<T: Config> {
        Staked {
            who: T::AccountId,
            amount: u128,
        },
        UnstakeRequested {
            who: T::AccountId,
            amount: u128,
            available_at: Epoch,
        },
        Withdrawn {
            who: T::AccountId,
            amount: u128,
        },
        /// An epoch was settled: `paid` ICT to `miners` miners and `validators` validators.
        EpochSettled {
            epoch: Epoch,
            paid: u128,
            miners: u32,
            validators: u32,
        },
        /// No usable weights; the pot carries over.
        EpochSkipped {
            epoch: Epoch,
        },
        /// Launch-period reward locked in the vault for `who`.
        LaunchRewardLocked {
            who: T::AccountId,
            amount: u128,
        },
        /// Unlocked launch-period rewards released to `who`.
        LaunchRewardClaimed {
            who: T::AccountId,
            amount: u128,
        },
    }

    #[pallet::error]
    pub enum Error<T> {
        NotSn0Neuron,
        BelowMinimumStake,
        InsufficientStake,
        NothingToWithdraw,
        NotYetUnbonded,
        /// Nothing is unlocked yet for this account.
        NothingUnlocked,
    }

    #[pallet::hooks]
    impl<T: Config> Hooks<BlockNumberFor<T>> for Pallet<T> {
        fn on_initialize(n: BlockNumberFor<T>) -> Weight {
            let n: u64 = n.saturated_into();
            let len = <T as pallet_weights::Config>::EpochLength::get() as u64;
            if len == 0 || !n.is_multiple_of(len) {
                return T::DbWeight::get().reads(1);
            }
            let epoch = n / len;
            if epoch < 2 {
                return T::DbWeight::get().reads(1);
            }
            // Every validator is a neuron, so n bounds both the rows and the payouts.
            let n = NeuronCount::<T>::get(SN0) as u32;
            Self::settle(epoch - 2);
            <T as Config>::WeightInfo::settle(n, n)
        }
    }

    #[pallet::call]
    impl<T: Config> Pallet<T> {
        /// Stake ICT on the caller's SN0 validator hotkey.
        #[pallet::call_index(0)]
        #[pallet::weight(<T as Config>::WeightInfo::stake())]
        pub fn stake(origin: OriginFor<T>, amount: u128) -> DispatchResult {
            let who = ensure_signed(origin)?;
            ensure!(
                NeuronUid::<T>::contains_key(SN0, &who),
                Error::<T>::NotSn0Neuron
            );
            let new = Stake::<T>::get(&who) + amount;
            // The minimum is waived during the disclosed bootstrap period, when the
            // operator's validators start from the small genesis allocation.
            ensure!(
                new >= T::MinValidatorStake::get()
                    || pallet_subnet_registry::Pallet::<T>::in_bootstrap(),
                Error::<T>::BelowMinimumStake
            );
            T::StakeCurrency::transfer(&who, &Self::account_id(), amount, Preservation::Preserve)?;
            Stake::<T>::insert(&who, new);
            TotalStake::<T>::mutate(|t| *t += amount);
            Self::deposit_event(Event::Staked { who, amount });
            Ok(())
        }

        #[pallet::call_index(1)]
        #[pallet::weight(<T as Config>::WeightInfo::unstake())]
        pub fn unstake(origin: OriginFor<T>, amount: u128) -> DispatchResult {
            let who = ensure_signed(origin)?;
            let cur = Stake::<T>::get(&who);
            ensure!(cur >= amount, Error::<T>::InsufficientStake);
            Stake::<T>::insert(&who, cur - amount);
            TotalStake::<T>::mutate(|t| *t -= amount);
            let at = pallet_weights::Pallet::<T>::current_epoch() + T::StakeUnbondingEpochs::get();
            Unstaking::<T>::mutate(&who, |u| {
                let prev = u.map(|x| x.0).unwrap_or(0);
                *u = Some((prev + amount, at));
            });
            Self::deposit_event(Event::UnstakeRequested {
                who,
                amount,
                available_at: at,
            });
            Ok(())
        }

        #[pallet::call_index(2)]
        #[pallet::weight(<T as Config>::WeightInfo::withdraw_unstaked())]
        pub fn withdraw_unstaked(origin: OriginFor<T>) -> DispatchResult {
            let who = ensure_signed(origin)?;
            let (amount, at) = Unstaking::<T>::get(&who).ok_or(Error::<T>::NothingToWithdraw)?;
            ensure!(
                pallet_weights::Pallet::<T>::current_epoch() >= at,
                Error::<T>::NotYetUnbonded
            );
            Unstaking::<T>::remove(&who);
            T::StakeCurrency::transfer(
                &Self::account_id(),
                &who,
                amount,
                Preservation::Expendable,
            )?;
            Self::deposit_event(Event::Withdrawn { who, amount });
            Ok(())
        }

        /// Release the unlocked part of the caller's locked launch-period rewards:
        /// fully locked for `LaunchCliff` blocks after the launch period, then
        /// `LaunchStepPpm` at the end of the cliff and every `LaunchStep` blocks after it.
        #[pallet::call_index(3)]
        #[pallet::weight(<T as Config>::WeightInfo::claim_launch_rewards())]
        pub fn claim_launch_rewards(origin: OriginFor<T>) -> DispatchResult {
            let who = ensure_signed(origin)?;
            let amount = Self::launch_claimable(&who);
            ensure!(amount > 0, Error::<T>::NothingUnlocked);
            T::StakeCurrency::transfer(
                &Self::launch_vault(),
                &who,
                amount,
                Preservation::Expendable,
            )?;
            LaunchClaimed::<T>::mutate(&who, |c| *c += amount);
            Self::deposit_event(Event::LaunchRewardClaimed { who, amount });
            Ok(())
        }
    }

    impl<T: Config> Pallet<T> {
        pub fn account_id() -> T::AccountId {
            T::StakePalletId::get().into_account_truncating()
        }

        /// Vault holding locked launch-period rewards.
        pub fn launch_vault() -> T::AccountId {
            T::LaunchPalletId::get().into_account_truncating()
        }

        /// ICT that `who` can claim from the launch vault right now.
        pub fn launch_claimable(who: &T::AccountId) -> u128 {
            if pallet_subnet_registry::Pallet::<T>::in_bootstrap() {
                return 0;
            }
            let end = pallet_subnet_registry::BootstrapUntil::<T>::get() as u64;
            let now: u64 = polkadot_sdk::frame_system::Pallet::<T>::block_number().saturated_into();
            let ppm = ict_economics::registry::launch_unlocked_ppm(
                now.saturating_sub(end),
                T::LaunchCliff::get(),
                T::LaunchStep::get(),
                T::LaunchStepPpm::get() as u128,
            );
            mul_div(LaunchLocked::<T>::get(who), ppm, PPM)
                .saturating_sub(LaunchClaimed::<T>::get(who))
        }

        /// Pay a reward. Work done in the launch period: `LaunchImmediatePpm` of it is paid at
        /// once and the rest goes into the launch vault. Otherwise all of it is paid at once.
        /// Payments to miners go through work-verify, so pools are credited.
        fn pay(
            from: &T::AccountId,
            to: &T::AccountId,
            amount: u128,
            launch: bool,
            miner: bool,
        ) -> bool {
            let now = if launch {
                mul_div(amount, T::LaunchImmediatePpm::get() as u128, PPM)
            } else {
                amount
            };
            let locked = amount - now;
            if locked > 0 {
                if T::StakeCurrency::transfer(
                    from,
                    &Self::launch_vault(),
                    locked,
                    Preservation::Expendable,
                )
                .is_err()
                {
                    return false;
                }
                LaunchLocked::<T>::mutate(to, |l| *l += locked);
                Self::deposit_event(Event::LaunchRewardLocked {
                    who: to.clone(),
                    amount: locked,
                });
            }
            if now == 0 {
                true
            } else if miner {
                pallet_work_verify::Pallet::<T>::credit(from, to, now).is_ok()
            } else {
                T::StakeCurrency::transfer(from, to, now, Preservation::Expendable).is_ok()
            }
        }

        /// Settle weight epoch `we` for SN0.
        pub fn settle(we: Epoch) {
            let rows = pallet_weights::Pallet::<T>::revealed(SN0, we);
            let n = NeuronCount::<T>::get(SN0) as usize;
            let mut validators: Vec<T::AccountId> = Vec::new();
            let mut stakes: Vec<u128> = Vec::new();
            let mut matrix: Vec<Vec<u128>> = Vec::new();
            for (v, row) in rows {
                let st = Stake::<T>::get(&v);
                if st == 0 || pallet_work_verify::Pallet::<T>::is_banned(&v) {
                    continue;
                }
                let mut r = alloc::vec![0u128; n];
                for (uid, w) in row {
                    let u = uid as usize;
                    if u < n && !pallet_work_verify::Pallet::<T>::is_penalized(we, uid) {
                        r[u] = w as u128;
                    }
                }
                validators.push(v);
                stakes.push(st);
                matrix.push(r);
            }
            pallet_weights::Pallet::<T>::prune(SN0, we);
            LastSettled::<T>::put(we);

            let out = yuma_lite(
                &stakes,
                &matrix,
                n,
                T::KappaPpm::get() as u128,
                T::MaxValidatorPpm::get() as u128,
            );
            let pot_acct = T::SubnetPot::get();
            let pot = T::StakeCurrency::reducible_balance(
                &pot_acct,
                Preservation::Expendable,
                Fortitude::Polite,
            );
            if pot == 0 || out.incentive.iter().all(|x| *x == 0) {
                pallet_subnet_registry::Pallet::<T>::note_sn0_epoch(0);
                Self::deposit_event(Event::EpochSkipped { epoch: we });
                return;
            }
            // Work from an epoch that started in the launch period is paid into the vault.
            let len = <T as pallet_weights::Config>::EpochLength::get() as u64;
            let launch =
                we.saturating_mul(len) < pallet_subnet_registry::BootstrapUntil::<T>::get() as u64;
            let to_miners = mul_div(pot, T::MinerSharePpm::get() as u128, PPM);
            let to_validators = pot - to_miners;
            let mut paid = 0u128;
            let mut miners = 0u32;
            for (uid, inc) in out.incentive.iter().enumerate() {
                if *inc == 0 {
                    continue;
                }
                if let Some(hk) = Neurons::<T>::get(SN0, uid as u16) {
                    if pallet_work_verify::Pallet::<T>::is_banned(&hk) {
                        continue;
                    }
                    let amt = mul_div(to_miners, *inc, PPM);
                    if Self::pay(&pot_acct, &hk, amt, launch, true) {
                        paid += amt;
                        miners += 1;
                    }
                }
            }
            let mut vals = 0u32;
            for (i, d) in out.dividends.iter().enumerate() {
                let amt = mul_div(to_validators, *d, PPM);
                if amt > 0 && Self::pay(&pot_acct, &validators[i], amt, launch, false) {
                    paid += amt;
                    vals += 1;
                }
            }
            // Counts towards the SN1 unlock criterion (300 active miners, 30-day average).
            pallet_subnet_registry::Pallet::<T>::note_sn0_epoch(miners);
            Self::deposit_event(Event::EpochSettled {
                epoch: we,
                paid,
                miners,
                validators: vals,
            });
        }
    }

    impl<T: Config> pallet_work_verify::StakeOracle<T::AccountId> for Pallet<T> {
        fn stake_of(who: &T::AccountId) -> u128 {
            Stake::<T>::get(who)
        }
        fn total_stake() -> u128 {
            TotalStake::<T>::get()
        }
        #[cfg(feature = "runtime-benchmarks")]
        fn set_stake(who: &T::AccountId, amount: u128) {
            let old = Stake::<T>::get(who);
            Stake::<T>::insert(who, amount);
            TotalStake::<T>::mutate(|t| *t = t.saturating_sub(old).saturating_add(amount));
        }
    }
}
