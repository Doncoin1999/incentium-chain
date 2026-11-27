//! # Subnet registry pallet (spec Section 4, subnet registration specification of 25 Nov 2026)
//!
//! - SN0 exists from genesis and has no owner.
//! - Subnet registration unlocks by itself, in the first daily check where all five criteria
//!   hold: day 180, at least 300 active SN0 miners as a 30-day average, sudo removed with at
//!   least 3 approved referenda, the Base swap contract attested by governance as audited
//!   and immutable, and the Alpha module installed. Nobody switches it on.
//! - Slots are sold by Dutch auction (see `ict_economics::registry`): SN1 starts at
//!   50,000 ICT, later slots at 4 x the previous sale price, falling 25 % a day to a floor.
//!   The buyer publishes name, description and the hash of the incentive code in the same call.
//! - The payment is not refunded: half is burned; the other half goes to the subnet's Alpha
//!   pool once the Alpha module is installed (to the treasury before that).
//! - One slot at a time: the next auction starts `MinSpacing` (60 days) after a sale, and only
//!   while fewer than `MaxSubnets` (8) subnets exist. A new subnet cannot be deregistered for
//!   180 days.
//! - Each subnet (SN1 and later) is controlled by one key, then optionally a multisig, then
//!   optionally its stakers as a DAO; control only moves forward. The controller sets the
//!   subnet's settings (neuron fee, slots, penalties) within the protocol's bounds and its
//!   metadata. In DAO mode the stakers vote (5 days, weighed by stake at close) and the
//!   decision is carried out at once; the owner share then goes to the subnet's DAO account.
//! - Subnets may turn on penalties: validators holding half of the active stake can take a
//!   miner's incentive for an epoch; repeated penalties ban the hotkey from the subnet.
//! - Neurons (miners and validators) register with a burned fee.
//! - SN0 runs an open model list (spec Section 5, decided 27 Nov 2026; replaces the fixed
//!   tiers). Anyone proposes a model with a deposit; it opens when at least three validators
//!   have declared on chain that they hold it and governance (Parameters track) has approved
//!   its value factor G and tolerance. Each miner declares the open model it runs; validators
//!   check it against that model. During the launch period only Qwen3.5-2B is open.
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

/// What the runtime tells the registry about governance, for the unlock criteria.
pub trait UnlockSignals {
    /// True once the sudo key is gone for good.
    fn sudo_removed() -> bool;
    /// Number of approved referenda, counted up to `at_least` (counting may stop there).
    fn approved_referenda(at_least: u32) -> u32;
    /// True once the Alpha module (subnet pools) is part of the runtime.
    fn alpha_installed() -> bool;
}

impl UnlockSignals for () {
    fn sudo_removed() -> bool {
        false
    }
    fn approved_referenda(_at_least: u32) -> u32 {
        0
    }
    fn alpha_installed() -> bool {
        false
    }
}

/// Where the pool half of a slot payment goes (implemented by the Alpha module).
pub trait SubnetSale<AccountId> {
    /// True if sold subnets get an Alpha pool; otherwise the half goes to the treasury.
    fn pools_installed() -> bool;
    /// Register the pool of `netuid`, moving `amount` ICT from `buyer` into it.
    fn subnet_sold(netuid: u16, buyer: &AccountId, amount: u128) -> polkadot_sdk::sp_runtime::DispatchResult;
}

impl<AccountId> SubnetSale<AccountId> for () {
    fn pools_installed() -> bool {
        false
    }
    fn subnet_sold(_netuid: u16, _buyer: &AccountId, _amount: u128) -> polkadot_sdk::sp_runtime::DispatchResult {
        Ok(())
    }
}

/// Stake on SN1 and later subnets (implemented by the consensus pallet), for subnet DAO votes
/// and failed-work reports.
pub trait SubnetStakeInfo<AccountId> {
    fn stake_of(netuid: u16, who: &AccountId) -> u128;
    fn total(netuid: u16) -> u128;
    /// Stake of the validators whose weights counted in the subnet's last settled epoch
    /// (falls back to the total before any epoch is settled).
    fn active(netuid: u16) -> u128;
    /// Stake behind a validator hotkey, for failed-work reports. With ICT staking it is the
    /// same as `stake_of`; with Alpha it is all Alpha staked on the hotkey, while `stake_of`
    /// is the account's own Alpha, for votes.
    fn validator_stake(netuid: u16, who: &AccountId) -> u128 {
        Self::stake_of(netuid, who)
    }
    /// Benchmarks only: give `who` SN0 stake (for model declarations).
    #[cfg(feature = "runtime-benchmarks")]
    fn set_sn0_stake(_who: &AccountId, _amount: u128) {}
}

impl<AccountId> SubnetStakeInfo<AccountId> for () {
    fn stake_of(_netuid: u16, _who: &AccountId) -> u128 {
        0
    }
    fn total(_netuid: u16) -> u128 {
        0
    }
    fn active(_netuid: u16) -> u128 {
        0
    }
}

/// Told when a subnet is closed, so stake on it can be released.
pub trait OnSubnetRemoved {
    fn subnet_removed(netuid: u16);
}

impl OnSubnetRemoved for () {
    fn subnet_removed(_netuid: u16) {}
}

#[polkadot_sdk::frame_support::pallet]
pub mod pallet {
    use crate::{OnSubnetRemoved, SubnetSale, SubnetStakeInfo, UnlockSignals, WeightInfo};
    use ict_economics::registry::{auction_price, floor_price, payment_split, start_price};
    use ict_economics::BLOCKS_PER_DAY;
    use polkadot_sdk::frame_support::{
        pallet_prelude::*,
        traits::{
            fungible::Mutate,
            tokens::{Fortitude, Precision, Preservation},
        },
        PalletId,
    };
    use polkadot_sdk::frame_system::pallet_prelude::*;
    use polkadot_sdk::sp_runtime::traits::{
        AccountIdConversion, SaturatedConversion, TrailingZeroInput,
    };
    extern crate alloc;
    use alloc::vec::Vec;

    pub type NetUid = u16;
    pub type Uid = u16;
    pub type ModelId = u8;
    pub const SN0: NetUid = 0;
    /// Upper bound on entries in the model list, and on models one validator declares.
    pub const MAX_MODELS: u32 = 32;
    /// A model opens only when at least this many validators declare that they hold it.
    pub const MIN_MODEL_HOLDERS: u32 = 3;

    const DAY: u32 = BLOCKS_PER_DAY as u32;
    /// Unlock criterion 1: day 180 after genesis.
    pub const UNLOCK_MIN_BLOCK: u32 = 180 * DAY;
    /// Unlock criterion 2: at least 300 active SN0 miners as a 30-day average.
    pub const UNLOCK_MIN_MINERS: u32 = 300;
    pub const MINER_WINDOW_DAYS: u32 = 30;
    /// Unlock criterion 3: sudo removed and at least 3 approved referenda.
    pub const UNLOCK_MIN_REFERENDA: u32 = 3;
    /// A new subnet cannot be deregistered for 180 days.
    pub const IMMUNITY_BLOCKS: u32 = 180 * DAY;
    /// Default gap between a sale and the next auction (60 days); governance may lower it
    /// to `MIN_SPACING_FLOOR` (30 days).
    pub const DEFAULT_MIN_SPACING: u32 = 60 * DAY;
    pub const MIN_SPACING_FLOOR: u32 = 30 * DAY;
    /// Subnets (excluding SN0) allowed until governance raises the cap.
    pub const DEFAULT_MAX_SUBNETS: u16 = 8;
    /// Hard upper bound for the cap.
    pub const MAX_SUBNETS_LIMIT: u16 = 256;
    /// A subnet past its immunity is dead, and anyone may remove it, if no epoch in the last
    /// 30 days paid at least 3 distinct miners.
    pub const DEAD_AFTER_BLOCKS: u32 = 30 * DAY;
    pub const DEAD_MIN_MINERS: u32 = 3;
    /// The cap doubles by itself (8 -> 16 -> ... -> 256) when every slot has been taken for
    /// 90 days, at least 75 % of the subnets paid at least 10 miners on average over 30 days,
    /// 180 days have passed since the last raise, and not before day 775 (1 January 2029 on
    /// mainnet, whose genesis was 18 November 2026).
    pub const CAP_FULL_BLOCKS: u32 = 90 * DAY;
    pub const CAP_LIVE_PPM: u32 = 750_000;
    pub const CAP_LIVE_MINERS: u32 = 10;
    pub const CAP_RAISE_SPACING: u32 = 180 * DAY;
    pub const CAP_LADDER_FROM: u32 = 775 * DAY;

    const STORAGE_VERSION: StorageVersion = StorageVersion::new(3);

