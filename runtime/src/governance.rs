//! On-chain governance and the development fund (spec Section 10, whitepaper Sections 5.1 and 10).
//!
//! - The development fund is `pallet-treasury` on the fund account that `ict-emission` already
//!   pays 10 % of issuance into. Nothing leaves it without a vote; unspent funds are not burned.
//! - Bounties with child bounties pay programmes (marketing, exchange listing) through a curator.
//! - OpenGov: `referenda` + `conviction-voting`, three tracks (Root, Treasurer, Small spender).
//!   Turnout is measured against ICT that can vote: protocol accounts (launch vault, fund,
//!   subnet pot, stake and work accounts) are left out, so launch-period rewards cannot vote
//!   until they are released.
//! - `launch`: at the end of the launch period the runtime removes the sudo key and creates the
//!   two disclosed bounties with their curator. Nobody has to act.

use alloc::borrow::Cow;
use polkadot_sdk::*;

use frame_support::{
    parameter_types,
    traits::{
        fungible::Inspect, tokens::Fortitude, tokens::Preservation, EitherOf, EqualPrivilegeOnly,
        Get, LinearStoragePrice,
    },
    PalletId,
};
use frame_system::{EnsureRoot, EnsureRootWithSuccess, EnsureSigned};
use pallet_referenda::{Curve, Track, TrackInfo};
use sp_runtime::{str_array as s, traits::AccountIdConversion, Perbill, Permill};

use super::{
    AccountId, Balance, Balances, BlockNumber, Bounties, ChildBounties, OriginCaller, Preimage,
    Referenda, Runtime, RuntimeCall, RuntimeEvent, RuntimeHoldReason, RuntimeOrigin, Scheduler,
    System, Treasury, DAYS, HOURS, UNIT,
};
use crate::configs::{
    FundPalletId, LaunchPalletId, PotPalletId, RegistryPalletId, RuntimeBlockWeights,
    StakePalletId, WorkPalletId,
};

// ---------------------------------------------------------------- custom origins

/// Origins that the treasury tracks dispatch with; each carries a spending limit.
#[frame_support::pallet]
pub mod pallet_custom_origins {
    use polkadot_sdk::frame_support::pallet_prelude::*;

    #[pallet::config]
    pub trait Config: polkadot_sdk::frame_system::Config {}

    #[pallet::pallet]
    pub struct Pallet<T>(_);

    #[derive(
        PartialEq, Eq, Clone, MaxEncodedLen, Encode, Decode, DecodeWithMemTracking, TypeInfo, Debug,
    )]
    #[pallet::origin]
    pub enum Origin {
        /// Spends of any size from the fund (7-day vote).
        Treasurer,
        /// Spends of up to 1,000 ICT (3-day vote).
        SmallSpender,
    }
}

impl pallet_custom_origins::Config for Runtime {}

pub use pallet_custom_origins::Origin as CustomOrigin;

pub const MAX_BALANCE: Balance = Balance::MAX;
pub const SMALL_SPEND: Balance = 1_000 * UNIT;

/// Treasurer → unlimited, SmallSpender → 1,000 ICT.
pub struct Spender;
impl<O: Into<Result<CustomOrigin, O>> + From<CustomOrigin>> frame_support::traits::EnsureOrigin<O>
    for Spender
{
    type Success = Balance;
    fn try_origin(o: O) -> Result<Balance, O> {
        o.into().map(|o| match o {
            CustomOrigin::Treasurer => MAX_BALANCE,
            CustomOrigin::SmallSpender => SMALL_SPEND,
        })
    }
    #[cfg(feature = "runtime-benchmarks")]
    fn try_successful_origin() -> Result<O, ()> {
        Ok(O::from(CustomOrigin::Treasurer))
    }
}

parameter_types! {
    pub const MaxBalance: Balance = MAX_BALANCE;
}

/// Root (sudo in the launch period, the Root track after it) or a treasury track.
pub type TreasurySpender = EitherOf<EnsureRootWithSuccess<AccountId, MaxBalance>, Spender>;

// ---------------------------------------------------------------- scheduler, preimage, multisig

parameter_types! {
    pub MaximumSchedulerWeight: frame_support::weights::Weight =
        Perbill::from_percent(80) * RuntimeBlockWeights::get().max_block;
    pub const PreimageBaseDeposit: Balance = UNIT;
    pub const PreimageByteDeposit: Balance = UNIT / 1_000;
    pub const PreimageHoldReason: RuntimeHoldReason =
        RuntimeHoldReason::Preimage(pallet_preimage::HoldReason::Preimage);
    pub const MultisigDepositBase: Balance = UNIT;
    pub const MultisigDepositFactor: Balance = UNIT / 100;
}

