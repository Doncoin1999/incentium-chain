use polkadot_sdk::*;

use frame_support::{
    derive_impl,
    dispatch::DispatchClass,
    parameter_types,
    traits::{ConstBool, ConstU128, ConstU32, ConstU64, ConstU8, VariantCountOf},
    weights::{
        constants::{RocksDbWeight, WEIGHT_REF_TIME_PER_SECOND},
        IdentityFee, Weight,
    },
    PalletId,
};
use frame_system::limits::{BlockLength, BlockWeights};
use pallet_transaction_payment::{ConstFeeMultiplier, FungibleAdapter, Multiplier};
use sp_consensus_aura::sr25519::AuthorityId as AuraId;
use sp_runtime::{
    traits::{AccountIdConversion, One},
    Perbill,
};
use sp_version::RuntimeVersion;

use super::{
    AccountId, Aura, Balance, Balances, Block, BlockNumber, Consensus, Hash, Nonce, PalletInfo,
    Runtime, RuntimeCall, RuntimeEvent, RuntimeFreezeReason, RuntimeHoldReason, RuntimeOrigin,
    RuntimeTask, System, Timestamp, EPOCH_LENGTH, EXISTENTIAL_DEPOSIT, SLOT_DURATION, UNIT,
    VERSION,
};

const NORMAL_DISPATCH_RATIO: Perbill = Perbill::from_percent(75);
const MAX_BLOCK_LENGTH: u32 = 5 * 1024 * 1024;

parameter_types! {
    pub const BlockHashCount: BlockNumber = 2400;
    pub const Version: RuntimeVersion = VERSION;
    /// 2 seconds of compute per 12-second block.
    pub RuntimeBlockWeights: BlockWeights = BlockWeights::with_sensible_defaults(
        Weight::from_parts(2u64 * WEIGHT_REF_TIME_PER_SECOND, u64::MAX),
        NORMAL_DISPATCH_RATIO,
    );
    /// 5 MiB per block; normal transactions may use 75 % of it.
    pub RuntimeBlockLength: BlockLength = BlockLength::builder()
        .max_length(MAX_BLOCK_LENGTH)
        .modify_max_length_for_class(DispatchClass::Normal, |m| *m = NORMAL_DISPATCH_RATIO * *m)
        .build();
    /// Generic Substrate address format: addresses start with `5`.
    pub const SS58Prefix: u16 = 42;
}

#[derive_impl(frame_system::config_preludes::SolochainDefaultConfig)]
impl frame_system::Config for Runtime {
    type Block = Block;
    type BlockWeights = RuntimeBlockWeights;
    type BlockLength = RuntimeBlockLength;
    type AccountId = AccountId;
    type Nonce = Nonce;
    type Hash = Hash;
    type BlockHashCount = BlockHashCount;
    type DbWeight = RocksDbWeight;
    type Version = Version;
    type AccountData = pallet_balances::AccountData<Balance>;
    type SS58Prefix = SS58Prefix;
    type MaxConsumers = frame_support::traits::ConstU32<16>;
}

impl pallet_aura::Config for Runtime {
    type AuthorityId = AuraId;
    type DisabledValidators = ();
    type MaxAuthorities = ConstU32<32>;
    type AllowMultipleBlocksPerSlot = ConstBool<false>;
    type SlotDuration = pallet_aura::MinimumPeriodTimesTwo<Runtime>;
}

impl pallet_grandpa::Config for Runtime {
    type RuntimeEvent = RuntimeEvent;
    type WeightInfo = ();
    type MaxAuthorities = ConstU32<32>;
    type MaxNominators = ConstU32<0>;
    type MaxSetIdSessionEntries = ConstU64<0>;
    type KeyOwnerProof = sp_core::Void;
    type EquivocationReportSystem = ();
}

impl pallet_timestamp::Config for Runtime {
    type Moment = u64;
    type OnTimestampSet = Aura;
    type MinimumPeriod = ConstU64<{ SLOT_DURATION / 2 }>;
    type WeightInfo = ();
}

