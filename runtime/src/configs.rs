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
    RuntimeTask, System, EPOCH_LENGTH, EXISTENTIAL_DEPOSIT, SLOT_DURATION, UNIT, VERSION,
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

    pub const DepositFloor: Balance = 1_000 * UNIT;
    pub const DecayBlocks: u32 = 14 * super::DAYS;
    pub const ReturnPpm: u32 = 900_000;
    pub const NeuronFee: Balance = UNIT;
    pub const MaxNeurons: u16 = 256;

    pub const EpochLength: u32 = EPOCH_LENGTH;
    pub const MaxWeights: u32 = 256;

    pub const PoolBond: Balance = 5_000 * UNIT;
    pub const MaxPoolFeePpm: u32 = 50_000;
    /// 7 days in epochs (20 epochs per day).
    pub const FeeChangeDelay: u64 = 140;
    pub const SlashPpm: u32 = 100_000;
    pub const QuorumPpm: u32 = 333_334;
    pub const MaxFailures: u32 = 3;
    /// 30 days in epochs.
    pub const FailureWindow: u64 = 600;
    pub const UnbondingEpochs: u64 = 140;
    pub const MaxReporters: u32 = 64;
    pub const MaxPayees: u32 = 1_024;

    /// Phase 1: miners 60 % and validators 30 % of issuance; the pot holds the 90 %.
    pub const MinerSharePpm: u32 = 666_667;
    pub const KappaPpm: u32 = 500_000;
    pub const MaxValidatorPpm: u32 = 100_000;
    pub const MinValidatorStake: Balance = 1_000 * UNIT;

    /// Launch-period rewards: locked 2 months after the period, then 10 % every 2 months.
    pub const LaunchCliff: u64 = 60 * super::DAYS as u64;
    pub const LaunchStep: u64 = 60 * super::DAYS as u64;
    pub const LaunchStepPpm: u32 = 100_000;
}

impl pallet_ict_emission::Config for Runtime {
    type RuntimeEvent = RuntimeEvent;
    type WeightInfo = ();
    type Currency = Balances;
    type FundAccount = FundAccount;
    type SubnetPot = SubnetPot;
}

impl pallet_subnet_registry::Config for Runtime {
    type RuntimeEvent = RuntimeEvent;
    type WeightInfo = ();
    type Currency = Balances;
    type PalletId = RegistryPalletId;
    type DepositFloor = DepositFloor;
    type DecayBlocks = DecayBlocks;
    type ReturnPpm = ReturnPpm;
    type NeuronFee = NeuronFee;
    type MaxNeurons = MaxNeurons;
}

impl pallet_weights::Config for Runtime {
    type RuntimeEvent = RuntimeEvent;
    type WeightInfo = ();
    type EpochLength = EpochLength;
    type MaxWeights = MaxWeights;
}

impl pallet_work_verify::Config for Runtime {
    type RuntimeEvent = RuntimeEvent;
    type WeightInfo = ();
    type WorkCurrency = Balances;
    type Stake = Consensus;
    type WorkPalletId = WorkPalletId;
    type InsuranceFund = InsuranceFund;
    type PoolBond = PoolBond;
    type MaxPoolFeePpm = MaxPoolFeePpm;
    type FeeChangeDelay = FeeChangeDelay;
    type SlashPpm = SlashPpm;
    type QuorumPpm = QuorumPpm;
    type MaxFailures = MaxFailures;
    type FailureWindow = FailureWindow;
    type UnbondingEpochs = UnbondingEpochs;
    type MaxReporters = MaxReporters;
    type MaxPayees = MaxPayees;
}

impl pallet_ict_consensus::Config for Runtime {
    type RuntimeEvent = RuntimeEvent;
    type WeightInfo = ();
    type StakeCurrency = Balances;
    type StakePalletId = StakePalletId;
    type SubnetPot = SubnetPot;
    type MinerSharePpm = MinerSharePpm;
    type KappaPpm = KappaPpm;
    type MaxValidatorPpm = MaxValidatorPpm;
    type MinValidatorStake = MinValidatorStake;
    type StakeUnbondingEpochs = UnbondingEpochs;
    type LaunchPalletId = LaunchPalletId;
    type LaunchCliff = LaunchCliff;
    type LaunchStep = LaunchStep;
    type LaunchStepPpm = LaunchStepPpm;
}