impl pallet_scheduler::Config for Runtime {
    type RuntimeEvent = RuntimeEvent;
    type RuntimeOrigin = RuntimeOrigin;
    type PalletsOrigin = OriginCaller;
    type RuntimeCall = RuntimeCall;
    type MaximumWeight = MaximumSchedulerWeight;
    type ScheduleOrigin = EnsureRoot<AccountId>;
    type OriginPrivilegeCmp = EqualPrivilegeOnly;
    type MaxScheduledPerBlock = frame_support::traits::ConstU32<50>;
    type WeightInfo = pallet_scheduler::weights::SubstrateWeight<Runtime>;
    type Preimages = Preimage;
    type BlockNumberProvider = System;
}

impl pallet_preimage::Config for Runtime {
    type RuntimeEvent = RuntimeEvent;
    type WeightInfo = pallet_preimage::weights::SubstrateWeight<Runtime>;
    type Currency = Balances;
    type ManagerOrigin = EnsureRoot<AccountId>;
    type Consideration = frame_support::traits::fungible::HoldConsideration<
        AccountId,
        Balances,
        PreimageHoldReason,
        LinearStoragePrice<PreimageBaseDeposit, PreimageByteDeposit, Balance>,
    >;
}

impl pallet_multisig::Config for Runtime {
    type RuntimeEvent = RuntimeEvent;
    type RuntimeCall = RuntimeCall;
    type Currency = Balances;
    type DepositBase = MultisigDepositBase;
    type DepositFactor = MultisigDepositFactor;
    type MaxSignatories = frame_support::traits::ConstU32<20>;
    type WeightInfo = pallet_multisig::weights::SubstrateWeight<Runtime>;
    type BlockNumberProvider = System;
}

// ---------------------------------------------------------------- treasury and bounties

parameter_types! {
    /// Approved spends and bounties are paid once a day.
    pub const SpendPeriod: BlockNumber = DAYS;
    /// Unspent funds stay in the fund (whitepaper: no burn).
    pub const NoBurn: Permill = Permill::zero();
    pub const PayoutPeriod: BlockNumber = 30 * DAYS;
    pub FundAccountId: AccountId = FundPalletId::get().into_account_truncating();

    pub const BountyDepositBase: Balance = UNIT;
    /// An award can be claimed 3 days after it is made; governance can stop it meanwhile.
    pub const BountyDepositPayoutDelay: BlockNumber = 3 * DAYS;
    /// A curator must show activity at least every 90 days.
    pub const BountyUpdatePeriod: BlockNumber = 90 * DAYS;
    pub const CuratorDepositMultiplier: Permill = Permill::from_percent(50);
    pub CuratorDepositMin: Option<Balance> = Some(UNIT);
    pub CuratorDepositMax: Option<Balance> = Some(100 * UNIT);
    pub const BountyValueMinimum: Balance = 10 * UNIT;
    pub const DataDepositPerByte: Balance = UNIT / 100;
    pub const MaximumReasonLength: u32 = 1_024;
    pub const ChildBountyValueMinimum: Balance = UNIT;
}

impl pallet_treasury::Config for Runtime {
    type Currency = Balances;
    type RejectOrigin = EnsureRoot<AccountId>;
    type RuntimeEvent = RuntimeEvent;
    type SpendPeriod = SpendPeriod;
    type Burn = NoBurn;
    /// Same PalletId as the fund account `ict-emission` pays into.
    type PalletId = FundPalletId;
    type BurnDestination = ();
    type WeightInfo = pallet_treasury::weights::SubstrateWeight<Runtime>;
    type SpendFunds = Bounties;
    type MaxApprovals = frame_support::traits::ConstU32<100>;
    type SpendOrigin = TreasurySpender;
    type AssetKind = ();
    type Beneficiary = AccountId;
    type BeneficiaryLookup = sp_runtime::traits::IdentityLookup<AccountId>;
    type Paymaster = frame_support::traits::tokens::pay::PayFromAccount<Balances, FundAccountId>;
    type BalanceConverter = frame_support::traits::tokens::UnityAssetBalanceConversion;
    type PayoutPeriod = PayoutPeriod;
    #[cfg(feature = "runtime-benchmarks")]
    type BenchmarkHelper = ();
    type BlockNumberProvider = System;
}

