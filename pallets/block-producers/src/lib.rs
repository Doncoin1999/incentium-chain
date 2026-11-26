//! # Block producers (docs/OPEN-BLOCK-PRODUCTION.md, approved 26 Nov 2026)
//!
//! Block production is elected by `pallet_staking`. This pallet adds three Incentium rules:
//!
//! - **Genesis reserve.** The genesis authorities are added to every elected set until, in 30
//!   consecutive eras, at least 7 elected validators were not genesis keys. Then the reserve ends
//!   for good, the number of elected slots rises by the number of genesis keys, and they leave at
//!   the next new set. Root may end the reserve earlier, never restart it.
//! - **Offline.** A validator elected for an era that wrote no block in it is removed as a
//!   candidate when the next era starts. It can stand again with `validate`. No slash.
//! - **Minimum slash.** An offence reported to staking is slashed at least `MinSlash`.
//!
//! The pallet wraps staking as the session manager: `SessionManager = NoteHistoricalRoot<Runtime,
//! BlockProducers>`.
#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

pub use pallet::*;

#[polkadot_sdk::frame_support::pallet]
pub mod pallet {
    use alloc::vec::Vec;
    use polkadot_sdk::frame_support::pallet_prelude::*;
    use polkadot_sdk::frame_system::pallet_prelude::*;
    use polkadot_sdk::pallet_session::historical;
    use polkadot_sdk::pallet_staking;
    use polkadot_sdk::sp_runtime::Perbill;
    use polkadot_sdk::sp_staking::{
        offence::{OffenceDetails, OnOffenceHandler},
        EraIndex, SessionIndex,
    };

    #[pallet::pallet]
    pub struct Pallet<T>(_);

    #[pallet::config]
    pub trait Config: polkadot_sdk::frame_system::Config + pallet_staking::Config {
        #[allow(deprecated)]
        type RuntimeEvent: From<Event<Self>>
            + IsType<<Self as polkadot_sdk::frame_system::Config>::RuntimeEvent>;
        /// Independent validators needed in each era of the streak (7).
        #[pallet::constant]
        type MinIndependent: Get<u32>;
        /// Consecutive eras needed (30).
        #[pallet::constant]
        type StreakEras: Get<u32>;
        /// Largest number of genesis keys (bound for storage).
        #[pallet::constant]
        type MaxGenesis: Get<u32>;
    }

    /// The genesis authorities' validator ids.
    #[pallet::storage]
    pub type GenesisValidators<T: Config> =
        StorageValue<_, BoundedVec<T::AccountId, T::MaxGenesis>, ValueQuery>;

    /// True while the genesis keys keep reserved slots.
    #[pallet::storage]
    pub type ReserveActive<T> = StorageValue<_, bool, ValueQuery>;

    /// Consecutive eras with at least `MinIndependent` independent validators.
    #[pallet::storage]
    pub type Streak<T> = StorageValue<_, u32, ValueQuery>;

    /// Last era whose start was processed.
    #[pallet::storage]
    pub type LastEra<T> = StorageValue<_, EraIndex, OptionQuery>;

    #[pallet::genesis_config]
    #[derive(polkadot_sdk::frame_support::DefaultNoBound)]
    pub struct GenesisConfig<T: Config> {
        pub genesis_validators: Vec<T::AccountId>,
    }

    #[pallet::genesis_build]
    impl<T: Config> BuildGenesisConfig for GenesisConfig<T> {
        fn build(&self) {
            Pallet::<T>::set_genesis_validators(self.genesis_validators.clone());
        }
    }

    #[pallet::event]
    #[pallet::generate_deposit(pub(super) fn deposit_event)]
    pub enum Event<T: Config> {
        /// Elected validators that are not genesis keys in a new era, and the streak so far.
        EraCounted { era: EraIndex, independent: u32, streak: u32 },
        /// The genesis keys' reserve has ended for good.
        ReserveEnded { era: EraIndex },
        /// A validator wrote no block in an era and is no longer a candidate.
        ChilledOffline { stash: T::AccountId, era: EraIndex },
    }

    #[pallet::error]
    pub enum Error<T> {
        ReserveAlreadyEnded,
    }

