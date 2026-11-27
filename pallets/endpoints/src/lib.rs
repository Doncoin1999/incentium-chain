//! # Endpoints pallet (spec Section 19.2)
//!
//! An open, on-chain register of public RPC endpoints, archive nodes and bootnodes, so web
//! pages, wallets and miners find many independent operators instead of a fixed list.
//!
//! - **Register.** Anyone registers an endpoint with a deposit held on its account (at most
//!   `MaxPerOwner` entries per account; a URL can be registered once). The owner can update or
//!   deregister it; the deposit is released on deregistration. Nobody approves an entry.
//! - **Probes.** Block producers and SN0 validators (`ReporterStake`) test registered endpoints
//!   during a day: an endpoint passes a probe when it answers in time with the chain's finalized
//!   block hash. The next day each reporter submits one report: per endpoint, the share of its
//!   probes that passed (0–100). A day's score for an endpoint is the stake-weighted median of
//!   the reports, and counts only when reporters holding more than half of all reporter stake
//!   reported on it, so a minority cannot raise or lower a score.
//! - **Payout.** `ict-emission` sends up to 5 % of issuance (`EndpointShare`) to this pallet's
//!   pot. At the start of each day the pallet settles the day before last: every endpoint with
//!   a score of at least `MinScore` gets `pot × weight × score / Σ(weight × score)`, with
//!   weight 1 for RPC, 2 for archive and 0 for bootnodes, capped per owner at `OperatorCap` of
//!   the pot. Whatever is not paid goes to the development fund, so the share is a ceiling.
//! - **Inactive.** An endpoint that scores below 50 % for `InactiveAfterDays` days in a row is
//!   marked inactive, and clients skip it; it is active again after a day at or above
//!   `MinScore`. There is no slashing.
//!
//! Deposit, `MinScore` and `OperatorCap` are set by `ParamsOrigin` (the Parameters track).
#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

pub use pallet::*;
pub mod weights;
pub use weights::WeightInfo;

#[cfg(test)]
mod mock;
#[cfg(test)]
mod tests;

/// Stake of the accounts whose probe reports count: block producers and SN0 validators.
pub trait ReporterStake<AccountId> {
    /// Stake behind `who` as a reporter; 0 if `who` may not report.
    fn stake_of(who: &AccountId) -> u128;
    /// Stake of all accounts that may report.
    fn total() -> u128;
}

impl<AccountId> ReporterStake<AccountId> for () {
    fn stake_of(_who: &AccountId) -> u128 {
        0
    }
    fn total() -> u128 {
        0
    }
}

#[polkadot_sdk::frame_support::pallet]
pub mod pallet {
    use super::{ReporterStake, WeightInfo};
    use alloc::vec::Vec;
    use codec::{Decode, DecodeWithMemTracking, Encode, MaxEncodedLen};
    use polkadot_sdk::frame_support::{
        pallet_prelude::*,
        traits::{
            fungible::{Inspect, Mutate, MutateHold},
            tokens::{Fortitude, Precision, Preservation},
        },
        CloneNoBound, DebugNoBound, DefaultNoBound, EqNoBound, PartialEqNoBound,
    };
    use polkadot_sdk::frame_system::pallet_prelude::*;
    use polkadot_sdk::sp_runtime::traits::Zero;
    use scale_info::TypeInfo;

    pub type EndpointId = u32;
    pub type Day = u32;

    /// Scores are percentages, 0–100.
    pub const FULL_SCORE: u8 = 100;
    /// Below this score a day counts towards marking the endpoint inactive.
    pub const FAILING_SCORE: u8 = 50;

