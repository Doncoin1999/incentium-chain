//! # Incentium consensus pallet (spec Section 6, SN0 devnet version)
//!
//! - Validators stake ICT on their SN0 hotkey (unstaking waits `UnbondingEpochs`).
//! - At the first block of epoch E the pallet takes the weights committed in epoch E-2
//!   (revealed during E-1), drops banned validators and penalised miners, runs
//!   Yuma-lite (sqrt stake, 10 % cap, kappa 0.5, clipping) and pays out the subnet pot:
//!   `MinerSharePpm` to miners by incentive (pools via work-verify), the rest to
//!   validators by dividends. With no usable weights the pot carries over.
//! - Once subnet registration has unlocked (subnet registration specification, 25 Nov 2026):
//!   - validators of SN1 and later stake on their own subnet (`stake_subnet`);
//!   - each epoch the pot is shared between SN0 and the other subnets. The other subnets'
//!     combined share ramps from 0 to its maximum over 180 days from the unlock, shared by
//!     their stake. SN0 keeps at least `Sn0MinPotPpm` of the pot (50 % of all issuance);
//!   - SN1 and later pay 50 % to miners, 38 % to validators and 12 % to the owner;
//!   - SN0 has no owner and splits 50 : 38 between miners and validators
//!     (`Sn0MinerSharePpm`). Before the unlock SN0 keeps `MinerSharePpm`.
//! - Closing a subnet releases its stake into the normal unbonding queue.
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
    use pallet_subnet_registry::{NetUid, NeuronCount, NeuronUid, Neurons, Subnets, SN0};
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

    /// SN1 and later: miners 50 %, validators 38 %, owner 12 % of the subnet's slice.
    pub const SUBNET_MINER_PPM: u128 = 500_000;
    pub const SUBNET_OWNER_PPM: u128 = 120_000;
    /// The other subnets' share ramps in over 180 days from the unlock.
    pub const EMISSION_RAMP_BLOCKS: u128 = 180 * ict_economics::BLOCKS_PER_DAY as u128;

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
        /// SN0 miner share of each payout before subnet registration unlocks, in ppm
        /// (Phase 1: 60 / 90 of issuance = 666,667).
        #[pallet::constant]
        type MinerSharePpm: Get<u32>;
        /// SN0 miner share after the unlock, in ppm (50 / 88 = 568,182; the rest to validators).
        #[pallet::constant]
        type Sn0MinerSharePpm: Get<u32>;
        /// Smallest share of the pot kept by SN0 after the unlock, in ppm. The runtime sets it
        /// so that SN0 keeps 50 % of all issuance (the development fund takes its share first).
        type Sn0MinPotPpm: Get<u32>;
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

    /// Stake on SN1 and later subnets: (netuid, validator hotkey) -> ICT.
    #[pallet::storage]
    pub type SubnetStake<T: Config> =
        StorageDoubleMap<_, Twox64Concat, NetUid, Blake2_128Concat, T::AccountId, u128, ValueQuery>;

    /// Total stake per subnet (SN1 and later); the pot is shared by it.
    #[pallet::storage]
    pub type SubnetTotalStake<T> = StorageMap<_, Twox64Concat, NetUid, u128, ValueQuery>;

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
        /// A subnet (SN1 and later) was settled.
        SubnetEpochSettled {
            netuid: NetUid,
            epoch: Epoch,
            paid: u128,
            miners: u32,
            validators: u32,
            owner: u128,
        },
        /// A subnet had no usable weights; its slice stays in the pot.
        SubnetEpochSkipped {
            netuid: NetUid,
            epoch: Epoch,
        },
        /// Stake on a subnet (SN1 and later).
        SubnetStaked {
            netuid: NetUid,
            who: T::AccountId,
            amount: u128,
        },
        /// A closed subnet's stake moved to the unbonding queue.
        SubnetStakeReleased {
            netuid: NetUid,
            stakers: u32,
            amount: u128,
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
        /// The caller is not a neuron of that subnet.
        NotSubnetNeuron,
        /// Use `stake` / `unstake` for SN0.
        UseSn0Calls,
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
            let n: u32 = Subnets::<T>::iter_keys()
                .map(|net| NeuronCount::<T>::get(net) as u32)
                .sum();
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
            let at = Self::queue_unbonding(&who, amount);
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

        /// Stake ICT on the caller's validator hotkey in subnet `netuid` (SN1 and later).
        #[pallet::call_index(4)]
        #[pallet::weight(<T as Config>::WeightInfo::stake())]
        pub fn stake_subnet(origin: OriginFor<T>, netuid: NetUid, amount: u128) -> DispatchResult {
            let who = ensure_signed(origin)?;
            ensure!(netuid != SN0, Error::<T>::UseSn0Calls);
            ensure!(
                NeuronUid::<T>::contains_key(netuid, &who),
                Error::<T>::NotSubnetNeuron
            );
            let new = SubnetStake::<T>::get(netuid, &who) + amount;
            ensure!(
                new >= T::MinValidatorStake::get(),
                Error::<T>::BelowMinimumStake
            );
            T::StakeCurrency::transfer(&who, &Self::account_id(), amount, Preservation::Preserve)?;
            SubnetStake::<T>::insert(netuid, &who, new);
            SubnetTotalStake::<T>::mutate(netuid, |t| *t += amount);
            Self::deposit_event(Event::SubnetStaked {
                netuid,
                who,
                amount,
            });
            Ok(())
        }

        /// Start unbonding stake from subnet `netuid` (SN1 and later).
        #[pallet::call_index(5)]
        #[pallet::weight(<T as Config>::WeightInfo::unstake())]
        pub fn unstake_subnet(
            origin: OriginFor<T>,
            netuid: NetUid,
            amount: u128,
        ) -> DispatchResult {
            let who = ensure_signed(origin)?;
            ensure!(netuid != SN0, Error::<T>::UseSn0Calls);
            let cur = SubnetStake::<T>::get(netuid, &who);
            ensure!(cur >= amount, Error::<T>::InsufficientStake);
            if cur == amount {
                SubnetStake::<T>::remove(netuid, &who);
            } else {
                SubnetStake::<T>::insert(netuid, &who, cur - amount);
            }
            SubnetTotalStake::<T>::mutate(netuid, |t| *t = t.saturating_sub(amount));
            let at = Self::queue_unbonding(&who, amount);
            Self::deposit_event(Event::UnstakeRequested {
                who,
                amount,
                available_at: at,
            });
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

        /// Add `amount` to `who`'s unbonding queue; returns the epoch it becomes available.
        fn queue_unbonding(who: &T::AccountId, amount: u128) -> Epoch {
            let at = pallet_weights::Pallet::<T>::current_epoch() + T::StakeUnbondingEpochs::get();
            Unstaking::<T>::mutate(who, |u| {
                let prev = u.map(|x| x.0).unwrap_or(0);
                *u = Some((prev + amount, at));
            });
            at
        }

        fn stake_on(netuid: NetUid, who: &T::AccountId) -> u128 {
            if netuid == SN0 {
                Stake::<T>::get(who)
            } else {
                SubnetStake::<T>::get(netuid, who)
            }
        }

        /// Share of the pot that SN1 and later subnets get together now, in ppm: 0 before the
        /// unlock, then rising linearly over 180 days to `PPM - Sn0MinPotPpm`.
        pub fn others_share_ppm() -> u128 {
            let Some(unlocked) = pallet_subnet_registry::UnlockedAt::<T>::get() else {
                return 0;
            };
            if !pallet_subnet_registry::Phase2Enabled::<T>::get() {
                return 0;
            }
            let now: u64 = polkadot_sdk::frame_system::Pallet::<T>::block_number().saturated_into();
            let elapsed = now.saturating_sub(unlocked as u64) as u128;
            let ramp = mul_div(PPM, elapsed, EMISSION_RAMP_BLOCKS).min(PPM);
            let max = PPM.saturating_sub(T::Sn0MinPotPpm::get() as u128);
            mul_div(max, ramp, PPM)
        }

        /// How the pot is shared this epoch: SN0 first, then each staked subnet by its stake.
        /// Subnets without stake get nothing; if none has stake, SN0 gets the whole pot.
        pub fn allocation(pot: u128) -> Vec<(NetUid, u128)> {
            let share = Self::others_share_ppm();
            let staked: Vec<(NetUid, u128)> = if share == 0 {
                Vec::new()
            } else {
                Subnets::<T>::iter_keys()
                    .filter(|n| *n != SN0)
                    .map(|n| (n, SubnetTotalStake::<T>::get(n)))
                    .filter(|(_, st)| *st > 0)
                    .collect()
            };
            let total: u128 = staked.iter().map(|(_, s)| *s).sum();
            if total == 0 {
                return alloc::vec![(SN0, pot)];
            }
            let others = mul_div(pot, share, PPM);
            let mut out = Vec::with_capacity(staked.len() + 1);
            let mut given = 0u128;
            for (n, st) in staked {
                let slice = mul_div(others, st, total);
                given += slice;
                out.push((n, slice));
            }
            out.insert(0, (SN0, pot - given));
            out
        }

        /// Settle weight epoch `we` for every subnet.
        pub fn settle(we: Epoch) {
            let pot_acct = T::SubnetPot::get();
            let pot = T::StakeCurrency::reducible_balance(
                &pot_acct,
                Preservation::Expendable,
                Fortitude::Polite,
            );
            LastSettled::<T>::put(we);
            let unlocked = pallet_subnet_registry::Phase2Enabled::<T>::get();
            let plan = Self::allocation(pot);
            let mut planned = alloc::collections::BTreeSet::new();
            for (netuid, slice) in plan {
                planned.insert(netuid);
                if netuid == SN0 {
                    let miner_ppm = if unlocked {
                        T::Sn0MinerSharePpm::get()
                    } else {
                        T::MinerSharePpm::get()
                    };
                    Self::settle_subnet(SN0, we, slice, miner_ppm as u128, None);
                } else {
                    let owner = Subnets::<T>::get(netuid).and_then(|i| i.owner);
                    Self::settle_subnet(
                        netuid,
                        we,
                        slice,
                        SUBNET_MINER_PPM,
                        owner.map(|o| (o, SUBNET_OWNER_PPM)),
                    );
                }
            }
            // Subnets without a slice still drop their revealed weights.
            for netuid in Subnets::<T>::iter_keys() {
                if !planned.contains(&netuid) {
                    pallet_weights::Pallet::<T>::prune(netuid, we);
                }
            }
        }

        /// Settle one subnet's `slice` of the pot: `owner_ppm` of it to the owner, `miner_ppm`
        /// to miners by incentive, the rest to validators by dividends. With no usable weights
        /// the slice stays in the pot.
        fn settle_subnet(
            netuid: NetUid,
            we: Epoch,
            slice: u128,
            miner_ppm: u128,
            owner: Option<(T::AccountId, u128)>,
        ) {
            let rows = pallet_weights::Pallet::<T>::revealed(netuid, we);
            let n = NeuronCount::<T>::get(netuid) as usize;
            let mut validators: Vec<T::AccountId> = Vec::new();
            let mut stakes: Vec<u128> = Vec::new();
            let mut matrix: Vec<Vec<u128>> = Vec::new();
            for (v, row) in rows {
                let st = Self::stake_on(netuid, &v);
                if st == 0 || pallet_work_verify::Pallet::<T>::is_banned(&v) {
                    continue;
                }
                let mut r = alloc::vec![0u128; n];
                for (uid, w) in row {
                    let u = uid as usize;
                    // SN0: spot checks in work-verify. Later subnets: their own validators'
                    // failed-work reports, if the subnet turned penalties on.
                    let penalized = if netuid == SN0 {
                        pallet_work_verify::Pallet::<T>::is_penalized(we, uid)
                    } else {
                        pallet_subnet_registry::Pallet::<T>::is_work_penalized(netuid, we, uid)
                    };
                    if u < n && !penalized {
                        r[u] = w as u128;
                    }
                }
                validators.push(v);
                stakes.push(st);
                matrix.push(r);
            }
            pallet_weights::Pallet::<T>::prune(netuid, we);

            let out = yuma_lite(
                &stakes,
                &matrix,
                n,
                T::KappaPpm::get() as u128,
                T::MaxValidatorPpm::get() as u128,
            );
            let pot_acct = T::SubnetPot::get();
            if slice == 0 || out.incentive.iter().all(|x| *x == 0) {
                if netuid == SN0 {
                    pallet_subnet_registry::Pallet::<T>::note_sn0_epoch(0);
                    Self::deposit_event(Event::EpochSkipped { epoch: we });
                } else {
                    Self::deposit_event(Event::SubnetEpochSkipped { netuid, epoch: we });
                }
                return;
            }
            // Work from an SN0 epoch that started in the launch period is paid into the vault.
            let len = <T as pallet_weights::Config>::EpochLength::get() as u64;
            let launch = netuid == SN0
                && we.saturating_mul(len)
                    < pallet_subnet_registry::BootstrapUntil::<T>::get() as u64;
            let mut paid = 0u128;
            let mut to_owner = 0u128;
            if let Some((o, ppm)) = &owner {
                let amt = mul_div(slice, *ppm, PPM);
                if amt > 0
                    && T::StakeCurrency::transfer(&pot_acct, o, amt, Preservation::Expendable)
                        .is_ok()
                {
                    to_owner = amt;
                    paid += amt;
                }
            }
            let to_miners = mul_div(slice, miner_ppm, PPM);
            let owner_part = owner
                .as_ref()
                .map(|(_, p)| mul_div(slice, *p, PPM))
                .unwrap_or(0);
            let to_validators = slice.saturating_sub(to_miners).saturating_sub(owner_part);
            let mut miners = 0u32;
            for (uid, inc) in out.incentive.iter().enumerate() {
                if *inc == 0 {
                    continue;
                }
                if let Some(hk) = Neurons::<T>::get(netuid, uid as u16) {
                    if pallet_work_verify::Pallet::<T>::is_banned(&hk)
                        || pallet_subnet_registry::Pallet::<T>::is_banned_from(netuid, &hk)
                    {
                        continue;
                    }
                    let amt = mul_div(to_miners, *inc, PPM);
                    if Self::pay(&pot_acct, &hk, amt, launch, netuid == SN0) {
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
            if netuid == SN0 {
                // Counts towards the SN1 unlock criterion (300 active miners, 30-day average).
                pallet_subnet_registry::Pallet::<T>::note_sn0_epoch(miners);
                Self::deposit_event(Event::EpochSettled {
                    epoch: we,
                    paid,
                    miners,
                    validators: vals,
                });
            } else {
                Self::deposit_event(Event::SubnetEpochSettled {
                    netuid,
                    epoch: we,
                    paid,
                    miners,
                    validators: vals,
                    owner: to_owner,
                });
            }
        }
    }

    impl<T: Config> pallet_subnet_registry::SubnetStakeInfo<T::AccountId> for Pallet<T> {
        fn stake_of(netuid: u16, who: &T::AccountId) -> u128 {
            Self::stake_on(netuid, who)
        }
        fn total(netuid: u16) -> u128 {
            if netuid == SN0 {
                TotalStake::<T>::get()
            } else {
                SubnetTotalStake::<T>::get(netuid)
            }
        }
    }

    impl<T: Config> pallet_subnet_registry::OnSubnetRemoved for Pallet<T> {
        /// A closed subnet's stake moves to the unbonding queue of each staker.
        fn subnet_removed(netuid: u16) {
            if netuid == SN0 {
                return;
            }
            let mut stakers = 0u32;
            let mut amount = 0u128;
            for (who, st) in SubnetStake::<T>::drain_prefix(netuid) {
                Self::queue_unbonding(&who, st);
                stakers += 1;
                amount += st;
            }
            SubnetTotalStake::<T>::remove(netuid);
            Self::deposit_event(Event::SubnetStakeReleased {
                netuid,
                stakers,
                amount,
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