    #[pallet::call]
    impl<T: Config> Pallet<T> {
        /// Root: end the genesis keys' reserve now. It can never be restarted.
        #[pallet::call_index(0)]
        #[pallet::weight(Weight::from_parts(20_000_000, 2_000))]
        pub fn end_reserve(origin: OriginFor<T>) -> DispatchResult {
            ensure_root(origin)?;
            ensure!(ReserveActive::<T>::get(), Error::<T>::ReserveAlreadyEnded);
            Self::finish_reserve(pallet_staking::ActiveEra::<T>::get().map(|e| e.index).unwrap_or(0));
            Ok(())
        }
    }

    impl<T: Config> Pallet<T> {
        /// Store the genesis keys and turn the reserve on (genesis and the live-chain migration).
        pub fn set_genesis_validators(v: Vec<T::AccountId>) {
            let bounded: BoundedVec<_, _> = v.try_into().expect("at most MaxGenesis genesis keys");
            ReserveActive::<T>::put(!bounded.is_empty());
            GenesisValidators::<T>::put(bounded);
        }

        fn finish_reserve(era: EraIndex) {
            ReserveActive::<T>::put(false);
            let extra = GenesisValidators::<T>::get().len() as u32;
            pallet_staking::ValidatorCount::<T>::mutate(|c| *c = c.saturating_add(extra));
            Self::deposit_event(Event::ReserveEnded { era });
        }

        fn with_reserve(mut set: Vec<T::AccountId>) -> Vec<T::AccountId> {
            if ReserveActive::<T>::get() {
                for g in GenesisValidators::<T>::get() {
                    if !set.contains(&g) {
                        set.push(g);
                    }
                }
            }
            set
        }

        /// Called at every session start: when a new era has begun, apply the offline rule to
        /// the era that ended and count independents in the new one.
        fn on_session_start() {
            let Some(active) = pallet_staking::ActiveEra::<T>::get() else { return };
            let era = active.index;
            if LastEra::<T>::get() == Some(era) {
                return;
            }
            LastEra::<T>::put(era);
            if era > 0 {
                Self::chill_offline(era - 1);
            }
            if ReserveActive::<T>::get() {
                let genesis = GenesisValidators::<T>::get();
                let independent = pallet_staking::ErasStakersOverview::<T>::iter_key_prefix(era)
                    .filter(|v| !genesis.contains(v))
                    .count() as u32;
                let streak = if independent >= T::MinIndependent::get() {
                    Streak::<T>::get().saturating_add(1)
                } else {
                    0
                };
                Streak::<T>::put(streak);
                Self::deposit_event(Event::EraCounted { era, independent, streak });
                if streak >= T::StreakEras::get() {
                    Self::finish_reserve(era);
                }
            }
        }

        /// Elected staking validators of `era` with no era points are chilled.
        fn chill_offline(era: EraIndex) {
            let points = pallet_staking::ErasRewardPoints::<T>::get(era);
            let elected: Vec<T::AccountId> =
                pallet_staking::ErasStakersOverview::<T>::iter_key_prefix(era).collect();
            for v in elected {
                let wrote = points.individual.get(&v).copied().unwrap_or(0) > 0;
                if !wrote && pallet_staking::Pallet::<T>::do_remove_validator(&v) {
                    Self::deposit_event(Event::ChilledOffline { stash: v, era });
                }
            }
        }
    }

    // ------------------------------------------------------------ read API