impl pallet_bounties::Config for Runtime {
    type BountyDepositBase = BountyDepositBase;
    type BountyDepositPayoutDelay = BountyDepositPayoutDelay;
    type BountyUpdatePeriod = BountyUpdatePeriod;
    type CuratorDepositMultiplier = CuratorDepositMultiplier;
    type CuratorDepositMax = CuratorDepositMax;
    type CuratorDepositMin = CuratorDepositMin;
    type BountyValueMinimum = BountyValueMinimum;
    type DataDepositPerByte = DataDepositPerByte;
    type RuntimeEvent = RuntimeEvent;
    type MaximumReasonLength = MaximumReasonLength;
    type WeightInfo = pallet_bounties::weights::SubstrateWeight<Runtime>;
    type ChildBountyManager = ChildBounties;
    type OnSlash = Treasury;
    type TransferAllAssets = pallet_bounties::TransferFungible<AccountId, Balances>;
}

impl pallet_child_bounties::Config for Runtime {
    type MaxActiveChildBountyCount = frame_support::traits::ConstU32<100>;
    type ChildBountyValueMinimum = ChildBountyValueMinimum;
    type RuntimeEvent = RuntimeEvent;
    type WeightInfo = pallet_child_bounties::weights::SubstrateWeight<Runtime>;
}

// ---------------------------------------------------------------- OpenGov

/// ICT that can vote: total issuance minus what protocol accounts hold (launch vault, fund,
/// subnet pot, validator stake, pool bonds, subnet deposits).
pub struct VotableIssuance;
impl Get<Balance> for VotableIssuance {
    fn get() -> Balance {
        let held = |id: PalletId| -> Balance {
            let a: AccountId = id.into_account_truncating();
            <Balances as Inspect<AccountId>>::reducible_balance(
                &a,
                Preservation::Expendable,
                Fortitude::Force,
            )
        };
        let protocol = [
            LaunchPalletId::get(),
            FundPalletId::get(),
            PotPalletId::get(),
            StakePalletId::get(),
            WorkPalletId::get(),
            RegistryPalletId::get(),
        ]
        .into_iter()
        .map(held)
        .fold(0u128, |a, b| a.saturating_add(b));
        <Balances as Inspect<AccountId>>::total_issuance().saturating_sub(protocol)
    }
}

parameter_types! {
    pub const VoteLockingPeriod: BlockNumber = 7 * DAYS;
    pub const SubmissionDeposit: Balance = UNIT;
    pub const UndecidingTimeout: BlockNumber = 14 * DAYS;
}

impl pallet_conviction_voting::Config for Runtime {
    type RuntimeEvent = RuntimeEvent;
    type WeightInfo = pallet_conviction_voting::weights::SubstrateWeight<Runtime>;
    type Currency = Balances;
    type Polls = Referenda;
    type MaxTurnout = VotableIssuance;
    type MaxVotes = frame_support::traits::ConstU32<512>;
    type VoteLockingPeriod = VoteLockingPeriod;
    type BlockNumberProvider = System;
    type VotingHooks = ();
}

pub const TRACK_ROOT: u16 = 0;
pub const TRACK_TREASURER: u16 = 1;
pub const TRACK_SMALL_SPENDER: u16 = 2;

const fn pct(x: u32) -> Perbill {
    Perbill::from_percent(x)
}

/// Approval (share of conviction-weighted votes that are aye) and support (share of votable
/// ICT that voted aye) both fall linearly from `ceil` to `floor` over the decision period.
const fn linear(floor: u32, ceil: u32) -> Curve {
    Curve::LinearDecreasing {
        length: pct(100),
        floor: pct(floor),
        ceil: pct(ceil),
    }
}

const TRACKS: [Track<u16, Balance, BlockNumber>; 3] = [
    Track {
        id: TRACK_ROOT,
        info: TrackInfo {
            name: s("root"),
            max_deciding: 1,
            decision_deposit: 100 * UNIT,
            prepare_period: DAYS,
            decision_period: 7 * DAYS,
            confirm_period: DAYS,
            // Runtime upgrades take effect a week after approval at the earliest.
            min_enactment_period: 7 * DAYS,
            min_approval: linear(75, 100),
            min_support: linear(5, 50),
        },
    },
    Track {
        id: TRACK_TREASURER,
        info: TrackInfo {
            name: s("treasurer"),
            max_deciding: 10,
            decision_deposit: 10 * UNIT,
            prepare_period: DAYS,
            decision_period: 7 * DAYS,
            confirm_period: DAYS,
            min_enactment_period: DAYS,
            min_approval: linear(50, 100),
            min_support: linear(2, 25),
        },
    },
    Track {
        id: TRACK_SMALL_SPENDER,
        info: TrackInfo {
            name: s("small_spender"),
            max_deciding: 20,
            decision_deposit: UNIT,
            prepare_period: 4 * HOURS,
            decision_period: 3 * DAYS,
            confirm_period: 6 * HOURS,
            min_enactment_period: DAYS,
            min_approval: linear(50, 100),
            min_support: linear(1, 10),
        },
    },
];