    #[derive(
        Encode,
        Decode,
        DecodeWithMemTracking,
        Clone,
        Copy,
        PartialEq,
        Eq,
        Debug,
        TypeInfo,
        MaxEncodedLen,
    )]
    pub enum Kind {
        /// A public RPC endpoint (`wss://` or `https://`).
        Rpc,
        /// An archive node with the full history, also serving RPC.
        Archive,
        /// A bootnode (multiaddress); listed, not paid.
        Bootnode,
    }

    impl Kind {
        pub fn weight(self) -> u128 {
            match self {
                Kind::Rpc => 1,
                Kind::Archive => 2,
                Kind::Bootnode => 0,
            }
        }
    }

    #[derive(
        Encode,
        Decode,
        DecodeWithMemTracking,
        CloneNoBound,
        PartialEqNoBound,
        EqNoBound,
        DebugNoBound,
        TypeInfo,
        MaxEncodedLen,
    )]
    #[scale_info(skip_type_params(T))]
    pub struct Entry<T: Config> {
        pub owner: T::AccountId,
        pub kind: Kind,
        pub url: BoundedVec<u8, T::MaxUrlLen>,
        /// Free text such as "eu-west" or "NO"; clients may use it to prefer nearby endpoints.
        pub region: BoundedVec<u8, ConstU32<16>>,
        pub deposit: u128,
        pub registered_at: BlockNumberFor<T>,
        /// Clients skip inactive entries.
        pub active: bool,
        /// Consecutive settled days below `FAILING_SCORE`.
        pub failing_days: u8,
        /// Score of the last settled day, for clients and dashboards.
        pub last_score: u8,
    }

    #[derive(
        Encode,
        Decode,
        DecodeWithMemTracking,
        CloneNoBound,
        PartialEqNoBound,
        EqNoBound,
        DebugNoBound,
        TypeInfo,
        MaxEncodedLen,
        DefaultNoBound,
    )]
    #[scale_info(skip_type_params(T))]
    pub struct Tally<T: Config> {
        /// Stake of the reporters that reported on this endpoint.
        pub stake: u128,
        /// (score, stake) per report, for the weighted median.
        pub reports: BoundedVec<(u8, u128), T::MaxReporters>,
    }

    #[derive(
        Encode,
        Decode,
        DecodeWithMemTracking,
        Clone,
        Copy,
        PartialEq,
        Eq,
        Debug,
        TypeInfo,
        MaxEncodedLen,
    )]
    pub struct Params {
        /// Deposit held for each new entry.
        pub deposit: u128,
        /// Minimum day score for a payout and to become active again (percent).
        pub min_score: u8,
        /// Most one owner account can receive per day, in ppm of the day's pot.
        pub operator_cap_ppm: u32,
    }

    #[pallet::composite_enum]
    pub enum HoldReason {
        /// Deposit for an entry in the endpoint register.
        Endpoint,
    }

    #[pallet::pallet]
    pub struct Pallet<T>(_);

    #[pallet::config]
    pub trait Config: polkadot_sdk::frame_system::Config {
        #[allow(deprecated)]
        type RuntimeEvent: From<Event<Self>>
            + IsType<<Self as polkadot_sdk::frame_system::Config>::RuntimeEvent>;
        type WeightInfo: WeightInfo;
        type RuntimeHoldReason: From<HoldReason>;
        type Currency: Mutate<Self::AccountId, Balance = u128>
            + MutateHold<Self::AccountId, Balance = u128, Reason = Self::RuntimeHoldReason>;
        /// Stake of block producers and SN0 validators, whose probe reports count.
        type Reporters: ReporterStake<Self::AccountId>;
        /// Receives `EndpointShare` from `ict-emission`; this pallet pays from it.
        #[pallet::constant]
        type Pot: Get<Self::AccountId>;
        /// Development fund: receives whatever a day does not pay out.
        #[pallet::constant]
        type FundAccount: Get<Self::AccountId>;
        /// Sets the deposit, minimum score and operator cap (the Parameters track).
        type ParamsOrigin: EnsureOrigin<Self::RuntimeOrigin>;
        #[pallet::constant]
        type BlocksPerDay: Get<u32>;
        #[pallet::constant]
        type MaxUrlLen: Get<u32>;
        /// Entries per owner account.
        #[pallet::constant]
        type MaxPerOwner: Get<u32>;
        /// Entries in the register; bounds the daily settlement.
        #[pallet::constant]
        type MaxEndpoints: Get<u32>;
        /// Reports per endpoint per day.
        #[pallet::constant]
        type MaxReporters: Get<u32>;
        /// Days below 50 % in a row before an entry is marked inactive.
        #[pallet::constant]
        type InactiveAfterDays: Get<u8>;
    }

    #[pallet::type_value]
    pub fn DefaultParams() -> Params {
        Params {
            deposit: 100_000_000_000, // 100 ICT
            min_score: 90,
            operator_cap_ppm: 100_000, // 10 % of the day's pot
        }
    }

    #[pallet::storage]
    pub type CurrentParams<T> = StorageValue<_, Params, ValueQuery, DefaultParams>;

    #[pallet::storage]
    pub type NextId<T> = StorageValue<_, EndpointId, ValueQuery>;

    #[pallet::storage]
    pub type Count<T> = StorageValue<_, u32, ValueQuery>;

    #[pallet::storage]
    pub type Entries<T: Config> = StorageMap<_, Twox64Concat, EndpointId, Entry<T>>;

    /// One entry per URL.
    #[pallet::storage]
    pub type ByUrl<T: Config> =
        StorageMap<_, Blake2_128Concat, BoundedVec<u8, T::MaxUrlLen>, EndpointId>;

    #[pallet::storage]
    pub type OwnerCount<T: Config> = StorageMap<_, Blake2_128Concat, T::AccountId, u32, ValueQuery>;

    /// Reporters that have reported for a day.
    #[pallet::storage]
    pub type Reported<T: Config> =
        StorageDoubleMap<_, Twox64Concat, Day, Blake2_128Concat, T::AccountId, ()>;

    /// Reporter key of a principal (a block producer's stash or an SN0 hotkey): a key kept on
    /// the operator's server that reports on the principal's behalf, so the stash never has to.
    #[pallet::storage]
    pub type ReporterKey<T: Config> =
        StorageMap<_, Blake2_128Concat, T::AccountId, T::AccountId>;

    /// Principal a reporter key reports for.
    #[pallet::storage]
    pub type PrincipalOf<T: Config> =
        StorageMap<_, Blake2_128Concat, T::AccountId, T::AccountId>;

    #[pallet::storage]
    pub type Tallies<T: Config> =
        StorageDoubleMap<_, Twox64Concat, Day, Twox64Concat, EndpointId, Tally<T>, ValueQuery>;

    /// Last day settled, so a day is never paid twice.
    #[pallet::storage]
    pub type LastSettled<T> = StorageValue<_, Option<Day>, ValueQuery>;

    #[pallet::event]
    #[pallet::generate_deposit(pub(super) fn deposit_event)]
    pub enum Event<T: Config> {
        Registered {
            id: EndpointId,
            owner: T::AccountId,
            kind: Kind,
        },
        Updated {
            id: EndpointId,
        },
        Deregistered {
            id: EndpointId,
        },
        /// `reporter` is the principal (stash or SN0 hotkey) whose stake the report carries.
        Reported {
            day: Day,
            reporter: T::AccountId,
            endpoints: u32,
        },
        /// A principal set (or, with `None`, removed) the key that reports for it.
        ReporterSet {
            principal: T::AccountId,
            reporter: Option<T::AccountId>,
        },
        /// A day was settled: `paid` to endpoints, `to_fund` back to the development fund.
        Settled {
            day: Day,
            paid: u128,
            to_fund: u128,
        },
        Paid {
            day: Day,
            id: EndpointId,
            owner: T::AccountId,
            amount: u128,
            score: u8,
        },
        Inactive {
            id: EndpointId,
        },
        Active {
            id: EndpointId,
        },
        ParamsSet {
            params: Params,
        },
    }

    #[pallet::error]
    pub enum Error<T> {
        UrlTooLong,
        UrlTaken,
        /// The URL must start with wss://, https:// (RPC, archive) or /dns, /ip4, /ip6 (bootnode).
        BadUrl,
        TooManyForOwner,
        RegisterFull,
        UnknownEndpoint,
        NotOwner,
        /// Only block producers and SN0 validators report.
        NotReporter,
        /// Reports are for yesterday only.
        WrongDay,
        AlreadyReported,
        BadScore,
        TooManyReports,
        BadParams,
        /// The key already reports for another principal, or is itself a principal with a key.
        ReporterInUse,
    }

    #[pallet::hooks]
    impl<T: Config> Hooks<BlockNumberFor<T>> for Pallet<T> {
        fn on_initialize(n: BlockNumberFor<T>) -> Weight {
            let bpd = T::BlocksPerDay::get();
            let n: u32 = n.try_into().unwrap_or(u32::MAX);
            if bpd == 0 || !n.is_multiple_of(bpd) {
                return Weight::zero();
            }
            let today = n / bpd;
            if today < 2 {
                return Weight::zero();
            }
            let day = today - 2;
            if LastSettled::<T>::get().is_some_and(|d| d >= day) {
                return Weight::zero();
            }
            Self::settle(day);
            T::WeightInfo::settle(Count::<T>::get())
        }
    }

    #[pallet::call]
    impl<T: Config> Pallet<T> {
        /// Register an endpoint. The deposit is held until the entry is deregistered.
        #[pallet::call_index(0)]
        #[pallet::weight(T::WeightInfo::register())]
        pub fn register(
            origin: OriginFor<T>,
            kind: Kind,
            url: Vec<u8>,
            region: Vec<u8>,
        ) -> DispatchResult {
            let who = ensure_signed(origin)?;
            let url = Self::check_url(kind, url)?;
            let region: BoundedVec<u8, ConstU32<16>> =
                region.try_into().map_err(|_| Error::<T>::BadUrl)?;
            ensure!(!ByUrl::<T>::contains_key(&url), Error::<T>::UrlTaken);
            ensure!(
                OwnerCount::<T>::get(&who) < T::MaxPerOwner::get(),
                Error::<T>::TooManyForOwner
            );
            ensure!(
                Count::<T>::get() < T::MaxEndpoints::get(),
                Error::<T>::RegisterFull
            );
            let deposit = CurrentParams::<T>::get().deposit;
            T::Currency::hold(&HoldReason::Endpoint.into(), &who, deposit)?;
            let id = NextId::<T>::get();
            NextId::<T>::put(id.saturating_add(1));
            Count::<T>::mutate(|c| *c += 1);
            OwnerCount::<T>::mutate(&who, |c| *c += 1);
            ByUrl::<T>::insert(&url, id);
            Entries::<T>::insert(
                id,
                Entry {
                    owner: who.clone(),
                    kind,
                    url,
                    region,
                    deposit,
                    registered_at: polkadot_sdk::frame_system::Pallet::<T>::block_number(),
                    active: true,
                    failing_days: 0,
                    last_score: 0,
                },
            );
            Self::deposit_event(Event::Registered {
                id,
                owner: who,
                kind,
            });
            Ok(())
        }

        /// Change an entry's URL or region (owner only). Its score history stays.
        #[pallet::call_index(1)]
        #[pallet::weight(T::WeightInfo::update())]
        pub fn update(
            origin: OriginFor<T>,
            id: EndpointId,
            url: Vec<u8>,
            region: Vec<u8>,
        ) -> DispatchResult {
            let who = ensure_signed(origin)?;
            let mut e = Entries::<T>::get(id).ok_or(Error::<T>::UnknownEndpoint)?;
            ensure!(e.owner == who, Error::<T>::NotOwner);
            let url = Self::check_url(e.kind, url)?;
            if url != e.url {
                ensure!(!ByUrl::<T>::contains_key(&url), Error::<T>::UrlTaken);
                ByUrl::<T>::remove(&e.url);
                ByUrl::<T>::insert(&url, id);
                e.url = url;
            }
            e.region = region.try_into().map_err(|_| Error::<T>::BadUrl)?;
            Entries::<T>::insert(id, e);
            Self::deposit_event(Event::Updated { id });
            Ok(())
        }

        /// Remove an entry (owner only) and release its deposit.
        #[pallet::call_index(2)]
        #[pallet::weight(T::WeightInfo::deregister())]
        pub fn deregister(origin: OriginFor<T>, id: EndpointId) -> DispatchResult {
            let who = ensure_signed(origin)?;
            let e = Entries::<T>::get(id).ok_or(Error::<T>::UnknownEndpoint)?;
            ensure!(e.owner == who, Error::<T>::NotOwner);
            T::Currency::release(
                &HoldReason::Endpoint.into(),
                &who,
                e.deposit,
                Precision::BestEffort,
            )?;
            ByUrl::<T>::remove(&e.url);
            Entries::<T>::remove(id);
            Count::<T>::mutate(|c| *c = c.saturating_sub(1));
            OwnerCount::<T>::mutate(&who, |c| *c = c.saturating_sub(1));
            Self::deposit_event(Event::Deregistered { id });
            Ok(())
        }

        /// A block producer or SN0 validator reports yesterday's probe results: for each
        /// endpoint it probed, the share of probes that passed, in percent. One report per
        /// principal per day, sent by the principal itself or by its reporter key
        /// (`set_reporter`). A report that is accepted pays no fee, so a reporter key needs only
        /// a little ICT up front for the fee check.
        #[pallet::call_index(3)]
        #[pallet::weight(T::WeightInfo::report(scores.len() as u32))]
        pub fn report(
            origin: OriginFor<T>,
            day: Day,
            scores: Vec<(EndpointId, u8)>,
        ) -> DispatchResultWithPostInfo {
            let sender = ensure_signed(origin)?;
            let who = PrincipalOf::<T>::get(&sender).unwrap_or(sender);
            let stake = T::Reporters::stake_of(&who);
            ensure!(stake > 0, Error::<T>::NotReporter);
            ensure!(
                Some(day) == Self::today().checked_sub(1),
                Error::<T>::WrongDay
            );
            ensure!(
                !Reported::<T>::contains_key(day, &who),
                Error::<T>::AlreadyReported
            );
            ensure!(
                scores.len() as u32 <= T::MaxEndpoints::get(),
                Error::<T>::TooManyReports
            );
            let mut seen: Vec<EndpointId> = Vec::with_capacity(scores.len());
            for (id, score) in scores.iter() {
                ensure!(*score <= FULL_SCORE, Error::<T>::BadScore);
                ensure!(Entries::<T>::contains_key(id), Error::<T>::UnknownEndpoint);
                ensure!(!seen.contains(id), Error::<T>::BadScore);
                seen.push(*id);
                Tallies::<T>::try_mutate(day, id, |t| -> DispatchResult {
                    t.reports
                        .try_push((*score, stake))
                        .map_err(|_| Error::<T>::TooManyReports)?;
                    t.stake = t.stake.saturating_add(stake);
                    Ok(())
                })?;
            }
            Reported::<T>::insert(day, &who, ());
            Self::deposit_event(Event::Reported {
                day,
                reporter: who,
                endpoints: scores.len() as u32,
            });
            Ok(Pays::No.into())
        }

        /// Name the key that reports for the caller (a block producer's stash, signed in its own
        /// wallet, or an SN0 hotkey); `None` removes it. The key then reports with the caller's
        /// stake; the caller can still report itself. One key per principal, one principal per key.
        #[pallet::call_index(5)]
        #[pallet::weight(T::WeightInfo::set_reporter())]
        pub fn set_reporter(origin: OriginFor<T>, reporter: Option<T::AccountId>) -> DispatchResult {
            let who = ensure_signed(origin)?;
            // A reporter key cannot name a reporter of its own.
            ensure!(!PrincipalOf::<T>::contains_key(&who), Error::<T>::ReporterInUse);
            if let Some(r) = &reporter {
                ensure!(
                    r != &who
                        && PrincipalOf::<T>::get(r).is_none_or(|p| p == who)
                        && !ReporterKey::<T>::contains_key(r),
                    Error::<T>::ReporterInUse
                );
            }
            if let Some(old) = ReporterKey::<T>::take(&who) {
                PrincipalOf::<T>::remove(old);
            }
            if let Some(r) = &reporter {
                ReporterKey::<T>::insert(&who, r);
                PrincipalOf::<T>::insert(r, &who);
            }
            Self::deposit_event(Event::ReporterSet {
                principal: who,
                reporter,
            });
            Ok(())
        }

        /// Set the deposit for new entries, the minimum score and the operator cap.
        #[pallet::call_index(4)]
        #[pallet::weight(T::WeightInfo::set_params())]
        pub fn set_params(origin: OriginFor<T>, params: Params) -> DispatchResult {
            T::ParamsOrigin::ensure_origin(origin)?;
            ensure!(
                params.min_score <= FULL_SCORE
                    && params.operator_cap_ppm > 0
                    && params.operator_cap_ppm <= 1_000_000
                    && params.deposit > 0,
                Error::<T>::BadParams
            );
            CurrentParams::<T>::put(params);
            Self::deposit_event(Event::ParamsSet { params });
            Ok(())
        }
    }

    impl<T: Config> Pallet<T> {
        pub fn today() -> Day {
            let n: u32 = polkadot_sdk::frame_system::Pallet::<T>::block_number()
                .try_into()
                .unwrap_or(u32::MAX);
            n / T::BlocksPerDay::get().max(1)
        }

        fn check_url(
            kind: Kind,
            url: Vec<u8>,
        ) -> Result<BoundedVec<u8, T::MaxUrlLen>, DispatchError> {
            let ok = match kind {
                Kind::Rpc | Kind::Archive => {
                    url.starts_with(b"wss://") || url.starts_with(b"https://")
                }
                Kind::Bootnode => {
                    url.starts_with(b"/dns")
                        || url.starts_with(b"/ip4/")
                        || url.starts_with(b"/ip6/")
                }
            };
            ensure!(
                ok && url.iter().all(|c| c.is_ascii_graphic()),
                Error::<T>::BadUrl
            );
            url.try_into().map_err(|_| Error::<T>::UrlTooLong.into())
        }

        /// Stake-weighted median of the reports, or `None` when reporters with more than half
        /// of all reporter stake did not report on the endpoint.
        pub fn day_score(t: &Tally<T>, total_stake: u128) -> Option<u8> {
            if total_stake == 0 || t.stake.saturating_mul(2) <= total_stake {
                return None;
            }
            let mut r: Vec<(u8, u128)> = t.reports.to_vec();
            r.sort_by_key(|(s, _)| *s);
            let half = t.stake / 2;
            let mut acc: u128 = 0;
            for (score, stake) in r {
                acc = acc.saturating_add(stake);
                if acc > half {
                    return Some(score);
                }
            }
            None
        }

        /// Pay out one day from the pot and update each entry's status.
        pub fn settle(day: Day) {
            let params = CurrentParams::<T>::get();
            let total_stake = T::Reporters::total();
            let pot = T::Pot::get();
            let available =
                T::Currency::reducible_balance(&pot, Preservation::Expendable, Fortitude::Polite);

            // Score every entry; collect the paid ones.
            let mut scored: Vec<(EndpointId, T::AccountId, u128, u8)> = Vec::new(); // id, owner, weight×score, score
            let mut sum: u128 = 0;
            let ids: Vec<EndpointId> = Entries::<T>::iter_keys().collect();
            for id in ids {
                let Some(mut e) = Entries::<T>::get(id) else {
                    continue;
                };
                let tally = Tallies::<T>::take(day, id);
                let score = Self::day_score(&tally, total_stake);
                // Without a stake majority the day says nothing about this endpoint.
                let Some(score) = score else { continue };
                e.last_score = score;
                if score < FAILING_SCORE {
                    e.failing_days = e.failing_days.saturating_add(1);
                    if e.active && e.failing_days >= T::InactiveAfterDays::get() {
                        e.active = false;
                        Self::deposit_event(Event::Inactive { id });
                    }
                } else {
                    e.failing_days = 0;
                }
                if score >= params.min_score {
                    if !e.active {
                        e.active = true;
                        Self::deposit_event(Event::Active { id });
                    }
                    let w = e.kind.weight().saturating_mul(score as u128);
                    if w > 0 {
                        sum = sum.saturating_add(w);
                        scored.push((id, e.owner.clone(), w, score));
                    }
                }
                Entries::<T>::insert(id, e);
            }
            let _ = Tallies::<T>::clear_prefix(day, u32::MAX, None);
            let _ = Reported::<T>::clear_prefix(day, u32::MAX, None);

            // Pay, with a cap per owner account.
            let cap = available.saturating_mul(params.operator_cap_ppm as u128) / 1_000_000;
            let mut per_owner: Vec<(T::AccountId, u128)> = Vec::new();
            let mut paid: u128 = 0;
            if !sum.is_zero() {
                for (id, owner, w, score) in scored {
                    let mut amount = available.saturating_mul(w) / sum;
                    let already = per_owner
                        .iter()
                        .find(|(o, _)| *o == owner)
                        .map(|(_, a)| *a)
                        .unwrap_or(0);
                    amount = amount.min(cap.saturating_sub(already));
                    if amount.is_zero() {
                        continue;
                    }
                    if T::Currency::transfer(&pot, &owner, amount, Preservation::Expendable).is_ok()
                    {
                        paid = paid.saturating_add(amount);
                        match per_owner.iter_mut().find(|(o, _)| *o == owner) {
                            Some((_, a)) => *a = a.saturating_add(amount),
                            None => per_owner.push((owner.clone(), amount)),
                        }
                        Self::deposit_event(Event::Paid {
                            day,
                            id,
                            owner,
                            amount,
                            score,
                        });
                    }
                }
            }
            // The rest goes back to the fund: the share is a ceiling.
            let rest = available.saturating_sub(paid);
            let to_fund = if rest > 0
                && T::Currency::transfer(
                    &pot,
                    &T::FundAccount::get(),
                    rest,
                    Preservation::Expendable,
                )
                .is_ok()
            {
                rest
            } else {
                0
            };
            LastSettled::<T>::put(Some(day));
            Self::deposit_event(Event::Settled { day, paid, to_fund });
        }
    }
}