impl pallet_balances::Config for Runtime {
    type MaxLocks = ConstU32<50>;
    type MaxReserves = ();
    type ReserveIdentifier = [u8; 8];
    type Balance = Balance;
    type RuntimeEvent = RuntimeEvent;
    type DustRemoval = ();
    type ExistentialDeposit = ConstU128<EXISTENTIAL_DEPOSIT>;
    type AccountStore = System;
    type WeightInfo = pallet_balances::weights::SubstrateWeight<Runtime>;
    type FreezeIdentifier = RuntimeFreezeReason;
    type MaxFreezes = VariantCountOf<RuntimeFreezeReason>;
    type RuntimeHoldReason = RuntimeHoldReason;
    type RuntimeFreezeReason = RuntimeFreezeReason;
    type DoneSlashHandler = ();
}

parameter_types! {
    pub FeeMultiplier: Multiplier = Multiplier::one();
}

/// One ICT has 9 decimals, so the template's 1-to-1 mapping of weight (picoseconds) to the
/// smallest unit made a simple call cost around 0.1–1 ICT. Dividing by `FEE_DIVISOR` puts an
/// ordinary transfer at roughly a thousandth of an ICT; the first weight commit per epoch and
/// every successful reveal are free anyway (see `pallet-weights`).
pub const FEE_DIVISOR: Balance = 100;

pub struct IctWeightToFee;
impl frame_support::weights::WeightToFee for IctWeightToFee {
    type Balance = Balance;
    fn weight_to_fee(weight: &Weight) -> Self::Balance {
        (weight.ref_time() as Balance) / FEE_DIVISOR
    }
}

impl pallet_transaction_payment::Config for Runtime {
    type RuntimeEvent = RuntimeEvent;
    type OnChargeTransaction = FungibleAdapter<Balances, ()>;
    type OperationalFeeMultiplier = ConstU8<5>;
    type WeightToFee = IctWeightToFee;
    type LengthToFee = IdentityFee<Balance>;
    type FeeMultiplierUpdate = ConstFeeMultiplier<FeeMultiplier>;
    type WeightInfo = pallet_transaction_payment::weights::SubstrateWeight<Runtime>;
}

impl pallet_sudo::Config for Runtime {
    type RuntimeEvent = RuntimeEvent;
    type RuntimeCall = RuntimeCall;
    type WeightInfo = pallet_sudo::weights::SubstrateWeight<Runtime>;
}

// ---------------------------------------------------------------- Incentium pallets

parameter_types! {
    pub const FundPalletId: PalletId = PalletId(*b"ict/fund");
    pub const PotPalletId: PalletId = PalletId(*b"ict/spot");
    pub const InsurancePalletId: PalletId = PalletId(*b"ict/insu");
    pub const RegistryPalletId: PalletId = PalletId(*b"ict/regi");
    pub const WorkPalletId: PalletId = PalletId(*b"ict/work");
    pub const StakePalletId: PalletId = PalletId(*b"ict/stak");
    pub const LaunchPalletId: PalletId = PalletId(*b"ict/lnch");

    /// On-chain development fund (treasury), spent only by governance.
    pub FundAccount: AccountId = FundPalletId::get().into_account_truncating();
    /// Subnet issuance waiting for consensus payout.
    pub SubnetPot: AccountId = PotPalletId::get().into_account_truncating();
    pub InsuranceFund: AccountId = InsurancePalletId::get().into_account_truncating();

    pub const NeuronFee: Balance = UNIT;
    pub const MaxNeurons: u16 = 256;

    pub const EpochLength: u32 = EPOCH_LENGTH;
    pub const MaxWeights: u32 = 256;

    pub const PoolBond: Balance = 5_000 * UNIT;
    pub const MaxPoolFeePpm: u32 = 50_000;
    /// 7 days in epochs (20 epochs per day).
    pub const FeeChangeDelay: u64 = 140;
    pub const SlashPpm: u32 = 100_000;
    /// Lost payout: half of the active validator stake; slash: 2/3.
    pub const QuorumPpm: u32 = 500_000;
    pub const SlashQuorumPpm: u32 = 666_667;
    pub const MaxFailures: u32 = 3;
    /// 30 days in epochs.
    pub const FailureWindow: u64 = 600;
    pub const UnbondingEpochs: u64 = 140;
    pub const MaxReporters: u32 = 64;
    pub const MaxPayees: u32 = 1_024;

    /// Phase 1: miners 60 % and validators 30 % of issuance; the pot holds the 90 %.
    pub const MinerSharePpm: u32 = 666_667;
    /// After the SN1 unlock SN0 has no owner share and splits 50 : 38 (miners 50 / 88).
    pub const Sn0MinerSharePpm: u32 = 568_182;
    pub const KappaPpm: u32 = 500_000;
    pub const MaxValidatorPpm: u32 = 100_000;
    pub const MinValidatorStake: Balance = 1_000 * UNIT;

    /// Locked launch-period rewards: first 10 % two months after the period (day 150), then
    /// 10 % every 2 months.
    pub const LaunchCliff: u64 = 60 * super::DAYS as u64;
    pub const LaunchStep: u64 = 60 * super::DAYS as u64;
    pub const LaunchStepPpm: u32 = 100_000;
    /// Launch-period rewards: 25 % paid at once, 75 % locked in the launch vault.
    pub const LaunchImmediatePpm: u32 = 250_000;
}