pub struct TracksInfo;
impl pallet_referenda::TracksInfo<Balance, BlockNumber> for TracksInfo {
    type Id = u16;
    type RuntimeOrigin = <RuntimeOrigin as frame_support::traits::OriginTrait>::PalletsOrigin;

    fn tracks() -> impl Iterator<Item = Cow<'static, Track<Self::Id, Balance, BlockNumber>>> {
        TRACKS.iter().map(Cow::Borrowed)
    }

    fn track_for(id: &Self::RuntimeOrigin) -> Result<Self::Id, ()> {
        if let Ok(frame_system::RawOrigin::Root) = frame_system::RawOrigin::try_from(id.clone()) {
            return Ok(TRACK_ROOT);
        }
        match CustomOrigin::try_from(id.clone()) {
            Ok(CustomOrigin::Treasurer) => Ok(TRACK_TREASURER),
            Ok(CustomOrigin::SmallSpender) => Ok(TRACK_SMALL_SPENDER),
            Err(_) => Err(()),
        }
    }
}

impl pallet_referenda::Config for Runtime {
    type RuntimeCall = RuntimeCall;
    type RuntimeEvent = RuntimeEvent;
    type WeightInfo = pallet_referenda::weights::SubstrateWeight<Runtime>;
    type Scheduler = Scheduler;
    type Currency = Balances;
    type SubmitOrigin = frame_support::traits::AsEnsureOriginWithArg<EnsureSigned<AccountId>>;
    type CancelOrigin = EnsureRoot<AccountId>;
    type KillOrigin = EnsureRoot<AccountId>;
    type Slash = Treasury;
    type Votes = pallet_conviction_voting::VotesOf<Runtime>;
    type Tally = pallet_conviction_voting::TallyOf<Runtime>;
    type SubmissionDeposit = SubmissionDeposit;
    type MaxQueued = frame_support::traits::ConstU32<100>;
    type UndecidingTimeout = UndecidingTimeout;
    type AlarmInterval = frame_support::traits::ConstU32<1>;
    type Tracks = TracksInfo;
    type Preimages = Preimage;
    type BlockNumberProvider = System;
}

// ---------------------------------------------------------------- end of the launch period

/// At the end of the launch period: create the disclosed bounties with their curator and
/// remove the sudo key. Runs once, by itself, in the block where the period ends.
#[frame_support::pallet]
pub mod pallet_launch {
    use alloc::vec::Vec;
    use polkadot_sdk::frame_support::pallet_prelude::*;
    use polkadot_sdk::frame_system::{pallet_prelude::*, RawOrigin};
    use polkadot_sdk::sp_runtime::traits::{SaturatedConversion, StaticLookup};
    use polkadot_sdk::{pallet_bounties, pallet_sudo, pallet_treasury};

    type BalanceOf<T> = pallet_treasury::BalanceOf<T>;

    #[pallet::config]
    pub trait Config:
        polkadot_sdk::frame_system::Config
        + pallet_bounties::Config
        + pallet_sudo::Config
        + pallet_subnet_registry::Config
    {
        #[allow(deprecated)]
        type RuntimeEvent: From<Event<Self>>
            + IsType<<Self as polkadot_sdk::frame_system::Config>::RuntimeEvent>;
    }

    #[pallet::pallet]
    pub struct Pallet<T>(_);

    pub type Description = BoundedVec<u8, ConstU32<256>>;

    /// Curator and bounties to create at the end of the launch period. Set at genesis
    /// (disclosed) and changeable only by Root before it runs.
    #[pallet::storage]
    pub type Curator<T: Config> = StorageValue<_, T::AccountId, OptionQuery>;

    #[pallet::storage]
    pub type PlannedBounties<T: Config> =
        StorageValue<_, BoundedVec<(Description, BalanceOf<T>), ConstU32<4>>, ValueQuery>;

    /// Set once the end-of-launch steps have run.
    #[pallet::storage]
    pub type Done<T> = StorageValue<_, bool, ValueQuery>;

