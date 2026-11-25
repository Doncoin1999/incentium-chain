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
    AccountId, Address, Aura, Balance, Balances, Block, BlockNumber, Consensus, EthExtraImpl, Hash,
    Nonce, PalletInfo, Runtime, RuntimeCall, RuntimeEvent, RuntimeFreezeReason, RuntimeHoldReason,
    RuntimeOrigin, RuntimeTask, Signature, System, Timestamp, EPOCH_LENGTH, EXISTENTIAL_DEPOSIT,
    SLOT_DURATION, UNIT, VERSION,
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
    type DisabledValidators = crate::Session;
    type MaxAuthorities = ConstU32<128>;
    type AllowMultipleBlocksPerSlot = ConstBool<false>;
    type SlotDuration = pallet_aura::MinimumPeriodTimesTwo<Runtime>;
}

impl pallet_grandpa::Config for Runtime {
    type RuntimeEvent = RuntimeEvent;
    type WeightInfo = ();
    type MaxAuthorities = ConstU32<128>;
    type MaxNominators = crate::block_production::MaxBackersPerWinner;
    /// Sessions in the bonding period: equivocations can be reported that far back.
    type MaxSetIdSessionEntries = ConstU64<28>;
    type KeyOwnerProof = sp_session::MembershipProof;
    type EquivocationReportSystem = pallet_grandpa::EquivocationReportSystem<
        Self,
        crate::Offences,
        crate::Historical,
        crate::block_production::ReportLongevity,
    >;
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

/// Fee = ref_time / `FEE_DIVISOR`, as before. pallet-revive only accepts its own
/// `BlockRatioFee` because its gas mapping depends on it. The type also adds a proof-size
/// component, but with this solochain's unlimited proof size that part is negligible, so
/// fees for ordinary transactions are unchanged.
pub type IctWeightToFee = pallet_revive::evm::fees::BlockRatioFee<1, FEE_DIVISOR, Runtime, Balance>;

impl pallet_transaction_payment::Config for Runtime {
    type RuntimeEvent = RuntimeEvent;
    /// Fees go to the block producers' reward pot (until runtime 12 they were burned).
    type OnChargeTransaction = FungibleAdapter<Balances, crate::block_production::FeesToPot>;
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
    type ValidatorPot = crate::block_production::ValidatorPot;
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
}

// ---------------------------------------------------------------------------------------------
// EVM: pallet-revive with the REVM backend (spec Section 14).
//
// Runs unmodified Solidity/EVM bytecode, so Hyperlane's standard contracts can be deployed with
// Hyperlane's own tooling. Ethereum JSON-RPC (MetaMask, Foundry, Hyperlane agents) is served by
// the separate `eth-rpc` server (crate `pallet-revive-eth-rpc`) next to a node; the node binary
// itself needs no changes.
// ---------------------------------------------------------------------------------------------

/// EIP-155 chain ID of the Incentium EVM. Check that it is free on chainid.network before
/// mainnet: it cannot be changed once wallets and contracts rely on it.
pub const EVM_CHAIN_ID: u64 = 20_261_118;

parameter_types! {
    /// Storage deposits for contracts, scaled to ICT (9 decimals). The deposit is refunded when
    /// the storage is freed, and the values can be changed on a live chain (refunds are pro rata).
    pub const ReviveDepositPerItem: Balance = UNIT / 100; // 0.01 ICT per storage item
    pub const ReviveDepositPerChildTrieItem: Balance = UNIT / 10_000; // 0.0001 ICT per contract slot
    pub const ReviveDepositPerByte: Balance = UNIT / 100_000; // 0.00001 ICT per byte
    pub CodeHashLockupDepositPercent: Perbill = Perbill::from_percent(30);
}

#[derive_impl(pallet_revive::config_preludes::TestDefaultConfig)]
impl pallet_revive::Config for Runtime {
    type Time = Timestamp;
    type Balance = Balance;
    type Currency = Balances;
    /// 32-byte Substrate accounts keep working. MetaMask accounts get a native account
    /// automatically; a Talisman (sr25519) account calls `map_account` once to get an EVM address.
    type AddressMapper = pallet_revive::AccountId32Mapper<Self>;
    type ChainId = ConstU64<EVM_CHAIN_ID>;
    /// ICT has 9 decimals, the EVM shows 18: 1 ICT = 10^9 base units = 10^18 wei.
    type NativeToEthRatio = ConstU32<1_000_000_000>;
    type FeeInfo = pallet_revive::evm::fees::Info<Address, Signature, EthExtraImpl>;
    type DepositPerItem = ReviveDepositPerItem;
    type DepositPerChildTrieItem = ReviveDepositPerChildTrieItem;
    type DepositPerByte = ReviveDepositPerByte;
    type CodeHashLockupDepositPercent = CodeHashLockupDepositPercent;
    type AllowEVMBytecode = ConstBool<true>;
    /// Anyone may deploy contracts. To restrict deployment during the launch period, replace
    /// these with an origin that only admits approved accounts.
    type UploadOrigin = frame_system::EnsureSigned<AccountId>;
    type InstantiateOrigin = frame_system::EnsureSigned<AccountId>;
    type DebugEnabled = ConstBool<false>;
    /// Existing accounts would not be mapped retroactively, so mapping stays explicit.
    type AutoMap = ConstBool<false>;
    /// Scales gas so that common operations cost roughly what Ethereum tooling expects
    /// (a plain transfer in the order of 20,000 gas). Does not change what a transaction costs.
    type GasScale = ConstU32<100>;
    /// Parity's reference weights; re-benchmark on the mainnet hardware before mainnet.
    type WeightInfo = pallet_revive::weights::SubstrateWeight<Self>;
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
    /// 0.5 % per trade, half burned (proposed).
    pub const PoolFeePpm: u32 = 5_000;
    pub const PoolFeeBurnPpm: u32 = 500_000;
    /// Smallest stake or unstake: 0.01 ICT or Alpha.
    pub const PoolMinTrade: u128 = UNIT / 100;
}

impl pallet_subnet_pools::Config for Runtime {
    type RuntimeEvent = RuntimeEvent;
    type Currency = Balances;
    type PalletId = SubnetPoolsPalletId;
    type Subnets = RegistrySubnets;
    type FeePpm = PoolFeePpm;
    type FeeBurnPpm = PoolFeeBurnPpm;
    type MinTrade = PoolMinTrade;
}