impl pallet_ict_emission::Config for Runtime {
    type RuntimeEvent = RuntimeEvent;
    type WeightInfo = crate::weights::pallet_ict_emission::WeightInfo<Runtime>;
    type Currency = Balances;
    type FundAccount = FundAccount;
    type SubnetPot = SubnetPot;
}

impl pallet_subnet_registry::Config for Runtime {
    type RuntimeEvent = RuntimeEvent;
    type WeightInfo = crate::weights::pallet_subnet_registry::WeightInfo<Runtime>;
    type Currency = Balances;
    type PalletId = RegistryPalletId;
    type TreasuryAccount = FundAccount;
    type Unlock = crate::governance::GovernanceSignals;
    type StakeInfo = Consensus;
    type OnRemoved = Consensus;
    type Sale = AlphaPools;
    type NeuronFee = NeuronFee;
    type MaxNeurons = MaxNeurons;
}

impl pallet_weights::Config for Runtime {
    type RuntimeEvent = RuntimeEvent;
    type WeightInfo = crate::weights::pallet_weights::WeightInfo<Runtime>;
    type EpochLength = EpochLength;
    type MaxWeights = MaxWeights;
}

impl pallet_work_verify::Config for Runtime {
    type RuntimeEvent = RuntimeEvent;
    type WeightInfo = crate::weights::pallet_work_verify::WeightInfo<Runtime>;
    type WorkCurrency = Balances;
    type Stake = Consensus;
    type WorkPalletId = WorkPalletId;
    type InsuranceFund = InsuranceFund;
    type PoolBond = PoolBond;
    type MaxPoolFeePpm = MaxPoolFeePpm;
    type FeeChangeDelay = FeeChangeDelay;
    type SlashPpm = SlashPpm;
    type QuorumPpm = QuorumPpm;
    type SlashQuorumPpm = SlashQuorumPpm;
    type MaxFailures = MaxFailures;
    type FailureWindow = FailureWindow;
    type UnbondingEpochs = UnbondingEpochs;
    type MaxReporters = MaxReporters;
    type MaxPayees = MaxPayees;
}

/// SN0 keeps at least 50 % of all issuance. The development fund takes its share before the
/// pot, so as a share of the pot that is 0.5 / (1 - fund share): 555,556 ppm at 10 %.
pub struct Sn0MinPotPpm;
impl frame_support::traits::Get<u32> for Sn0MinPotPpm {
    fn get() -> u32 {
        const SN0_MIN_OF_ISSUANCE: u64 = 500_000;
        let fund = pallet_ict_emission::FundShare::<Runtime>::get() as u64;
        let pot = 1_000_000u64.saturating_sub(fund).max(1);
        (SN0_MIN_OF_ISSUANCE * 1_000_000)
            .div_ceil(pot)
            .min(1_000_000) as u32
    }
}