    impl<T: Config> Pallet<T>
    where
        T::CurrencyBalance: Into<u128>,
        T: polkadot_sdk::pallet_session::Config<
            ValidatorId = <T as polkadot_sdk::frame_system::Config>::AccountId,
        >,
    {
        /// Every candidate and elected validator, for incentium.org/validators.
        pub fn validators() -> Vec<crate::runtime_api::ValidatorInfo<T::AccountId>> {
            use alloc::collections::BTreeSet;
            let era = pallet_staking::ActiveEra::<T>::get().map(|e| e.index).unwrap_or(0);
            let last_points = if era > 0 {
                pallet_staking::ErasRewardPoints::<T>::get(era - 1)
            } else {
                Default::default()
            };
            let now_points = pallet_staking::ErasRewardPoints::<T>::get(era);
            let genesis = GenesisValidators::<T>::get();
            let session: BTreeSet<T::AccountId> =
                polkadot_sdk::pallet_session::Validators::<T>::get().into_iter().collect();
            let mut all: BTreeSet<T::AccountId> =
                pallet_staking::Validators::<T>::iter_keys().collect();
            all.extend(session.iter().cloned());
            all.into_iter()
                .map(|stash| {
                    let prefs = pallet_staking::Validators::<T>::get(&stash);
                    let bond: u128 = pallet_staking::Ledger::<T>::get(&stash)
                        .map(|l| l.active.into())
                        .unwrap_or(0);
                    let exposure = pallet_staking::ErasStakersOverview::<T>::get(era, &stash);
                    crate::runtime_api::ValidatorInfo {
                        candidate: pallet_staking::Validators::<T>::contains_key(&stash),
                        producing: session.contains(&stash),
                        genesis: genesis.contains(&stash),
                        self_bond: bond,
                        backing: exposure.as_ref().map(|e| e.total.into()).unwrap_or(0),
                        nominators: exposure.as_ref().map(|e| e.nominator_count).unwrap_or(0),
                        commission_ppb: prefs.commission.deconstruct(),
                        blocked: prefs.blocked,
                        points_last_era: last_points.individual.get(&stash).copied().unwrap_or(0),
                        points_this_era: now_points.individual.get(&stash).copied().unwrap_or(0),
                        stash,
                    }
                })
                .collect()
        }

        /// Network-wide figures; `pot` is the reward pot's balance (known to the runtime).
        pub fn overview(pot: u128) -> crate::runtime_api::Overview {
            let era = pallet_staking::ActiveEra::<T>::get().map(|e| e.index).unwrap_or(0);
            // fewest elected validators whose combined backing exceeds a third of all backing
            let mut backing: Vec<u128> = pallet_staking::ErasStakersOverview::<T>::iter_prefix(era)
                .map(|(_, e)| e.total.into())
                .collect();
            backing.sort_unstable_by(|a, b| b.cmp(a));
            let total: u128 = backing.iter().sum();
            let mut acc = 0u128;
            let mut to_halt = 0u32;
            for b in backing.iter() {
                if total > 0 && acc * 3 > total {
                    break;
                }
                acc += *b;
                to_halt += 1;
            }
            crate::runtime_api::Overview {
                era,
                slots: pallet_staking::ValidatorCount::<T>::get(),
                elected: backing.len() as u32,
                producing: polkadot_sdk::pallet_session::Validators::<T>::get().len() as u32,
                candidates: pallet_staking::Validators::<T>::count(),
                nominators: pallet_staking::Nominators::<T>::count(),
                total_backing: total,
                min_validator_bond: pallet_staking::MinValidatorBond::<T>::get().into(),
                min_nominator_bond: pallet_staking::MinNominatorBond::<T>::get().into(),
                reserve_active: ReserveActive::<T>::get(),
                streak: Streak::<T>::get(),
                streak_needed: T::StreakEras::get(),
                min_independent: T::MinIndependent::get(),
                pot,
                last_era_payout: if era > 0 {
                    pallet_staking::ErasValidatorReward::<T>::get(era - 1).map(Into::into).unwrap_or(0)
                } else {
                    0
                },
                to_halt: if total == 0 { 0 } else { to_halt },
            }
        }
    }

    // ------------------------------------------------------------ session manager

    impl<T: Config> polkadot_sdk::pallet_session::SessionManager<T::AccountId> for Pallet<T> {
        fn new_session(i: SessionIndex) -> Option<Vec<T::AccountId>> {
            <pallet_staking::Pallet<T> as polkadot_sdk::pallet_session::SessionManager<_>>::new_session(i)
                .map(Self::with_reserve)
        }
        fn new_session_genesis(i: SessionIndex) -> Option<Vec<T::AccountId>> {
            let set = <pallet_staking::Pallet<T> as polkadot_sdk::pallet_session::SessionManager<_>>::new_session_genesis(i)
                .unwrap_or_default();
            let set = Self::with_reserve(set);
            if set.is_empty() { None } else { Some(set) }
        }
        fn start_session(i: SessionIndex) {
            <pallet_staking::Pallet<T> as polkadot_sdk::pallet_session::SessionManager<_>>::start_session(i);
            Self::on_session_start();
        }
        fn end_session(i: SessionIndex) {
            <pallet_staking::Pallet<T> as polkadot_sdk::pallet_session::SessionManager<_>>::end_session(i);
        }
    }