    #[pallet::genesis_config]
    #[derive(polkadot_sdk::frame_support::DefaultNoBound)]
    pub struct GenesisConfig<T: Config> {
        pub curator: Option<T::AccountId>,
        /// (description, amount in base units)
        pub bounties: Vec<(Vec<u8>, BalanceOf<T>)>,
    }

    #[pallet::genesis_build]
    impl<T: Config> BuildGenesisConfig for GenesisConfig<T> {
        fn build(&self) {
            if let Some(c) = &self.curator {
                Curator::<T>::put(c.clone());
            }
            let b: Vec<(Description, BalanceOf<T>)> = self
                .bounties
                .iter()
                .map(|(d, v)| (Description::truncate_from(d.clone()), *v))
                .collect();
            PlannedBounties::<T>::put(BoundedVec::truncate_from(b));
        }
    }

    #[pallet::event]
    #[pallet::generate_deposit(pub(super) fn deposit_event)]
    pub enum Event<T: Config> {
        /// The launch period ended: bounties created, sudo removed.
        LaunchEnded {
            bounties: u32,
            sudo_removed: bool,
        },
        /// A planned bounty could not be created (the reason is in the error).
        BountyFailed {
            index: u32,
            error: DispatchError,
        },
        PlanChanged,
    }

    #[pallet::error]
    pub enum Error<T> {
        AlreadyDone,
    }

    #[pallet::hooks]
    impl<T: Config> Hooks<BlockNumberFor<T>> for Pallet<T> {
        fn on_initialize(n: BlockNumberFor<T>) -> Weight {
            let until = pallet_subnet_registry::BootstrapUntil::<T>::get();
            let n: u32 = n.saturated_into();
            if until == 0 || n < until || Done::<T>::get() {
                return T::DbWeight::get().reads(2);
            }
            Self::end_launch();
            // A handful of bounty calls and one sudo call; well within a block.
            Weight::from_parts(500_000_000, 100_000)
        }
    }

    #[pallet::call]
    impl<T: Config> Pallet<T> {
        /// Change the curator or the planned bounties before the launch period ends (Root).
        #[pallet::call_index(0)]
        #[pallet::weight(T::DbWeight::get().writes(2))]
        pub fn set_plan(
            origin: OriginFor<T>,
            curator: T::AccountId,
            bounties: Vec<(Vec<u8>, BalanceOf<T>)>,
        ) -> DispatchResult {
            ensure_root(origin)?;
            ensure!(!Done::<T>::get(), Error::<T>::AlreadyDone);
            Curator::<T>::put(curator);
            let b: Vec<(Description, BalanceOf<T>)> = bounties
                .into_iter()
                .map(|(d, v)| (Description::truncate_from(d), v))
                .collect();
            PlannedBounties::<T>::put(BoundedVec::truncate_from(b));
            Self::deposit_event(Event::PlanChanged);
            Ok(())
        }
    }

    impl<T: Config> Pallet<T> {
        pub fn end_launch() {
            Done::<T>::put(true);
            let mut created = 0u32;
            if let Some(curator) = Curator::<T>::get() {
                let fund = pallet_treasury::Pallet::<T>::account_id();
                for (i, (desc, value)) in PlannedBounties::<T>::get().into_iter().enumerate() {
                    let id = pallet_bounties::BountyCount::<T>::get();
                    let r = pallet_bounties::Pallet::<T>::propose_bounty(
                        RawOrigin::Signed(fund.clone()).into(),
                        value,
                        desc.into_inner(),
                    )
                    .and_then(|_| {
                        pallet_bounties::Pallet::<T>::approve_bounty_with_curator(
                            RawOrigin::Root.into(),
                            id,
                            <T as polkadot_sdk::frame_system::Config>::Lookup::unlookup(
                                curator.clone(),
                            ),
                            Zero::zero(),
                        )
                    });
                    match r {
                        Ok(()) => created += 1,
                        Err(error) => Self::deposit_event(Event::BountyFailed {
                            index: i as u32,
                            error,
                        }),
                    }
                }
            }
            let mut sudo_removed = false;
            if let Some(key) = pallet_sudo::Key::<T>::get() {
                sudo_removed =
                    pallet_sudo::Pallet::<T>::remove_key(RawOrigin::Signed(key).into()).is_ok();
            }
            Self::deposit_event(Event::LaunchEnded {
                bounties: created,
                sudo_removed,
            });
        }
    }
}

impl pallet_launch::Config for Runtime {
    type RuntimeEvent = RuntimeEvent;
}