impl pallet_ict_consensus::Config for Runtime {
    type RuntimeEvent = RuntimeEvent;
    type WeightInfo = crate::weights::pallet_ict_consensus::WeightInfo<Runtime>;
    type StakeCurrency = Balances;
    type Alpha = AlphaPools;
    type StakePalletId = StakePalletId;
    type SubnetPot = SubnetPot;
    type MinerSharePpm = MinerSharePpm;
    type Sn0MinerSharePpm = Sn0MinerSharePpm;
    type Sn0MinPotPpm = Sn0MinPotPpm;
    type KappaPpm = KappaPpm;
    type MaxValidatorPpm = MaxValidatorPpm;
    type MinValidatorStake = MinValidatorStake;
    type StakeUnbondingEpochs = UnbondingEpochs;
    type LaunchPalletId = LaunchPalletId;
    type LaunchCliff = LaunchCliff;
    type LaunchStep = LaunchStep;
    type LaunchStepPpm = LaunchStepPpm;
    type LaunchImmediatePpm = LaunchImmediatePpm;
}

parameter_types! {
    pub const SwapBlocksPerDay: u32 = super::DAYS;
    /// 5 USDC (6 decimals): smallest fill.
    pub const SwapMinFillUsdc: u128 = 5_000_000;
    /// Fills in steps of 0.01 ICT.
    pub const SwapQtyStep: Balance = UNIT / 100;
    pub const SwapMaxOpenReservations: u8 = 3;
    pub const SwapOrderTtlMax: u64 = 24 * 3600;
    /// 1 h for the taker to lock + 12 h for the maker to confirm + 6 h claim margin.
    pub const SwapReservationTtl: u64 = 19 * 3600;
    pub const SwapLockTtlMax: u64 = 12 * 3600;
    /// Reservation deposit: 2 % of the quantity, at least 2 ICT (review fix H2).
    pub const SwapReservationDepositPpm: u32 = 20_000;
    pub const SwapMinReservationDeposit: Balance = 2 * UNIT;
    /// The deposit arbiter may return an unclaimed reservation's deposit for 24 h.
    pub const SwapDepositDisputeWindow: u64 = 24 * 3600;
    /// Gaps between blocks above 2 minutes count as downtime (review fix H1).
    pub const SwapMaxBlockGap: u64 = 120;
}

/// ICT <-> USDC atomic swaps with Base (Incentium Swap Technical Specification v0.1).
impl pallet_ict_swap::Config for Runtime {
    type RuntimeEvent = RuntimeEvent;
    type WeightInfo = ();
    type RuntimeHoldReason = RuntimeHoldReason;
    type Currency = Balances;
    type UnixTime = Timestamp;
    type BlocksPerDay = SwapBlocksPerDay;
    type MinFillUsdc = SwapMinFillUsdc;
    type QtyStep = SwapQtyStep;
    type MaxOpenReservations = SwapMaxOpenReservations;
    type OrderTtlMax = SwapOrderTtlMax;
    type ReservationTtl = SwapReservationTtl;
    type LockTtlMax = SwapLockTtlMax;
    type ReservationDepositPpm = SwapReservationDepositPpm;
    type MinReservationDeposit = SwapMinReservationDeposit;
    type DepositDisputeWindow = SwapDepositDisputeWindow;
    type MaxBlockGap = SwapMaxBlockGap;
}

// ------------------------------------------------------------------ subnet pools

/// What the registry tells the pools about subnets.
pub struct RegistrySubnets;
impl pallet_subnet_pools::SubnetInfo<AccountId> for RegistrySubnets {
    fn exists(netuid: u16) -> bool {
        netuid != pallet_subnet_registry::SN0 && pallet_subnet_registry::Subnets::<Runtime>::contains_key(netuid)
    }
    fn owner(netuid: u16) -> Option<AccountId> {
        if !Self::exists(netuid) {
            return None;
        }
        match pallet_subnet_registry::Control::<Runtime>::get(netuid) {
            pallet_subnet_registry::ControlKind::Dao => Some(crate::Registry::dao_account(netuid)),
            _ => pallet_subnet_registry::Subnets::<Runtime>::get(netuid).and_then(|s| s.owner),
        }
    }
    fn is_neuron(netuid: u16, hotkey: &AccountId) -> bool {
        Self::exists(netuid) && pallet_subnet_registry::NeuronUid::<Runtime>::contains_key(netuid, hotkey)
    }
    fn name(netuid: u16) -> alloc::vec::Vec<u8> {
        pallet_subnet_registry::Metadata::<Runtime>::get(netuid)
            .map(|m| m.name.into_inner())
            .unwrap_or_default()
    }
}