    impl<T: Config> historical::SessionManager<T::AccountId, ()> for Pallet<T> {
        fn new_session(i: SessionIndex) -> Option<Vec<(T::AccountId, ())>> {
            <Self as polkadot_sdk::pallet_session::SessionManager<_>>::new_session(i)
                .map(|v| v.into_iter().map(|a| (a, ())).collect())
        }
        fn new_session_genesis(i: SessionIndex) -> Option<Vec<(T::AccountId, ())>> {
            <Self as polkadot_sdk::pallet_session::SessionManager<_>>::new_session_genesis(i)
                .map(|v| v.into_iter().map(|a| (a, ())).collect())
        }
        fn start_session(i: SessionIndex) {
            <Self as polkadot_sdk::pallet_session::SessionManager<_>>::start_session(i)
        }
        fn end_session(i: SessionIndex) {
            <Self as polkadot_sdk::pallet_session::SessionManager<_>>::end_session(i)
        }
    }

    // ------------------------------------------------------------ minimum slash

    /// Forwards offences to `Inner` with every slash fraction raised to at least `Min`.
    pub struct MinSlash<Inner, Min>(core::marker::PhantomData<(Inner, Min)>);
    impl<Reporter, Offender, Res, Inner, Min> OnOffenceHandler<Reporter, Offender, Res>
        for MinSlash<Inner, Min>
    where
        Inner: OnOffenceHandler<Reporter, Offender, Res>,
        Min: Get<Perbill>,
    {
        fn on_offence(
            offenders: &[OffenceDetails<Reporter, Offender>],
            slash_fraction: &[Perbill],
            session: SessionIndex,
        ) -> Res {
            let raised: Vec<Perbill> = slash_fraction.iter().map(|f| (*f).max(Min::get())).collect();
            Inner::on_offence(offenders, &raised, session)
        }
    }
}

pub mod runtime_api {
    extern crate alloc;
    use alloc::vec::Vec;
    use codec::{Codec, Decode, Encode};
    use scale_info::TypeInfo;

    /// One candidate or producing validator.
    #[derive(Clone, Encode, Decode, TypeInfo, Debug, PartialEq, Eq)]
    pub struct ValidatorInfo<AccountId> {
        pub stash: AccountId,
        /// Stands as a candidate in staking.
        pub candidate: bool,
        /// In the current session's block-producer set.
        pub producing: bool,
        /// One of the genesis keys.
        pub genesis: bool,
        pub self_bond: u128,
        /// Total backing in the current era (zero if not elected).
        pub backing: u128,
        pub nominators: u32,
        /// Commission in parts per billion.
        pub commission_ppb: u32,
        pub blocked: bool,
        pub points_last_era: u32,
        pub points_this_era: u32,
    }

    #[derive(Clone, Encode, Decode, TypeInfo, Debug, PartialEq, Eq)]
    pub struct Overview {
        pub era: u32,
        pub slots: u32,
        pub elected: u32,
        pub producing: u32,
        pub candidates: u32,
        pub nominators: u32,
        pub total_backing: u128,
        pub min_validator_bond: u128,
        pub min_nominator_bond: u128,
        pub reserve_active: bool,
        pub streak: u32,
        pub streak_needed: u32,
        pub min_independent: u32,
        /// Reward pot now (paid out at the end of the era).
        pub pot: u128,
        pub last_era_payout: u128,
        /// Fewest elected validators that together hold more than a third of the backing.
        pub to_halt: u32,
    }

    polkadot_sdk::sp_api::decl_runtime_apis! {
        pub trait BlockProducersApi<AccountId> where AccountId: Codec {
            fn overview() -> Overview;
            fn validators() -> Vec<ValidatorInfo<AccountId>>;
        }
    }
}