    /// Where a model stands in the list (spec Section 5).
    #[derive(
        Clone,
        Copy,
        Encode,
        Decode,
        DecodeWithMemTracking,
        PartialEq,
        Eq,
        Debug,
        TypeInfo,
        MaxEncodedLen,
    )]
    pub enum ModelStatus {
        /// Listed, not yet approved: miners cannot declare it.
        Proposed,
        /// Miners may declare it and are paid for it.
        Open,
        /// Being replaced: miners that declared it are still paid, no new declarations.
        Retiring,
        /// Not paid and not declarable.
        Closed,
    }

    /// One entry in the open model list (spec Section 5).
    #[derive(
        Clone, Encode, Decode, DecodeWithMemTracking, PartialEq, Eq, Debug, TypeInfo, MaxEncodedLen,
    )]
    pub struct ModelInfo<AccountId> {
        /// Model name, e.g. `Qwen3.5-2B`.
        pub name: BoundedVec<u8, ConstU32<64>>,
        /// SHA-256 of the weights file; all zero only for entries listed before hashes were
        /// pinned. A model cannot open without one.
        pub weights_hash: [u8; 32],
        /// Inference engine and version, e.g. `llama.cpp b6000`.
        pub engine: BoundedVec<u8, ConstU32<64>>,
        /// Memory the model needs with its context (two slots of 4,096 tokens), in MB.
        pub mem_mb: u32,
        /// Value factor G in ppm (1.0 = 1,000,000).
        pub g_ppm: u32,
        /// Maximum mean top-k log-probability distance in a spot check, in ppm.
        pub tolerance_ppm: u32,
        pub status: ModelStatus,
        /// Who proposed it (`None` for the starting list).
        pub proposer: Option<AccountId>,
        /// Deposit held from the proposer until the model opens or is withdrawn.
        pub deposit: u128,
    }

    /// The starting list (spec Section 5, G to be calibrated on testnet). Ids 0-3 are the
    /// former tiers, so declarations made under the tier table keep their meaning.
    pub fn default_models<AccountId>() -> [(ModelId, ModelInfo<AccountId>); 6] {
        let m = |name: &[u8], mem_mb: u32, g: u32, tol: u32, status| ModelInfo {
            name: BoundedVec::truncate_from(name.to_vec()),
            weights_hash: [0; 32],
            engine: BoundedVec::truncate_from(b"llama.cpp".to_vec()),
            mem_mb,
            g_ppm: g,
            tolerance_ppm: tol,
            status,
            proposer: None,
            deposit: 0,
        };
        use ModelStatus::*;
        [
            (0, m(b"Qwen3.5-2B", 1_800, 100_000, 350_000, Open)),
            (1, m(b"Qwen3.5-9B", 6_700, 1_000_000, 350_000, Proposed)),
            (2, m(b"Qwen3.5-27B", 18_000, 3_000_000, 250_000, Proposed)),
            (3, m(b"Qwen3.5-122B", 72_000, 5_000_000, 400_000, Closed)),
            (4, m(b"Qwen3.5-0.8B", 1_000, 50_000, 350_000, Proposed)),
            (5, m(b"Qwen3.5-4B", 3_300, 200_000, 350_000, Proposed)),
        ]
    }

    /// Tier table of storage version 2, read once by the v2 -> v3 migration.
    #[derive(Clone, Encode, Decode, PartialEq, Eq, Debug, TypeInfo, MaxEncodedLen)]
    pub struct LegacyTierInfo {
        pub weight_ppm: u32,
        pub tolerance_ppm: u32,
        pub model: BoundedVec<u8, ConstU32<64>>,
        pub model_hash: [u8; 32],
        pub open: bool,
    }

    #[derive(
        Clone, Encode, Decode, DecodeWithMemTracking, PartialEq, Eq, Debug, TypeInfo, MaxEncodedLen,
    )]
    pub struct SubnetInfo<AccountId, BlockNumber> {
        /// `None` for SN0.
        pub owner: Option<AccountId>,
        /// ICT paid for the slot, in base units (not refundable).
        pub deposit: u128,
        pub created: BlockNumber,
    }

    /// What the buyer of a slot publishes on chain. No approval, only openness.
    #[derive(
        Clone, Encode, Decode, DecodeWithMemTracking, PartialEq, Eq, Debug, TypeInfo, MaxEncodedLen,
    )]
    pub struct SubnetMetadata {
        pub name: BoundedVec<u8, ConstU32<64>>,
        pub description: BoundedVec<u8, ConstU32<512>>,
        /// Hash of the incentive-mechanism code (e.g. SHA-256 of a release archive or a git
        /// commit id padded to 32 bytes).
        pub code_hash: [u8; 32],
    }

    /// Governance's attestation that the Base swap contract is audited and immutable.
    #[derive(
        Clone, Encode, Decode, DecodeWithMemTracking, PartialEq, Eq, Debug, TypeInfo, MaxEncodedLen,
    )]
    pub struct SwapAttestation {
        /// Contract address on Base.
        pub contract: [u8; 20],
        /// Hash of the audit report.
        pub audit_hash: [u8; 32],
    }

    /// Where each unlock criterion stands (read by incentium.org's progress tab).
    #[derive(Clone, Encode, Decode, PartialEq, Eq, Debug, TypeInfo)]
    pub struct UnlockStatus {
        pub unlocked: bool,
        pub block: u32,
        pub min_block: u32,
        /// 30-day average of active SN0 miners; `None` until 30 full days are recorded.
        pub miners_avg: Option<u32>,
        pub miner_days: u32,
        pub min_miners: u32,
        pub sudo_removed: bool,
        pub approved_referenda: u32,
        pub min_referenda: u32,
        pub swap_attested: bool,
        /// The Alpha module is installed (fifth criterion, decided 26 Nov 2026).
        pub alpha_installed: bool,
    }

    // ------------------------------------------------------------ subnet control

    /// How a subnet (SN1 and later) is controlled. It can only move forward:
    /// `Key` -> `Multisig` -> `Dao`, or `Key` -> `Dao`.
    #[derive(
        Clone,
        Copy,
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
    pub enum ControlKind {
        /// One key: the owner account signs.
        #[default]
        Key,
        /// The owner account is a multisig of the signatories in `MultisigOf`.
        Multisig,
        /// The subnet's stakers decide by vote; the owner share goes to the subnet's DAO account.
        Dao,
    }

    /// Settings a subnet's controller may change, within the protocol's bounds.
    #[derive(
        Clone,
        Copy,
        Encode,
        Decode,
        DecodeWithMemTracking,
        PartialEq,
        Eq,
        Debug,
        TypeInfo,
        MaxEncodedLen,
    )]
    pub struct SubnetParams {
        /// Burned fee to register a neuron in this subnet.
        pub neuron_fee: u128,
        /// Neuron slots in this subnet.
        pub max_neurons: u16,
        /// Whether the subnet's validators can penalise miners for failed work.
        pub penalties: bool,
        /// Penalties inside `failure_window` epochs that ban a hotkey from the subnet.
        pub max_failures: u8,
        pub failure_window: u32,
    }

    /// What a subnet DAO can decide.
    #[derive(
        Clone, Encode, Decode, DecodeWithMemTracking, PartialEq, Eq, Debug, TypeInfo, MaxEncodedLen,
    )]
    pub enum SubnetAction<AccountId> {
        SetParams(SubnetParams),
        SetMetadata(SubnetMetadata),
        /// Pay from the subnet's DAO account.
        Spend {
            to: AccountId,
            amount: u128,
        },
    }

    #[derive(
        Clone, Encode, Decode, DecodeWithMemTracking, PartialEq, Eq, Debug, TypeInfo, MaxEncodedLen,
    )]
    pub struct Proposal<AccountId> {
        pub action: SubnetAction<AccountId>,
        pub proposer: AccountId,
        /// Block from which the vote can be closed.
        pub end: u32,
    }

    /// Bounds the protocol puts on subnet settings.
    pub const MAX_NEURON_FEE: u128 = 1_000 * ict_economics::UNIT;
    pub const MIN_NEURON_SLOTS: u16 = 16;
    pub const MAX_FAILURES_LIMIT: u8 = 10;
    /// Failure window between 1 and 90 days, in epochs of the runtime (20 per day).
    pub const MIN_FAILURE_WINDOW: u32 = 20;
    pub const MAX_FAILURE_WINDOW: u32 = 1_800;
    pub const DEFAULT_FAILURE_WINDOW: u32 = 600;
    pub const MAX_SIGNATORIES: u32 = 20;
    /// Subnet DAO votes run 5 days (shorter than the 7-day unbonding, so stake cannot vote,
    /// leave and vote again elsewhere in the same subnet).
    pub const DAO_VOTE_BLOCKS: u32 = 5 * DAY;
    /// A subnet DAO decision needs more aye than nay stake, and aye stake of at least 1/3 of
    /// the subnet's stake.
    pub const SUBNET_QUORUM_PPM: u128 = 333_334;
    /// A failed-work report takes a miner's incentive for the epoch once reporters hold at
    /// least half of the stake active in the subnet's last settled epoch (decided 25 Nov
    /// 2026). Slashing a bond, when bonds exist, will need 2/3.
    pub const PENALTY_QUORUM_PPM: u128 = 500_000;
    pub const MAX_REPORTERS: u32 = 64;

    /// The slot on sale, for incentium.org's subnet page.
    #[derive(Clone, Encode, Decode, PartialEq, Eq, Debug, TypeInfo)]
    pub struct AuctionInfo {
        pub now: u32,
        /// Current price; `None` when no auction runs.
        pub price: Option<u128>,
        /// Block the current (or next) auction starts.
        pub start_block: Option<u32>,
        pub start_price: u128,
        pub floor: u128,
        /// Number of the next sale (1 = SN1's slot).
        pub next_sale: u32,
        pub subnet_count: u16,
        pub max_subnets: u16,
        pub min_spacing: u32,
        pub full_since: Option<u32>,
        pub last_cap_raise: Option<u32>,
    }

    /// One subnet, for incentium.org's subnet page.
    #[derive(Clone, Encode, Decode, PartialEq, Eq, Debug, TypeInfo)]
    pub struct SubnetSummary<AccountId> {
        pub netuid: NetUid,
        pub owner: Option<AccountId>,
        pub control: ControlKind,
        pub created: u32,
        pub price_paid: u128,
        pub name: Vec<u8>,
        pub description: Vec<u8>,
        pub code_hash: [u8; 32],
        pub neurons: u16,
        pub stake: u128,
        /// Share of all issuance this subnet gets now, in ppm (filled in by the runtime).
        pub emission_ppm: u32,
        pub immune_until: u32,
        pub miners_avg: Option<u32>,
        pub dead: bool,
    }

    #[pallet::pallet]
    #[pallet::storage_version(STORAGE_VERSION)]
    pub struct Pallet<T>(_);

    #[pallet::config]
    pub trait Config: polkadot_sdk::frame_system::Config {
        #[allow(deprecated)]
        type RuntimeEvent: From<Event<Self>>
            + IsType<<Self as polkadot_sdk::frame_system::Config>::RuntimeEvent>;
        /// Weights of this pallet's calls (benchmarked in the runtime).
        type WeightInfo: WeightInfo;
        type Currency: Mutate<Self::AccountId, Balance = u128>;
        /// The registry's own account (kept for neuron bookkeeping and benchmarks).
        #[pallet::constant]
        type PalletId: Get<PalletId>;
        /// On-chain treasury that receives half of every slot payment.
        #[pallet::constant]
        type TreasuryAccount: Get<Self::AccountId>;
        /// Governance facts for the unlock criteria.
        type Unlock: UnlockSignals;
        /// Called when a subnet is closed (consensus releases its stake).
        type OnRemoved: OnSubnetRemoved;
        /// Stake per subnet (consensus), for subnet DAO votes and failed-work reports.
        type StakeInfo: SubnetStakeInfo<Self::AccountId>;
        /// The Alpha module, which receives the pool half of each slot payment.
        type Sale: SubnetSale<Self::AccountId>;
        /// Burned fee to register a neuron.
        #[pallet::constant]
        type NeuronFee: Get<u128>;
        /// Maximum neurons per subnet.
        #[pallet::constant]
        type MaxNeurons: Get<u16>;
        /// Deposit to propose a model, returned when it opens or is withdrawn, burned if
        /// governance closes it without opening it.
        #[pallet::constant]
        type ModelDeposit: Get<u128>;
        /// SN0 stake a validator needs for its model declarations to count.
        #[pallet::constant]
        type ModelDeclarerStake: Get<u128>;
    }

    #[pallet::storage]
    pub type Subnets<T: Config> = StorageMap<
        _,
        Twox64Concat,
        NetUid,
        SubnetInfo<T::AccountId, BlockNumberFor<T>>,
        OptionQuery,
    >;

    #[pallet::storage]
    pub type Metadata<T> = StorageMap<_, Twox64Concat, NetUid, SubnetMetadata, OptionQuery>;

    /// Subnets excluding SN0.
    #[pallet::storage]
    pub type SubnetCount<T> = StorageValue<_, u16, ValueQuery>;

    #[pallet::type_value]
    pub fn DefaultMaxSubnets() -> u16 {
        DEFAULT_MAX_SUBNETS
    }

    /// Cap on subnets (excluding SN0); only governance can raise it.
    #[pallet::storage]
    pub type MaxSubnets<T> = StorageValue<_, u16, ValueQuery, DefaultMaxSubnets>;

    #[pallet::type_value]
    pub fn DefaultMinSpacing() -> u32 {
        DEFAULT_MIN_SPACING
    }

    /// Blocks from a sale to the start of the next auction.
    #[pallet::storage]
    pub type MinSpacing<T> = StorageValue<_, u32, ValueQuery, DefaultMinSpacing>;

    /// True once subnet registration has unlocked. It never locks again.
    #[pallet::storage]
    pub type Phase2Enabled<T> = StorageValue<_, bool, ValueQuery>;

    /// Block where subnet registration unlocked (start of the 180-day emission ramp).
    #[pallet::storage]
    pub type UnlockedAt<T> = StorageValue<_, u32, OptionQuery>;

    /// Block where the current (or next) auction starts.
    #[pallet::storage]
    pub type AuctionStart<T> = StorageValue<_, u32, OptionQuery>;

    /// Slots sold since genesis (deregistrations do not lower it).
    #[pallet::storage]
    pub type Sales<T> = StorageValue<_, u32, ValueQuery>;

    /// Price the last slot sold for.
    #[pallet::storage]
    pub type LastSalePrice<T> = StorageValue<_, u128, ValueQuery>;

    #[pallet::storage]
    pub type SwapAttested<T> = StorageValue<_, SwapAttestation, OptionQuery>;

    /// Running tally for the current day: (day, sum of active miners over epochs, epochs).
    #[pallet::storage]
    pub type MinerDay<T> = StorageValue<_, (u32, u64, u32), ValueQuery>;

    /// Daily averages of active SN0 miners, oldest first, at most 30.
    #[pallet::storage]
    pub type MinerHistory<T> =
        StorageValue<_, BoundedVec<u32, ConstU32<MINER_WINDOW_DAYS>>, ValueQuery>;

    /// Per subnet (SN1 and later): running tally for the current day of paid miners.
    #[pallet::storage]
    pub type SubnetMinerDay<T> = StorageMap<_, Twox64Concat, NetUid, (u32, u64, u32), ValueQuery>;

    /// Per subnet: daily averages of paid miners, oldest first, at most 30.
    #[pallet::storage]
    pub type SubnetMinerHistory<T> = StorageMap<
        _,
        Twox64Concat,
        NetUid,
        BoundedVec<u32, ConstU32<MINER_WINDOW_DAYS>>,
        ValueQuery,
    >;

    /// Per subnet: last block whose epoch settlement paid at least `DEAD_MIN_MINERS` miners.
    #[pallet::storage]
    pub type LastHealthy<T> = StorageMap<_, Twox64Concat, NetUid, u32, OptionQuery>;

    /// Block since which every slot has been taken (`None` while one is free).
    #[pallet::storage]
    pub type FullSince<T> = StorageValue<_, u32, OptionQuery>;

    /// Block of the last raise of the cap (by the ladder or by governance).
    #[pallet::storage]
    pub type LastCapRaise<T> = StorageValue<_, u32, OptionQuery>;

    #[pallet::storage]
    pub type Control<T> = StorageMap<_, Twox64Concat, NetUid, ControlKind, ValueQuery>;

    /// Multisig subnets: (threshold, sorted signatories).
    #[pallet::storage]
    pub type MultisigOf<T: Config> = StorageMap<
        _,
        Twox64Concat,
        NetUid,
        (u16, BoundedVec<T::AccountId, ConstU32<MAX_SIGNATORIES>>),
        OptionQuery,
    >;

    /// Settings of a subnet; the protocol default applies when unset.
    #[pallet::storage]
    pub type Params<T> = StorageMap<_, Twox64Concat, NetUid, SubnetParams, OptionQuery>;

    /// The open subnet DAO proposal (one at a time per subnet).
    #[pallet::storage]
    pub type Proposals<T: Config> =
        StorageMap<_, Twox64Concat, NetUid, Proposal<T::AccountId>, OptionQuery>;

    /// Votes on the open proposal: (netuid, voter) -> aye. Weighed by stake when closed.
    #[pallet::storage]
    pub type Votes<T: Config> = StorageDoubleMap<
        _,
        Twox64Concat,
        NetUid,
        Blake2_128Concat,
        T::AccountId,
        bool,
        OptionQuery,
    >;

    /// Failed-work reports: (netuid, epoch, uid) -> reporters.
    #[pallet::storage]
    pub type WorkReporters<T: Config> = StorageNMap<
        _,
        (
            NMapKey<Twox64Concat, NetUid>,
            NMapKey<Twox64Concat, u64>,
            NMapKey<Twox64Concat, Uid>,
        ),
        BoundedVec<T::AccountId, ConstU32<MAX_REPORTERS>>,
        ValueQuery,
    >;

    /// Miners penalised in a subnet epoch: they get no incentive for it.
    #[pallet::storage]
    pub type WorkPenalized<T> = StorageNMap<
        _,
        (
            NMapKey<Twox64Concat, NetUid>,
            NMapKey<Twox64Concat, u64>,
            NMapKey<Twox64Concat, Uid>,
        ),
        (),
        OptionQuery,
    >;

    /// Epochs in which each hotkey was penalised in a subnet.
    #[pallet::storage]
    pub type WorkFailures<T: Config> = StorageDoubleMap<
        _,
        Twox64Concat,
        NetUid,
        Blake2_128Concat,
        T::AccountId,
        BoundedVec<u64, ConstU32<{ MAX_FAILURES_LIMIT as u32 }>>,
        ValueQuery,
    >;

    /// Hotkeys banned from a subnet: no more payouts there.
    #[pallet::storage]
    pub type SubnetBanned<T: Config> =
        StorageDoubleMap<_, Twox64Concat, NetUid, Blake2_128Concat, T::AccountId, (), OptionQuery>;

    #[pallet::storage]
    pub type Neurons<T: Config> =
        StorageDoubleMap<_, Twox64Concat, NetUid, Twox64Concat, Uid, T::AccountId, OptionQuery>;

    #[pallet::storage]
    pub type NeuronUid<T: Config> =
        StorageDoubleMap<_, Twox64Concat, NetUid, Blake2_128Concat, T::AccountId, Uid, OptionQuery>;

    #[pallet::storage]
    pub type NeuronCount<T> = StorageMap<_, Twox64Concat, NetUid, u16, ValueQuery>;

    /// Public endpoint a neuron serves on (e.g. `https://1.2.3.4:8091`), so validators can
    /// find miners without an off-chain directory.
    #[pallet::storage]
    pub type Endpoints<T: Config> = StorageDoubleMap<
        _,
        Twox64Concat,
        NetUid,
        Blake2_128Concat,
        T::AccountId,
        BoundedVec<u8, ConstU32<128>>,
        OptionQuery,
    >;

    /// Storage version 2 tier table; emptied by the v2 -> v3 migration.
    #[pallet::storage]
    pub type Tiers<T> = StorageMap<_, Twox64Concat, ModelId, LegacyTierInfo, OptionQuery>;

    /// The open model list (spec Section 5).
    #[pallet::storage]
    pub type Models<T: Config> =
        StorageMap<_, Twox64Concat, ModelId, ModelInfo<T::AccountId>, OptionQuery>;

    /// Id the next proposed model gets.
    #[pallet::storage]
    pub type NextModelId<T> = StorageValue<_, ModelId, ValueQuery>;

    /// Model declared by each neuron. Neurons that never declared run model 0 (Qwen3.5-2B).
    #[pallet::storage]
    pub type NeuronModel<T: Config> = StorageDoubleMap<
        _,
        Twox64Concat,
        NetUid,
        Blake2_128Concat,
        T::AccountId,
        ModelId,
        ValueQuery,
    >;

    /// Models each SN0 validator declares that it holds and checks.
    #[pallet::storage]
    pub type ValidatorModels<T: Config> = StorageMap<
        _,
        Blake2_128Concat,
        T::AccountId,
        BoundedVec<ModelId, ConstU32<MAX_MODELS>>,
        ValueQuery,
    >;

    /// Last block (exclusive) of the bootstrap period; 0 = no bootstrap.
    #[pallet::storage]
    pub type BootstrapUntil<T> = StorageValue<_, u32, ValueQuery>;

    /// During bootstrap only this account can register neurons (fee waived).
    #[pallet::storage]
    pub type BootstrapOperator<T: Config> = StorageValue<_, T::AccountId, OptionQuery>;

    #[pallet::genesis_config]
    #[derive(polkadot_sdk::frame_support::DefaultNoBound)]
    pub struct GenesisConfig<T: Config> {
        /// Devnets only: start unlocked, with the SN1 auction running from block 0.
        pub phase2_enabled: bool,
        /// Length of the disclosed bootstrap period in blocks (90 days = 648,000). 0 = none.
        pub bootstrap_blocks: u32,
        pub bootstrap_operator: Option<T::AccountId>,
        #[serde(skip)]
        pub _phantom: core::marker::PhantomData<T>,
    }

    #[pallet::genesis_build]
    impl<T: Config> BuildGenesisConfig for GenesisConfig<T> {
        fn build(&self) {
            Subnets::<T>::insert(
                SN0,
                SubnetInfo {
                    owner: None,
                    deposit: 0,
                    created: Zero::zero(),
                },
            );
            if self.phase2_enabled {
                Phase2Enabled::<T>::put(true);
                UnlockedAt::<T>::put(0);
                AuctionStart::<T>::put(0);
            }
            if self.bootstrap_blocks > 0 {
                assert!(
                    self.bootstrap_operator.is_some(),
                    "bootstrap needs an operator"
                );
                BootstrapUntil::<T>::put(self.bootstrap_blocks);
            }
            if let Some(op) = &self.bootstrap_operator {
                BootstrapOperator::<T>::put(op.clone());
            }
            Pallet::<T>::write_default_models();
        }
    }

    #[pallet::hooks]
    impl<T: Config> Hooks<BlockNumberFor<T>> for Pallet<T> {
        /// Once a day, until it has happened: unlock subnet registration if all criteria hold.
        fn on_initialize(n: BlockNumberFor<T>) -> Weight {
            let n: u32 = n.saturated_into();
            if n % DAY != 0 {
                return T::DbWeight::get().reads(1);
            }
            if Phase2Enabled::<T>::get() {
                let _ = Self::try_raise_cap(n);
                // One miner history per subnet.
                return T::DbWeight::get().reads_writes(8 + SubnetCount::<T>::get() as u64, 4);
            }
            let _ = Self::try_unlock_now();
            // Status reads plus a bounded referendum count.
            T::DbWeight::get().reads_writes(16, 3)
        }

        /// v0/v1 -> v2: auction storage replaces the doubling-deposit storage (no subnet was
        /// ever registered on mainnet, so there is nothing to move).
        /// v2 -> v3: the tier table becomes the open model list (chains from before tiers
        /// get the starting list).
        fn on_runtime_upgrade() -> Weight {
            let on_chain = StorageVersion::get::<Pallet<T>>();
            if on_chain >= 3 {
                return T::DbWeight::get().reads(1);
            }
            let mut w = T::DbWeight::get().reads(1);
            if on_chain < 2 {
                let prefix =
                    <Pallet<T> as polkadot_sdk::frame_support::traits::PalletInfoAccess>::name()
                        .as_bytes();
                for item in [&b"LastRegPrice"[..], &b"LastRegBlock"[..]] {
                    let _ = polkadot_sdk::frame_support::storage::migration::clear_storage_prefix(
                        prefix, item, b"", None, None,
                    );
                }
                w = w.saturating_add(T::DbWeight::get().writes(2));
            }
            w = w.saturating_add(Self::migrate_tiers_to_models());
            STORAGE_VERSION.put::<Pallet<T>>();
            w.saturating_add(T::DbWeight::get().writes(1))
        }
    }

    #[pallet::event]
    #[pallet::generate_deposit(pub(super) fn deposit_event)]
    pub enum Event<T: Config> {
        /// A slot was bought. `to_treasury` went to the treasury (0 once the Alpha module is
        /// installed: that half went to the subnet's pool), `burned` was burned.
        SubnetRegistered {
            netuid: NetUid,
            owner: T::AccountId,
            price: u128,
            to_treasury: u128,
            burned: u128,
        },
        /// A subnet was closed. Its payment is not refunded.
        SubnetDeregistered {
            netuid: NetUid,
        },
        NeuronRegistered {
            netuid: NetUid,
            uid: Uid,
            hotkey: T::AccountId,
        },
        /// All unlock criteria held; the SN1 auction starts at `auction_start`.
        RegistrationUnlocked {
            at: u32,
            auction_start: u32,
        },
        /// The next auction starts at block `at`.
        AuctionScheduled {
            at: u32,
            start_price: u128,
        },
        EndpointSet {
            netuid: NetUid,
            hotkey: T::AccountId,
        },
        /// The bootstrap period ended early; registration is open to everyone.
        BootstrapEnded {
            at: u32,
        },
        /// A neuron declared the model it runs.
        ModelDeclared {
            netuid: NetUid,
            hotkey: T::AccountId,
            model: ModelId,
        },
        /// Governance changed a model's G, tolerance or status.
        ModelUpdated {
            model: ModelId,
            status: ModelStatus,
        },
        /// A model was proposed for the list.
        ModelProposed {
            model: ModelId,
            proposer: T::AccountId,
        },
        /// The proposer withdrew a model that had not opened; the deposit was returned.
        ModelWithdrawn {
            model: ModelId,
        },
        /// A validator declared which models it holds and checks.
        ValidatorModelsDeclared {
            hotkey: T::AccountId,
            models: Vec<ModelId>,
        },
        /// Governance attested the audited, immutable Base swap contract.
        SwapAttested {
            contract: [u8; 20],
            audit_hash: [u8; 32],
        },
        MinSpacingSet {
            blocks: u32,
        },
        MaxSubnetsSet {
            max: u16,
        },
        /// A day's average of active SN0 miners was recorded.
        MinerDayRecorded {
            day: u32,
            average: u32,
        },
        OwnershipTransferred {
            netuid: NetUid,
            to: T::AccountId,
        },
        ControlChanged {
            netuid: NetUid,
            control: ControlKind,
            owner: T::AccountId,
        },
        ParamsSet {
            netuid: NetUid,
            params: SubnetParams,
        },
        MetadataSet {
            netuid: NetUid,
        },
        Proposed {
            netuid: NetUid,
            proposer: T::AccountId,
            end: u32,
        },
        Voted {
            netuid: NetUid,
            voter: T::AccountId,
            aye: bool,
        },
        /// A subnet DAO vote closed; `passed` tells whether the action was carried out.
        ProposalClosed {
            netuid: NetUid,
            passed: bool,
            ayes: u128,
            nays: u128,
        },
        DaoSpent {
            netuid: NetUid,
            to: T::AccountId,
            amount: u128,
        },
        WorkFailureReported {
            netuid: NetUid,
            epoch: u64,
            uid: Uid,
            reporter: T::AccountId,
            evidence: [u8; 32],
        },
        WorkPenalised {
            netuid: NetUid,
            epoch: u64,
            uid: Uid,
            hotkey: T::AccountId,
        },
        BannedFromSubnet {
            netuid: NetUid,
            hotkey: T::AccountId,
        },
        /// A dead subnet was removed (no epoch paid 3 miners in 30 days).
        DeadSubnetRemoved {
            netuid: NetUid,
        },
        /// The ladder doubled the cap.
        CapRaised {
            max: u16,
        },
    }

    #[pallet::error]
    pub enum Error<T> {
        /// Subnet registration has not unlocked yet.
        Phase2NotEnabled,
        TooManySubnets,
        /// The current price is above the buyer's limit.
        DepositAboveLimit,
        NoSuchSubnet,
        CannotDeregisterSn0,
        SubnetFull,
        AlreadyRegistered,
        NotRegistered,
        EndpointTooLong,
        /// During the bootstrap period only the bootstrap operator registers neurons.
        BootstrapRestricted,
        NotInBootstrap,
        /// The model is not in the list.
        UnknownModel,
        /// The model is not open for new declarations.
        ModelNotOpen,
        /// G, tolerance or memory is out of range.
        BadModelParams,
        ModelNameTooLong,
        /// No auction is running (waiting for the spacing period or a free slot).
        NoAuction,
        NameTooLong,
        DescriptionTooLong,
        EmptyName,
        /// The subnet is within its 180-day immunity.
        StillImmune,
        /// Not all unlock criteria hold yet.
        CriteriaNotMet,
        AlreadyUnlocked,
        AlreadyAttested,
        SpacingTooShort,
        /// The cap must be at least the current number of subnets and at most 256.
        BadMaxSubnets,
        /// Only the subnet's controller (owner key or multisig) may do this.
        NotController,
        /// SN0 is governed by Incentium governance.
        Sn0HasNoOwner,
        /// The subnet is run by its DAO: use a proposal.
        DaoControlled,
        /// Control can only move forward (Key -> Multisig -> Dao).
        ControlCannotGoBack,
        /// Ownership can only be handed over while one key controls the subnet.
        OnlyKeyCanTransfer,
        BadSignatories,
        /// A setting is outside the protocol's bounds.
        ParamOutOfBounds,
        /// The subnet is not run by a DAO.
        NotDao,
        ProposalOpen,
        NoProposal,
        VoteStillOpen,
        VoteClosed,
        /// Only stakers of the subnet may propose, vote or report.
        NotStaker,
        PenaltiesOff,
        AlreadyReported,
        TooManyReporters,
        /// The subnet is alive (or still immune): it cannot be removed as dead.
        NotDead,
        /// The model list is full.
        TooManyModels,
        EngineTooLong,
        /// A proposed model needs the hash of its weights file.
        NoWeightsHash,
        /// Only the proposer can withdraw a model, and only before it opens.
        NotProposer,
        /// That status change is not allowed (e.g. back to `Proposed`, or `Retiring` from a
        /// model that is not open).
        BadStatusChange,
        /// Fewer than three validators with enough stake declare that they hold the model.
        TooFewHolders,
        /// The weights hash is already pinned.
        HashAlreadySet,
        /// Model declarations need SN0 registration and at least `ModelDeclarerStake`.
        StakeTooLow,
        DuplicateModel,
    }

    #[pallet::call]
    impl<T: Config> Pallet<T> {
        /// Buy the slot on auction at the current price, publishing the subnet's name,
        /// description and incentive-code hash. Half the price is burned and half goes to the
        /// subnet's Alpha pool (to the treasury if the Alpha module is not installed); none of
        /// it is refunded.
        #[pallet::call_index(0)]
        #[pallet::weight(<T as Config>::WeightInfo::register_subnet())]
        pub fn register_subnet(
            origin: OriginFor<T>,
            max_price: u128,
            name: Vec<u8>,
            description: Vec<u8>,
            code_hash: [u8; 32],
        ) -> DispatchResult {
            let who = ensure_signed(origin)?;
            ensure!(Phase2Enabled::<T>::get(), Error::<T>::Phase2NotEnabled);
            ensure!(
                SubnetCount::<T>::get() < MaxSubnets::<T>::get(),
                Error::<T>::TooManySubnets
            );
            let price = Self::current_price().ok_or(Error::<T>::NoAuction)?;
            ensure!(price <= max_price, Error::<T>::DepositAboveLimit);
            ensure!(!name.is_empty(), Error::<T>::EmptyName);
            let meta = SubnetMetadata {
                name: name.try_into().map_err(|_| Error::<T>::NameTooLong)?,
                description: description
                    .try_into()
                    .map_err(|_| Error::<T>::DescriptionTooLong)?,
                code_hash,
            };
            let netuid = Self::next_free_netuid().ok_or(Error::<T>::TooManySubnets)?;

            let (half, burned) = payment_split(price);
            let to_treasury = if T::Sale::pools_installed() {
                // the subnet's Alpha pool holds this half until it opens (spec Section 7)
                T::Sale::subnet_sold(netuid, &who, half)?;
                0
            } else {
                T::Currency::transfer(&who, &T::TreasuryAccount::get(), half, Preservation::Preserve)?;
                half
            };
            T::Currency::burn_from(
                &who,
                burned,
                Preservation::Preserve,
                Precision::Exact,
                Fortitude::Polite,
            )?;

            let now = polkadot_sdk::frame_system::Pallet::<T>::block_number();
            Subnets::<T>::insert(
                netuid,
                SubnetInfo {
                    owner: Some(who.clone()),
                    deposit: price,
                    created: now,
                },
            );
            Metadata::<T>::insert(netuid, meta);
            SubnetCount::<T>::mutate(|c| *c += 1);
            if SubnetCount::<T>::get() >= MaxSubnets::<T>::get() {
                FullSince::<T>::put(
                    polkadot_sdk::frame_system::Pallet::<T>::block_number().saturated_into::<u32>(),
                );
            }
            Sales::<T>::mutate(|s| *s += 1);
            LastSalePrice::<T>::put(price);
            let now: u32 = now.saturated_into();
            let next = now.saturating_add(MinSpacing::<T>::get());
            AuctionStart::<T>::put(next);
            Self::deposit_event(Event::SubnetRegistered {
                netuid,
                owner: who,
                price,
                to_treasury,
                burned,
            });
            Self::deposit_event(Event::AuctionScheduled {
                at: next,
                start_price: Self::next_start_price(),
            });
            Ok(())
        }

        /// Close a subnet (Incentium governance: the emergency brake against abuse). Works
        /// also during the 180-day immunity, which only protects against removal as dead.
        /// Nothing is refunded.
        #[pallet::call_index(1)]
        #[pallet::weight(<T as Config>::WeightInfo::deregister_subnet(T::MaxNeurons::get() as u32))]
        pub fn deregister_subnet(origin: OriginFor<T>, netuid: NetUid) -> DispatchResult {
            ensure_root(origin)?;
            Self::do_deregister(netuid)
        }

        /// Register a hotkey as a neuron; the fee is burned.
        #[pallet::call_index(2)]
        #[pallet::weight(<T as Config>::WeightInfo::register_neuron())]
        pub fn register_neuron(
            origin: OriginFor<T>,
            netuid: NetUid,
            hotkey: T::AccountId,
        ) -> DispatchResult {
            let who = ensure_signed(origin)?;
            ensure!(Subnets::<T>::contains_key(netuid), Error::<T>::NoSuchSubnet);
            ensure!(
                !NeuronUid::<T>::contains_key(netuid, &hotkey),
                Error::<T>::AlreadyRegistered
            );
            let uid = NeuronCount::<T>::get(netuid);
            let params = Self::params(netuid);
            ensure!(
                uid < params.max_neurons.min(T::MaxNeurons::get()),
                Error::<T>::SubnetFull
            );
            if Self::in_bootstrap() {
                // Bootstrap: the operator registers its own hotkeys, without a fee.
                ensure!(
                    BootstrapOperator::<T>::get().as_ref() == Some(&who),
                    Error::<T>::BootstrapRestricted
                );
            } else {
                T::Currency::burn_from(
                    &who,
                    params.neuron_fee,
                    Preservation::Preserve,
                    Precision::Exact,
                    Fortitude::Polite,
                )?;
            }
            Neurons::<T>::insert(netuid, uid, hotkey.clone());
            NeuronUid::<T>::insert(netuid, hotkey.clone(), uid);
            NeuronCount::<T>::insert(netuid, uid + 1);
            Self::deposit_event(Event::NeuronRegistered {
                netuid,
                uid,
                hotkey,
            });
            Ok(())
        }

        /// Anyone may ask the chain to check the unlock criteria now instead of waiting for
        /// the daily check. Fails if a criterion does not hold.
        #[pallet::call_index(3)]
        #[pallet::weight(T::DbWeight::get().reads_writes(1_040, 3))]
        pub fn try_unlock(origin: OriginFor<T>) -> DispatchResult {
            ensure_signed(origin)?;
            ensure!(!Phase2Enabled::<T>::get(), Error::<T>::AlreadyUnlocked);
            ensure!(Self::try_unlock_now(), Error::<T>::CriteriaNotMet);
            Ok(())
        }

        /// Publish the endpoint the caller's neuron serves on.
        #[pallet::call_index(4)]
        #[pallet::weight(<T as Config>::WeightInfo::set_endpoint())]
        pub fn set_endpoint(
            origin: OriginFor<T>,
            netuid: NetUid,
            endpoint: Vec<u8>,
        ) -> DispatchResult {
            let who = ensure_signed(origin)?;
            ensure!(
                NeuronUid::<T>::contains_key(netuid, &who),
                Error::<T>::NotRegistered
            );
            let e: BoundedVec<u8, ConstU32<128>> = endpoint
                .try_into()
                .map_err(|_| Error::<T>::EndpointTooLong)?;
            Endpoints::<T>::insert(netuid, &who, e);
            Self::deposit_event(Event::EndpointSet {
                netuid,
                hotkey: who,
            });
            Ok(())
        }

        /// End the bootstrap period early (operator or governance). It can never be
        /// extended or restarted.
        #[pallet::call_index(5)]
        #[pallet::weight(<T as Config>::WeightInfo::end_bootstrap())]
        pub fn end_bootstrap(origin: OriginFor<T>) -> DispatchResult {
            let is_root = ensure_root(origin.clone()).is_ok();
            if !is_root {
                let who = ensure_signed(origin)?;
                ensure!(
                    BootstrapOperator::<T>::get().as_ref() == Some(&who),
                    Error::<T>::BootstrapRestricted
                );
            }
            ensure!(Self::in_bootstrap(), Error::<T>::NotInBootstrap);
            let now: u32 = polkadot_sdk::frame_system::Pallet::<T>::block_number().saturated_into();
            BootstrapUntil::<T>::put(now);
            Self::deposit_event(Event::BootstrapEnded { at: now });
            Ok(())
        }

        /// Declare the model the caller's neuron runs. The model must be open. Validators
        /// check the neuron against that model; a change counts from the next epoch.
        #[pallet::call_index(6)]
        #[pallet::weight(<T as Config>::WeightInfo::declare_model())]
        pub fn declare_model(
            origin: OriginFor<T>,
            netuid: NetUid,
            model: ModelId,
        ) -> DispatchResult {
            let who = ensure_signed(origin)?;
            ensure!(
                NeuronUid::<T>::contains_key(netuid, &who),
                Error::<T>::NotRegistered
            );
            let info = Models::<T>::get(model).ok_or(Error::<T>::UnknownModel)?;
            ensure!(info.status == ModelStatus::Open, Error::<T>::ModelNotOpen);
            NeuronModel::<T>::insert(netuid, &who, model);
            Self::deposit_event(Event::ModelDeclared {
                netuid,
                hotkey: who,
                model,
            });
            Ok(())
        }

        /// Set a model's G, tolerance and status, and pin its weights hash if it has none
        /// (governance, Parameters track). Opening needs a pinned hash and at least three
        /// validators that declare the model; it returns the proposer's deposit. Closing a
        /// model that never opened burns the deposit. Neurons that declared a model that
        /// retires or closes keep the declaration until they declare another.
        #[pallet::call_index(7)]
        #[pallet::weight(<T as Config>::WeightInfo::set_model())]
        pub fn set_model(
            origin: OriginFor<T>,
            model: ModelId,
            g_ppm: u32,
            tolerance_ppm: u32,
            status: ModelStatus,
            weights_hash: Option<[u8; 32]>,
        ) -> DispatchResult {
            ensure_root(origin)?;
            Self::check_model_params(g_ppm, tolerance_ppm, 1)?;
            let mut info = Models::<T>::get(model).ok_or(Error::<T>::UnknownModel)?;
            if let Some(h) = weights_hash {
                ensure!(info.weights_hash == [0; 32], Error::<T>::HashAlreadySet);
                ensure!(h != [0; 32], Error::<T>::NoWeightsHash);
                info.weights_hash = h;
            }
            use ModelStatus::*;
            let ok = match (info.status, status) {
                (a, b) if a == b => true,
                (_, Proposed) => false,
                (Open, Retiring) => true,
                (_, Retiring) => false,
                _ => true,
            };
            ensure!(ok, Error::<T>::BadStatusChange);
            if status == Open && info.status != Open {
                ensure!(info.weights_hash != [0; 32], Error::<T>::NoWeightsHash);
                ensure!(
                    Self::model_holders(model) >= MIN_MODEL_HOLDERS,
                    Error::<T>::TooFewHolders
                );
            }
            if info.deposit > 0 {
                if let Some(proposer) = info.proposer.clone() {
                    if status == Open {
                        T::Currency::transfer(
                            &Self::account_id(),
                            &proposer,
                            info.deposit,
                            Preservation::Expendable,
                        )?;
                        info.deposit = 0;
                    } else if status == Closed {
                        T::Currency::burn_from(
                            &Self::account_id(),
                            info.deposit,
                            Preservation::Expendable,
                            Precision::Exact,
                            Fortitude::Force,
                        )?;
                        info.deposit = 0;
                    }
                }
            }
            info.g_ppm = g_ppm;
            info.tolerance_ppm = tolerance_ppm;
            info.status = status;
            Models::<T>::insert(model, info);
            Self::deposit_event(Event::ModelUpdated { model, status });
            Ok(())
        }

        /// Attest, once, that the Base swap contract is audited and immutable (governance).
        #[pallet::call_index(8)]
        #[pallet::weight(T::DbWeight::get().reads_writes(1, 1))]
        pub fn attest_swap(
            origin: OriginFor<T>,
            contract: [u8; 20],
            audit_hash: [u8; 32],
        ) -> DispatchResult {
            ensure_root(origin)?;
            ensure!(!SwapAttested::<T>::exists(), Error::<T>::AlreadyAttested);
            SwapAttested::<T>::put(SwapAttestation {
                contract,
                audit_hash,
            });
            Self::deposit_event(Event::SwapAttested {
                contract,
                audit_hash,
            });
            Ok(())
        }

        /// Change the gap between a sale and the next auction (governance), at least 30 days.
        #[pallet::call_index(9)]
        #[pallet::weight(T::DbWeight::get().writes(1))]
        pub fn set_min_spacing(origin: OriginFor<T>, blocks: u32) -> DispatchResult {
            ensure_root(origin)?;
            ensure!(blocks >= MIN_SPACING_FLOOR, Error::<T>::SpacingTooShort);
            MinSpacing::<T>::put(blocks);
            Self::deposit_event(Event::MinSpacingSet { blocks });
            Ok(())
        }

        /// Change the cap on subnets (governance).
        #[pallet::call_index(10)]
        #[pallet::weight(T::DbWeight::get().reads_writes(3, 2))]
        pub fn set_max_subnets(origin: OriginFor<T>, max: u16) -> DispatchResult {
            ensure_root(origin)?;
            let count = SubnetCount::<T>::get();
            ensure!(
                max >= count && max <= MAX_SUBNETS_LIMIT,
                Error::<T>::BadMaxSubnets
            );
            let old_max = MaxSubnets::<T>::get();
            let was_full = count >= old_max;
            MaxSubnets::<T>::put(max);
            let now: u32 = polkadot_sdk::frame_system::Pallet::<T>::block_number().saturated_into();
            if max > old_max {
                LastCapRaise::<T>::put(now);
            }
            if count >= max {
                if !was_full {
                    FullSince::<T>::put(now);
                }
            } else {
                FullSince::<T>::kill();
            }
            if was_full && count < max {
                Self::slot_freed(now);
            }
            Self::deposit_event(Event::MaxSubnetsSet { max });
            Ok(())
        }

        // -------------------------------------------------------- subnet control

        /// Hand the subnet to another single key (controller in `Key` or `Multisig` mode).
        #[pallet::call_index(11)]
        #[pallet::weight(T::DbWeight::get().reads_writes(2, 1))]
        pub fn transfer_ownership(
            origin: OriginFor<T>,
            netuid: NetUid,
            to: T::AccountId,
        ) -> DispatchResult {
            Self::ensure_controller(origin, netuid)?;
            // One key or a multisig may sell or hand over the subnet; the new owner is one
            // key. A DAO subnet stays with its stakers.
            ensure!(
                Control::<T>::get(netuid) != ControlKind::Dao,
                Error::<T>::OnlyKeyCanTransfer
            );
            Control::<T>::insert(netuid, ControlKind::Key);
            MultisigOf::<T>::remove(netuid);
            Self::set_owner(netuid, to.clone());
            Self::deposit_event(Event::OwnershipTransferred { netuid, to });
            Ok(())
        }

        /// Move control to a multisig of `signatories` (2..=20) with `threshold` (2..=n).
        /// The owner account becomes the multisig account (same derivation as
        /// `pallet-multisig`), which then signs through `pallet-multisig`.
        #[pallet::call_index(12)]
        #[pallet::weight(T::DbWeight::get().reads_writes(2, 3))]
        pub fn set_multisig(
            origin: OriginFor<T>,
            netuid: NetUid,
            mut signatories: Vec<T::AccountId>,
            threshold: u16,
        ) -> DispatchResult {
            Self::ensure_controller(origin, netuid)?;
            ensure!(
                Control::<T>::get(netuid) == ControlKind::Key,
                Error::<T>::ControlCannotGoBack
            );
            signatories.sort();
            signatories.dedup();
            let n = signatories.len();
            ensure!(
                n >= 2
                    && n <= MAX_SIGNATORIES as usize
                    && threshold >= 2
                    && threshold as usize <= n,
                Error::<T>::BadSignatories
            );
            let account = Self::multi_account_id(&signatories, threshold);
            let bounded: BoundedVec<T::AccountId, ConstU32<MAX_SIGNATORIES>> = signatories
                .try_into()
                .map_err(|_| Error::<T>::BadSignatories)?;
            MultisigOf::<T>::insert(netuid, (threshold, bounded));
            Control::<T>::insert(netuid, ControlKind::Multisig);
            Self::set_owner(netuid, account.clone());
            Self::deposit_event(Event::ControlChanged {
                netuid,
                control: ControlKind::Multisig,
                owner: account,
            });
            Ok(())
        }

        /// Hand the subnet to its stakers for good. The owner share is paid to the subnet's
        /// DAO account, which only DAO votes can spend.
        #[pallet::call_index(13)]
        #[pallet::weight(T::DbWeight::get().reads_writes(2, 3))]
        pub fn set_dao(origin: OriginFor<T>, netuid: NetUid) -> DispatchResult {
            Self::ensure_controller(origin, netuid)?;
            let account = Self::dao_account(netuid);
            MultisigOf::<T>::remove(netuid);
            Control::<T>::insert(netuid, ControlKind::Dao);
            Self::set_owner(netuid, account.clone());
            Self::deposit_event(Event::ControlChanged {
                netuid,
                control: ControlKind::Dao,
                owner: account,
            });
            Ok(())
        }

        /// Change the subnet's settings (controller), within the protocol's bounds.
        #[pallet::call_index(14)]
        #[pallet::weight(T::DbWeight::get().reads_writes(2, 1))]
        pub fn set_params(
            origin: OriginFor<T>,
            netuid: NetUid,
            params: SubnetParams,
        ) -> DispatchResult {
            Self::ensure_controller(origin, netuid)?;
            Self::apply_params(netuid, params)
        }

        /// Update name, description and incentive-code hash (controller), e.g. for a new
        /// release of the incentive mechanism.
        #[pallet::call_index(15)]
        #[pallet::weight(T::DbWeight::get().reads_writes(2, 1))]
        pub fn set_metadata(
            origin: OriginFor<T>,
            netuid: NetUid,
            name: Vec<u8>,
            description: Vec<u8>,
            code_hash: [u8; 32],
        ) -> DispatchResult {
            Self::ensure_controller(origin, netuid)?;
            let meta = Self::make_metadata(name, description, code_hash)?;
            Metadata::<T>::insert(netuid, meta);
            Self::deposit_event(Event::MetadataSet { netuid });
            Ok(())
        }

        /// Open a vote in a DAO subnet (any staker of the subnet; one vote at a time).
        #[pallet::call_index(16)]
        #[pallet::weight(T::DbWeight::get().reads_writes(4, 2))]
        pub fn propose(
            origin: OriginFor<T>,
            netuid: NetUid,
            action: SubnetAction<T::AccountId>,
        ) -> DispatchResult {
            let who = ensure_signed(origin)?;
            ensure!(
                Control::<T>::get(netuid) == ControlKind::Dao,
                Error::<T>::NotDao
            );
            ensure!(
                T::StakeInfo::stake_of(netuid, &who) > 0,
                Error::<T>::NotStaker
            );
            ensure!(
                !Proposals::<T>::contains_key(netuid),
                Error::<T>::ProposalOpen
            );
            // Check the action can be carried out before anyone votes on it.
            match &action {
                SubnetAction::SetParams(p) => Self::check_params(p)?,
                SubnetAction::SetMetadata(m) => ensure!(!m.name.is_empty(), Error::<T>::EmptyName),
                SubnetAction::Spend { .. } => {}
            }
            let now: u32 = polkadot_sdk::frame_system::Pallet::<T>::block_number().saturated_into();
            let end = now.saturating_add(DAO_VOTE_BLOCKS);
            Proposals::<T>::insert(
                netuid,
                Proposal {
                    action,
                    proposer: who.clone(),
                    end,
                },
            );
            Votes::<T>::insert(netuid, &who, true);
            Self::deposit_event(Event::Proposed {
                netuid,
                proposer: who,
                end,
            });
            Ok(())
        }

        /// Vote on the open proposal (stakers of the subnet). A later vote replaces an
        /// earlier one. Votes are weighed by each voter's stake when the vote closes.
        #[pallet::call_index(17)]
        #[pallet::weight(T::DbWeight::get().reads_writes(3, 1))]
        pub fn vote(origin: OriginFor<T>, netuid: NetUid, aye: bool) -> DispatchResult {
            let who = ensure_signed(origin)?;
            let p = Proposals::<T>::get(netuid).ok_or(Error::<T>::NoProposal)?;
            let now: u32 = polkadot_sdk::frame_system::Pallet::<T>::block_number().saturated_into();
            ensure!(now < p.end, Error::<T>::VoteClosed);
            ensure!(
                T::StakeInfo::stake_of(netuid, &who) > 0,
                Error::<T>::NotStaker
            );
            Votes::<T>::insert(netuid, &who, aye);
            Self::deposit_event(Event::Voted {
                netuid,
                voter: who,
                aye,
            });
            Ok(())
        }

        /// Close a vote whose period is over (anyone). If aye stake exceeds nay stake and is
        /// at least 1/3 of the subnet's stake, the action is carried out at once.
        #[pallet::call_index(18)]
        #[pallet::weight(T::DbWeight::get().reads_writes(4 + 2 * T::MaxNeurons::get() as u64, 4 + T::MaxNeurons::get() as u64))]
        pub fn close_proposal(origin: OriginFor<T>, netuid: NetUid) -> DispatchResult {
            ensure_signed(origin)?;
            let p = Proposals::<T>::get(netuid).ok_or(Error::<T>::NoProposal)?;
            let now: u32 = polkadot_sdk::frame_system::Pallet::<T>::block_number().saturated_into();
            ensure!(now >= p.end, Error::<T>::VoteStillOpen);
            let (mut ayes, mut nays) = (0u128, 0u128);
            for (voter, aye) in Votes::<T>::drain_prefix(netuid) {
                let st = T::StakeInfo::stake_of(netuid, &voter);
                if aye {
                    ayes = ayes.saturating_add(st);
                } else {
                    nays = nays.saturating_add(st);
                }
            }
            Proposals::<T>::remove(netuid);
            let total = T::StakeInfo::total(netuid);
            let quorum = ict_economics::mul_div(total, SUBNET_QUORUM_PPM, ict_economics::PPM);
            let mut passed = ayes > nays && ayes >= quorum && ayes > 0;
            if passed {
                passed = Self::carry_out(netuid, p.action).is_ok();
            }
            Self::deposit_event(Event::ProposalClosed {
                netuid,
                passed,
                ayes,
                nays,
            });
            Ok(())
        }

        /// Remove a dead subnet (anyone): past its 180-day immunity, and no epoch in the last
        /// 30 days paid at least 3 distinct miners. Nothing is refunded.
        #[pallet::call_index(20)]
        #[pallet::weight(<T as Config>::WeightInfo::deregister_subnet(T::MaxNeurons::get() as u32))]
        pub fn remove_dead_subnet(origin: OriginFor<T>, netuid: NetUid) -> DispatchResult {
            ensure_signed(origin)?;
            ensure!(Self::is_dead(netuid), Error::<T>::NotDead);
            Self::do_deregister(netuid)?;
            Self::deposit_event(Event::DeadSubnetRemoved { netuid });
            Ok(())
        }

        /// A validator of the subnet reports that miner `uid` failed its work in `epoch`.
        /// When reporters holding at least half of the subnet's active stake agree, the miner gets
        /// no incentive for that epoch; `max_failures` penalties inside `failure_window`
        /// epochs ban the hotkey from the subnet. Only for subnets that turned penalties on.
        #[pallet::call_index(19)]
        #[pallet::weight(T::DbWeight::get().reads_writes(8, 5))]
        pub fn report_failed_work(
            origin: OriginFor<T>,
            netuid: NetUid,
            epoch: u64,
            uid: Uid,
            evidence: [u8; 32],
        ) -> DispatchResult {
            let who = ensure_signed(origin)?;
            ensure!(netuid != SN0, Error::<T>::Sn0HasNoOwner);
            let params = Self::params(netuid);
            ensure!(params.penalties, Error::<T>::PenaltiesOff);
            ensure!(
                T::StakeInfo::validator_stake(netuid, &who) > 0,
                Error::<T>::NotStaker
            );
            let hotkey = Neurons::<T>::get(netuid, uid).ok_or(Error::<T>::NotRegistered)?;
            let reporters = WorkReporters::<T>::try_mutate(
                (netuid, epoch, uid),
                |r| -> Result<Vec<T::AccountId>, DispatchError> {
                    ensure!(!r.contains(&who), Error::<T>::AlreadyReported);
                    r.try_push(who.clone())
                        .map_err(|_| Error::<T>::TooManyReporters)?;
                    Ok(r.to_vec())
                },
            )?;
            Self::deposit_event(Event::WorkFailureReported {
                netuid,
                epoch,
                uid,
                reporter: who,
                evidence,
            });
            if WorkPenalized::<T>::contains_key((netuid, epoch, uid)) {
                return Ok(());
            }
            // Reporter stake counted as it stands now, so moved stake cannot count twice.
            let reported: u128 = reporters
                .iter()
                .map(|r| T::StakeInfo::validator_stake(netuid, r))
                .fold(0, |a, b| a.saturating_add(b));
            // Lost payout: at least half of the stake active in the last settled epoch.
            let quorum =
                ict_economics::mul_div(T::StakeInfo::active(netuid), PENALTY_QUORUM_PPM, 1)
                    .div_ceil(ict_economics::PPM);
            if reported >= quorum && reported > 0 {
                WorkPenalized::<T>::insert((netuid, epoch, uid), ());
                Self::deposit_event(Event::WorkPenalised {
                    netuid,
                    epoch,
                    uid,
                    hotkey: hotkey.clone(),
                });
                let window_start = epoch.saturating_sub(params.failure_window as u64);
                let count = WorkFailures::<T>::mutate(netuid, &hotkey, |f| {
                    f.retain(|e| *e >= window_start);
                    if f.is_full() {
                        f.remove(0);
                    }
                    let _ = f.try_push(epoch);
                    f.len() as u32
                });
                if count >= params.max_failures as u32 {
                    SubnetBanned::<T>::insert(netuid, &hotkey, ());
                    Self::deposit_event(Event::BannedFromSubnet { netuid, hotkey });
                }
            }
            Ok(())
        }

        /// Propose a model for the open list. `ModelDeposit` is held until the model opens
        /// (returned), the proposer withdraws it (returned) or governance closes it without
        /// opening it (burned). The model opens only through `set_model`.
        #[pallet::call_index(21)]
        #[pallet::weight(<T as Config>::WeightInfo::propose_model())]
        pub fn propose_model(
            origin: OriginFor<T>,
            name: Vec<u8>,
            weights_hash: [u8; 32],
            engine: Vec<u8>,
            mem_mb: u32,
            g_ppm: u32,
            tolerance_ppm: u32,
        ) -> DispatchResult {
            let who = ensure_signed(origin)?;
            Self::check_model_params(g_ppm, tolerance_ppm, mem_mb)?;
            ensure!(weights_hash != [0; 32], Error::<T>::NoWeightsHash);
            let name: BoundedVec<u8, ConstU32<64>> =
                name.try_into().map_err(|_| Error::<T>::ModelNameTooLong)?;
            let engine: BoundedVec<u8, ConstU32<64>> =
                engine.try_into().map_err(|_| Error::<T>::EngineTooLong)?;
            let id = NextModelId::<T>::get();
            ensure!((id as u32) < MAX_MODELS, Error::<T>::TooManyModels);
            let deposit = T::ModelDeposit::get();
            T::Currency::transfer(&who, &Self::account_id(), deposit, Preservation::Preserve)?;
            Models::<T>::insert(
                id,
                ModelInfo {
                    name,
                    weights_hash,
                    engine,
                    mem_mb,
                    g_ppm,
                    tolerance_ppm,
                    status: ModelStatus::Proposed,
                    proposer: Some(who.clone()),
                    deposit,
                },
            );
            NextModelId::<T>::put(id + 1);
            Self::deposit_event(Event::ModelProposed {
                model: id,
                proposer: who,
            });
            Ok(())
        }

        /// Withdraw a model the caller proposed, before it opens. The deposit is returned and
        /// the entry is closed.
        #[pallet::call_index(22)]
        #[pallet::weight(<T as Config>::WeightInfo::withdraw_model())]
        pub fn withdraw_model(origin: OriginFor<T>, model: ModelId) -> DispatchResult {
            let who = ensure_signed(origin)?;
            let mut info = Models::<T>::get(model).ok_or(Error::<T>::UnknownModel)?;
            ensure!(
                info.status == ModelStatus::Proposed && info.proposer.as_ref() == Some(&who),
                Error::<T>::NotProposer
            );
            if info.deposit > 0 {
                T::Currency::transfer(
                    &Self::account_id(),
                    &who,
                    info.deposit,
                    Preservation::Expendable,
                )?;
            }
            info.deposit = 0;
            info.status = ModelStatus::Closed;
            Models::<T>::insert(model, info);
            Self::deposit_event(Event::ModelWithdrawn { model });
            Ok(())
        }

        /// Declare the models this SN0 validator holds and checks (replaces the earlier
        /// declaration; an empty list removes it). The validator must be registered on SN0
        /// with at least `ModelDeclarerStake`; at check time its stake must still be there
        /// for the declaration to count towards opening a model.
        #[pallet::call_index(23)]
        #[pallet::weight(<T as Config>::WeightInfo::declare_validator_models(models.len() as u32))]
        pub fn declare_validator_models(
            origin: OriginFor<T>,
            models: Vec<ModelId>,
        ) -> DispatchResult {
            let who = ensure_signed(origin)?;
            if models.is_empty() {
                ValidatorModels::<T>::remove(&who);
                Self::deposit_event(Event::ValidatorModelsDeclared {
                    hotkey: who,
                    models,
                });
                return Ok(());
            }
            ensure!(Self::can_declare_models(&who), Error::<T>::StakeTooLow);
            ensure!(models.len() as u32 <= MAX_MODELS, Error::<T>::TooManyModels);
            for (i, m) in models.iter().enumerate() {
                let info = Models::<T>::get(m).ok_or(Error::<T>::UnknownModel)?;
                ensure!(info.status != ModelStatus::Closed, Error::<T>::ModelNotOpen);
                ensure!(!models[..i].contains(m), Error::<T>::DuplicateModel);
            }
            let bounded: BoundedVec<ModelId, ConstU32<MAX_MODELS>> =
                models.clone().try_into().map_err(|_| Error::<T>::TooManyModels)?;
            ValidatorModels::<T>::insert(&who, bounded);
            Self::deposit_event(Event::ValidatorModelsDeclared {
                hotkey: who,
                models,
            });
            Ok(())
        }
    }

    impl<T: Config> Pallet<T> {
        /// G above 0 and at most 100, tolerance at most 10 nats, memory above 0.
        fn check_model_params(g_ppm: u32, tolerance_ppm: u32, mem_mb: u32) -> DispatchResult {
            ensure!(
                g_ppm > 0 && g_ppm <= 100_000_000 && tolerance_ppm <= 10_000_000 && mem_mb > 0,
                Error::<T>::BadModelParams
            );
            Ok(())
        }

        /// Registered on SN0 with at least `ModelDeclarerStake` of SN0 stake.
        pub fn can_declare_models(who: &T::AccountId) -> bool {
            NeuronUid::<T>::contains_key(SN0, who)
                && T::StakeInfo::stake_of(SN0, who) >= T::ModelDeclarerStake::get()
        }

        /// Validators that declare `model` and still qualify to declare. Bounded by the
        /// number of SN0 neurons, since only SN0 neurons can declare.
        pub fn model_holders(model: ModelId) -> u32 {
            ValidatorModels::<T>::iter()
                .filter(|(who, ms)| ms.contains(&model) && Self::can_declare_models(who))
                .count() as u32
        }

        /// Write the starting model list (genesis, or chains from before tiers).
        fn write_default_models() {
            for (id, info) in default_models::<T::AccountId>() {
                if !Models::<T>::contains_key(id) {
                    Models::<T>::insert(id, info);
                }
            }
            let n = default_models::<T::AccountId>().len() as ModelId;
            NextModelId::<T>::mutate(|id| *id = (*id).max(n));
        }

        /// v2 -> v3: the tier table becomes the model list (ids, G, tolerance, names and
        /// pinned hashes kept; open tiers stay open), and each neuron's declared tier becomes
        /// its declared model.
        fn migrate_tiers_to_models() -> Weight {
            let defaults = default_models::<T::AccountId>();
            let mut n = 0u64;
            for (id, t) in Tiers::<T>::drain() {
                n += 1;
                let base = defaults.iter().find(|(d, _)| *d == id).map(|(_, i)| i.clone());
                // Open tiers stay open; closed ones take the starting list's status
                // (Proposed, or Closed for 122B), never Open.
                let status = match (t.open, base.as_ref().map(|b| b.status)) {
                    (true, _) => ModelStatus::Open,
                    (false, Some(ModelStatus::Closed)) => ModelStatus::Closed,
                    _ => ModelStatus::Proposed,
                };
                Models::<T>::insert(
                    id,
                    ModelInfo {
                        name: t.model,
                        weights_hash: t.model_hash,
                        engine: BoundedVec::truncate_from(b"llama.cpp".to_vec()),
                        mem_mb: base.as_ref().map(|b| b.mem_mb).unwrap_or(1),
                        g_ppm: t.weight_ppm,
                        tolerance_ppm: t.tolerance_ppm,
                        status,
                        proposer: None,
                        deposit: 0,
                    },
                );
            }
            Self::write_default_models();
            let prefix =
                <Pallet<T> as polkadot_sdk::frame_support::traits::PalletInfoAccess>::name()
                    .as_bytes();
            let old = polkadot_sdk::frame_support::storage::storage_prefix(prefix, b"NeuronTier");
            let new = polkadot_sdk::frame_support::storage::storage_prefix(prefix, b"NeuronModel");
            polkadot_sdk::frame_support::storage::migration::move_prefix(&old, &new);
            // The tier table had 4 entries; neurons are bounded by the subnet slots.
            T::DbWeight::get().reads_writes(n + 8 + 1024, 2 * n + 8 + 1024)
        }

        /// True while the disclosed bootstrap period runs.
        pub fn in_bootstrap() -> bool {
            let now: u32 = polkadot_sdk::frame_system::Pallet::<T>::block_number().saturated_into();
            now < BootstrapUntil::<T>::get()
        }

        pub fn account_id() -> T::AccountId {
            T::PalletId::get().into_account_truncating()
        }

        /// Start price of the next slot to be sold.
        pub fn next_start_price() -> u128 {
            start_price(Sales::<T>::get() + 1, LastSalePrice::<T>::get())
        }

        /// Price of the slot on auction now; `None` when no auction runs.
        pub fn current_price() -> Option<u128> {
            if !Phase2Enabled::<T>::get() || SubnetCount::<T>::get() >= MaxSubnets::<T>::get() {
                return None;
            }
            let start = AuctionStart::<T>::get()?;
            let now: u32 = polkadot_sdk::frame_system::Pallet::<T>::block_number().saturated_into();
            if now < start {
                return None;
            }
            let sale = Sales::<T>::get() + 1;
            Some(auction_price(
                Self::next_start_price(),
                (now - start) as u64,
                floor_price(sale),
            ))
        }

        /// A slot became free while the cap was reached: an auction waiting for it starts now.
        fn slot_freed(now: u32) {
            if Phase2Enabled::<T>::get() && AuctionStart::<T>::get().map_or(false, |s| s < now) {
                AuctionStart::<T>::put(now);
                Self::deposit_event(Event::AuctionScheduled {
                    at: now,
                    start_price: Self::next_start_price(),
                });
            }
        }

        /// Price the slot on sale would have at `block`, if nothing changes before then.
        pub fn price_at(block: u32) -> Option<u128> {
            if !Phase2Enabled::<T>::get() || SubnetCount::<T>::get() >= MaxSubnets::<T>::get() {
                return None;
            }
            let start = AuctionStart::<T>::get()?;
            if block < start {
                return None;
            }
            let sale = Sales::<T>::get() + 1;
            Some(auction_price(
                Self::next_start_price(),
                (block - start) as u64,
                floor_price(sale),
            ))
        }

        pub fn auction_info() -> AuctionInfo {
            let now: u32 = polkadot_sdk::frame_system::Pallet::<T>::block_number().saturated_into();
            let sale = Sales::<T>::get() + 1;
            AuctionInfo {
                now,
                price: Self::current_price(),
                start_block: AuctionStart::<T>::get(),
                start_price: Self::next_start_price(),
                floor: floor_price(sale),
                next_sale: sale,
                subnet_count: SubnetCount::<T>::get(),
                max_subnets: MaxSubnets::<T>::get(),
                min_spacing: MinSpacing::<T>::get(),
                full_since: FullSince::<T>::get(),
                last_cap_raise: LastCapRaise::<T>::get(),
            }
        }

        /// Every subnet, SN0 first (the runtime fills in `emission_ppm`).
        pub fn subnet_summaries() -> Vec<SubnetSummary<T::AccountId>> {
            let mut out: Vec<SubnetSummary<T::AccountId>> = Subnets::<T>::iter()
                .map(|(netuid, info)| {
                    let meta = Metadata::<T>::get(netuid);
                    let created: u32 = info.created.saturated_into();
                    SubnetSummary {
                        netuid,
                        owner: info.owner,
                        control: Control::<T>::get(netuid),
                        created,
                        price_paid: info.deposit,
                        name: meta.as_ref().map(|m| m.name.to_vec()).unwrap_or_default(),
                        description: meta
                            .as_ref()
                            .map(|m| m.description.to_vec())
                            .unwrap_or_default(),
                        code_hash: meta.map(|m| m.code_hash).unwrap_or_default(),
                        neurons: NeuronCount::<T>::get(netuid),
                        stake: T::StakeInfo::total(netuid),
                        emission_ppm: 0,
                        immune_until: if netuid == SN0 {
                            0
                        } else {
                            created.saturating_add(IMMUNITY_BLOCKS)
                        },
                        miners_avg: if netuid == SN0 {
                            Self::miners_average()
                        } else {
                            Self::subnet_miners_average(netuid)
                        },
                        dead: Self::is_dead(netuid),
                    }
                })
                .collect();
            out.sort_by_key(|s| s.netuid);
            out
        }

        /// 30-day average of active SN0 miners, once 30 days are recorded.
        pub fn miners_average() -> Option<u32> {
            let h = MinerHistory::<T>::get();
            if (h.len() as u32) < MINER_WINDOW_DAYS {
                return None;
            }
            let sum: u64 = h.iter().map(|x| *x as u64).sum();
            Some((sum / h.len() as u64) as u32)
        }

        /// Called by consensus after each SN0 epoch with the number of miners paid in it
        /// (neither penalised by a failed spot check nor banned). Rolls the daily average.
        pub fn note_sn0_epoch(active_miners: u32) {
            let now: u32 = polkadot_sdk::frame_system::Pallet::<T>::block_number().saturated_into();
            let today = now / DAY;
            let (day, sum, epochs) = MinerDay::<T>::get();
            if day != today && epochs > 0 {
                let avg = (sum / epochs as u64) as u32;
                MinerHistory::<T>::mutate(|h| {
                    if h.is_full() {
                        h.remove(0);
                    }
                    let _ = h.try_push(avg);
                });
                Self::deposit_event(Event::MinerDayRecorded { day, average: avg });
            }
            if day != today {
                MinerDay::<T>::put((today, active_miners as u64, 1));
            } else {
                MinerDay::<T>::put((today, sum + active_miners as u64, epochs + 1));
            }
        }

        /// Where the unlock criteria stand.
        pub fn unlock_status() -> UnlockStatus {
            let block: u32 =
                polkadot_sdk::frame_system::Pallet::<T>::block_number().saturated_into();
            UnlockStatus {
                unlocked: Phase2Enabled::<T>::get(),
                block,
                min_block: UNLOCK_MIN_BLOCK,
                miners_avg: Self::miners_average(),
                miner_days: MinerHistory::<T>::decode_len().unwrap_or(0) as u32,
                min_miners: UNLOCK_MIN_MINERS,
                sudo_removed: T::Unlock::sudo_removed(),
                approved_referenda: T::Unlock::approved_referenda(UNLOCK_MIN_REFERENDA),
                min_referenda: UNLOCK_MIN_REFERENDA,
                swap_attested: SwapAttested::<T>::exists(),
                alpha_installed: T::Unlock::alpha_installed(),
            }
        }

        /// True if every criterion holds.
        pub fn criteria_met(s: &UnlockStatus) -> bool {
            s.block >= s.min_block
                && s.miners_avg.map_or(false, |m| m >= s.min_miners)
                && s.sudo_removed
                && s.approved_referenda >= s.min_referenda
                && s.swap_attested
                && s.alpha_installed
        }

        /// Unlock if all criteria hold. The SN1 auction starts in the same block.
        fn try_unlock_now() -> bool {
            if Phase2Enabled::<T>::get() {
                return false;
            }
            let s = Self::unlock_status();
            if !Self::criteria_met(&s) {
                return false;
            }
            Phase2Enabled::<T>::put(true);
            UnlockedAt::<T>::put(s.block);
            AuctionStart::<T>::put(s.block);
            Self::deposit_event(Event::RegistrationUnlocked {
                at: s.block,
                auction_start: s.block,
            });
            Self::deposit_event(Event::AuctionScheduled {
                at: s.block,
                start_price: Self::next_start_price(),
            });
            true
        }

        fn do_deregister(netuid: NetUid) -> DispatchResult {
            ensure!(netuid != SN0, Error::<T>::CannotDeregisterSn0);
            ensure!(Subnets::<T>::contains_key(netuid), Error::<T>::NoSuchSubnet);
            let now: u32 = polkadot_sdk::frame_system::Pallet::<T>::block_number().saturated_into();
            let was_full = SubnetCount::<T>::get() >= MaxSubnets::<T>::get();
            Subnets::<T>::remove(netuid);
            Metadata::<T>::remove(netuid);
            Self::clear_control(netuid);
            SubnetMinerDay::<T>::remove(netuid);
            SubnetMinerHistory::<T>::remove(netuid);
            LastHealthy::<T>::remove(netuid);
            let _ = Neurons::<T>::clear_prefix(netuid, u32::MAX, None);
            let _ = NeuronUid::<T>::clear_prefix(netuid, u32::MAX, None);
            let _ = Endpoints::<T>::clear_prefix(netuid, u32::MAX, None);
            let _ = NeuronModel::<T>::clear_prefix(netuid, u32::MAX, None);
            NeuronCount::<T>::remove(netuid);
            SubnetCount::<T>::mutate(|c| *c = c.saturating_sub(1));
            FullSince::<T>::kill();
            T::OnRemoved::subnet_removed(netuid);
            if was_full {
                Self::slot_freed(now);
            }
            Self::deposit_event(Event::SubnetDeregistered { netuid });
            Ok(())
        }

        /// Called by consensus after each settled epoch of SN1 and later with the number of
        /// miners paid in it: keeps the 30-day history and the last healthy block.
        pub fn note_subnet_epoch(netuid: NetUid, paid_miners: u32) {
            let now: u32 = polkadot_sdk::frame_system::Pallet::<T>::block_number().saturated_into();
            if paid_miners >= DEAD_MIN_MINERS {
                LastHealthy::<T>::insert(netuid, now);
            }
            let today = now / DAY;
            let (day, sum, epochs) = SubnetMinerDay::<T>::get(netuid);
            if day != today && epochs > 0 {
                let avg = (sum / epochs as u64) as u32;
                SubnetMinerHistory::<T>::mutate(netuid, |h| {
                    if h.is_full() {
                        h.remove(0);
                    }
                    let _ = h.try_push(avg);
                });
            }
            if day != today {
                SubnetMinerDay::<T>::insert(netuid, (today, paid_miners as u64, 1));
            } else {
                SubnetMinerDay::<T>::insert(netuid, (today, sum + paid_miners as u64, epochs + 1));
            }
        }

        /// Average paid miners per epoch over the recorded days (at most 30).
        pub fn subnet_miners_average(netuid: NetUid) -> Option<u32> {
            let h = SubnetMinerHistory::<T>::get(netuid);
            if h.is_empty() {
                return None;
            }
            let sum: u64 = h.iter().map(|x| *x as u64).sum();
            Some((sum / h.len() as u64) as u32)
        }

        /// Past immunity, and no epoch in the last 30 days paid at least 3 miners.
        pub fn is_dead(netuid: NetUid) -> bool {
            if netuid == SN0 {
                return false;
            }
            let Some(info) = Subnets::<T>::get(netuid) else {
                return false;
            };
            let now: u32 = polkadot_sdk::frame_system::Pallet::<T>::block_number().saturated_into();
            let created: u32 = info.created.saturated_into();
            let healthy = LastHealthy::<T>::get(netuid)
                .unwrap_or(created)
                .max(created);
            now >= created.saturating_add(IMMUNITY_BLOCKS)
                && now >= healthy.saturating_add(DEAD_AFTER_BLOCKS)
        }

        /// Double the cap if every ladder criterion holds. Returns whether it did.
        pub fn try_raise_cap(now: u32) -> bool {
            let max = MaxSubnets::<T>::get();
            if max >= MAX_SUBNETS_LIMIT || now < CAP_LADDER_FROM {
                return false;
            }
            let count = SubnetCount::<T>::get();
            let full_long = count >= max
                && FullSince::<T>::get()
                    .map_or(false, |f| now >= f.saturating_add(CAP_FULL_BLOCKS));
            let spaced = LastCapRaise::<T>::get()
                .map_or(true, |r| now >= r.saturating_add(CAP_RAISE_SPACING));
            if !full_long || !spaced || count == 0 {
                return false;
            }
            let live = Subnets::<T>::iter_keys()
                .filter(|n| *n != SN0)
                .filter(|n| Self::subnet_miners_average(*n).map_or(false, |a| a >= CAP_LIVE_MINERS))
                .count() as u128;
            if live * (ict_economics::PPM) < count as u128 * CAP_LIVE_PPM as u128 {
                return false;
            }
            let new = max.saturating_mul(2).min(MAX_SUBNETS_LIMIT);
            MaxSubnets::<T>::put(new);
            LastCapRaise::<T>::put(now);
            FullSince::<T>::kill();
            Self::slot_freed(now);
            Self::deposit_event(Event::CapRaised { max: new });
            true
        }

        /// Settings of a subnet (the protocol default when unset).
        pub fn params(netuid: NetUid) -> SubnetParams {
            Params::<T>::get(netuid).unwrap_or(SubnetParams {
                neuron_fee: T::NeuronFee::get(),
                max_neurons: T::MaxNeurons::get(),
                penalties: false,
                max_failures: 3,
                failure_window: DEFAULT_FAILURE_WINDOW,
            })
        }

        fn check_params(p: &SubnetParams) -> DispatchResult {
            ensure!(
                p.neuron_fee >= T::NeuronFee::get()
                    && p.neuron_fee <= MAX_NEURON_FEE
                    && p.max_neurons >= MIN_NEURON_SLOTS.min(T::MaxNeurons::get())
                    && p.max_neurons <= T::MaxNeurons::get()
                    && p.max_failures >= 1
                    && p.max_failures <= MAX_FAILURES_LIMIT
                    && p.failure_window >= MIN_FAILURE_WINDOW
                    && p.failure_window <= MAX_FAILURE_WINDOW,
                Error::<T>::ParamOutOfBounds
            );
            Ok(())
        }

        fn apply_params(netuid: NetUid, params: SubnetParams) -> DispatchResult {
            Self::check_params(&params)?;
            // Fewer slots than neurons already registered would strand them.
            ensure!(
                params.max_neurons >= NeuronCount::<T>::get(netuid),
                Error::<T>::ParamOutOfBounds
            );
            Params::<T>::insert(netuid, params);
            Self::deposit_event(Event::ParamsSet { netuid, params });
            Ok(())
        }

        fn make_metadata(
            name: Vec<u8>,
            description: Vec<u8>,
            code_hash: [u8; 32],
        ) -> Result<SubnetMetadata, DispatchError> {
            ensure!(!name.is_empty(), Error::<T>::EmptyName);
            Ok(SubnetMetadata {
                name: name.try_into().map_err(|_| Error::<T>::NameTooLong)?,
                description: description
                    .try_into()
                    .map_err(|_| Error::<T>::DescriptionTooLong)?,
                code_hash,
            })
        }

        /// The signed caller must be the owner of a subnet that is not run by its DAO.
        fn ensure_controller(origin: OriginFor<T>, netuid: NetUid) -> DispatchResult {
            let who = ensure_signed(origin)?;
            ensure!(netuid != SN0, Error::<T>::Sn0HasNoOwner);
            let info = Subnets::<T>::get(netuid).ok_or(Error::<T>::NoSuchSubnet)?;
            ensure!(
                Control::<T>::get(netuid) != ControlKind::Dao,
                Error::<T>::DaoControlled
            );
            ensure!(info.owner.as_ref() == Some(&who), Error::<T>::NotController);
            Ok(())
        }

        fn set_owner(netuid: NetUid, owner: T::AccountId) {
            Subnets::<T>::mutate(netuid, |i| {
                if let Some(i) = i {
                    i.owner = Some(owner);
                }
            });
        }

        fn carry_out(netuid: NetUid, action: SubnetAction<T::AccountId>) -> DispatchResult {
            match action {
                SubnetAction::SetParams(p) => Self::apply_params(netuid, p),
                SubnetAction::SetMetadata(m) => {
                    Metadata::<T>::insert(netuid, m);
                    Self::deposit_event(Event::MetadataSet { netuid });
                    Ok(())
                }
                SubnetAction::Spend { to, amount } => {
                    T::Currency::transfer(
                        &Self::dao_account(netuid),
                        &to,
                        amount,
                        Preservation::Expendable,
                    )?;
                    Self::deposit_event(Event::DaoSpent { netuid, to, amount });
                    Ok(())
                }
            }
        }

        fn clear_control(netuid: NetUid) {
            Control::<T>::remove(netuid);
            MultisigOf::<T>::remove(netuid);
            Params::<T>::remove(netuid);
            Proposals::<T>::remove(netuid);
            let _ = Votes::<T>::clear_prefix(netuid, u32::MAX, None);
            let _ = WorkReporters::<T>::clear_prefix((netuid,), u32::MAX, None);
            let _ = WorkPenalized::<T>::clear_prefix((netuid,), u32::MAX, None);
            let _ = WorkFailures::<T>::clear_prefix(netuid, u32::MAX, None);
            let _ = SubnetBanned::<T>::clear_prefix(netuid, u32::MAX, None);
        }

        /// The subnet's DAO account: receives the owner share once the subnet is run by its
        /// DAO; only DAO votes spend from it.
        pub fn dao_account(netuid: NetUid) -> T::AccountId {
            T::PalletId::get().into_sub_account_truncating((b"dao", netuid))
        }

        /// Same derivation as `pallet_multisig::Pallet::multi_account_id` (sorted signatories).
        pub fn multi_account_id(signatories: &[T::AccountId], threshold: u16) -> T::AccountId {
            use polkadot_sdk::sp_runtime::traits::{BlakeTwo256, Hash};
            // blake2_256 of the encoding, as pallet-multisig computes it.
            let entropy = (b"modlpy/utilisuba", signatories, threshold)
                .using_encoded(|b| BlakeTwo256::hash(b));
            Decode::decode(&mut TrailingZeroInput::new(entropy.as_ref()))
                .expect("infinite length input; no invalid inputs for type; qed")
        }

        /// Miner `uid` was penalised in subnet epoch `epoch` (SN1 and later).
        pub fn is_work_penalized(netuid: NetUid, epoch: u64, uid: Uid) -> bool {
            WorkPenalized::<T>::contains_key((netuid, epoch, uid))
        }

        pub fn is_banned_from(netuid: NetUid, who: &T::AccountId) -> bool {
            SubnetBanned::<T>::contains_key(netuid, who)
        }

        fn next_free_netuid() -> Option<NetUid> {
            (1..=NetUid::MAX).find(|n| !Subnets::<T>::contains_key(n))
        }
    }
}

/// Runtime API behind incentium.org's subnet page: the unlock criteria, the auction and the
/// subnets, computed by the chain itself so the page never shows its own arithmetic.
pub mod runtime_api {
    use super::pallet::{AuctionInfo, SubnetSummary, UnlockStatus};
    extern crate alloc;
    use alloc::vec::Vec;
    use codec::Codec;

    polkadot_sdk::sp_api::decl_runtime_apis! {
        pub trait SubnetRegistryApi<AccountId> where AccountId: Codec {
            /// Where each SN1 unlock criterion stands.
            fn unlock_status() -> UnlockStatus;
            /// The slot on sale (or the next one).
            fn auction() -> AuctionInfo;
            /// Price of the slot on sale at a future block, if nothing changes before then.
            fn price_at(block: u32) -> Option<u128>;
            /// Every subnet, SN0 first.
            fn subnets() -> Vec<SubnetSummary<AccountId>>;
        }
    }
}