parameter_types! {
    pub const SubnetPoolsPalletId: PalletId = PalletId(*b"ict/alph");
    /// 0.5 % per trade, half burned (decided 26 Nov 2026).
    pub const PoolFeePpm: u32 = 5_000;
    pub const PoolFeeBurnPpm: u32 = 500_000;
    /// Smallest stake or unstake: 0.01 ICT or Alpha.
    pub const PoolMinTrade: u128 = UNIT / 100;
}

/// What the start-price formula needs: the ICT a day the average subnet other than SN0
/// receives (all issuance less the fund, less SN0's floor of half of all issuance, shared
/// by the subnets) and the days until ICT's next halving.
pub struct PoolIssuance;
impl pallet_subnet_pools::IssuanceInfo for PoolIssuance {
    fn subnet_ict_per_day() -> u128 {
        let halvings = pallet_ict_emission::Halvings::<Runtime>::get();
        let per_day = ict_economics::emission::block_reward(halvings) * ict_economics::BLOCKS_PER_DAY as u128;
        let fund = pallet_ict_emission::FundShare::<Runtime>::get() as u128;
        let others_ppm = 1_000_000u128.saturating_sub(fund).saturating_sub(500_000);
        let n = (pallet_subnet_registry::SubnetCount::<Runtime>::get() as u128).max(1);
        per_day * others_ppm / 1_000_000 / n
    }
    fn days_to_ict_halving() -> u64 {
        let halvings = pallet_ict_emission::Halvings::<Runtime>::get();
        let left = ict_economics::emission::next_threshold(halvings)
            .saturating_sub(pallet_ict_emission::TotalIssued::<Runtime>::get());
        let per_day = (ict_economics::emission::block_reward(halvings) * ict_economics::BLOCKS_PER_DAY as u128).max(1);
        (left / per_day) as u64
    }
}

/// The Alpha pools, as the registry and consensus see them (decided 26 Nov 2026).
pub struct AlphaPools;
type Pools = crate::SubnetPools;
use pallet_subnet_pools::PoolsInterface as _;

impl pallet_subnet_registry::SubnetSale<AccountId> for AlphaPools {
    fn pools_installed() -> bool {
        true
    }
    fn subnet_sold(netuid: u16, buyer: &AccountId, amount: u128) -> sp_runtime::DispatchResult {
        Pools::register_pool(netuid, buyer, amount)
    }
}

impl pallet_ict_consensus::AlphaHooks<AccountId> for AlphaPools {
    fn active() -> bool {
        true
    }
    fn stake_ict(netuid: u16) -> u128 {
        Pools::stake_ict(netuid)
    }
    fn alpha_stake(netuid: u16, hotkey: &AccountId) -> u128 {
        Pools::alpha_stake(netuid, hotkey)
    }
    fn total_alpha(netuid: u16) -> u128 {
        Pools::total_alpha_stake(netuid)
    }
    fn staker_alpha(netuid: u16, who: &AccountId) -> u128 {
        Pools::staker_alpha(netuid, who)
    }
    fn on_epoch(netuid: u16, blocks: u32, miners: &[(AccountId, u128)], validators: &[(AccountId, u128)]) {
        Pools::on_epoch(netuid, blocks, miners, validators)
    }
    fn pay_validator(netuid: u16, source: &AccountId, hotkey: &AccountId, amount: u128) -> bool {
        Pools::pay_validator_ict(netuid, source, hotkey, amount).is_ok()
    }
    fn subnet_removed(netuid: u16) {
        Pools::close_pool(netuid)
    }
}

impl pallet_subnet_pools::Config for Runtime {
    type RuntimeEvent = RuntimeEvent;
    type Currency = Balances;
    type PalletId = SubnetPoolsPalletId;
    type Subnets = RegistrySubnets;
    type Issuance = PoolIssuance;
    type Treasury = FundAccount;
    type FeePpm = PoolFeePpm;
    type FeeBurnPpm = PoolFeeBurnPpm;
    type MinTrade = PoolMinTrade;
}
