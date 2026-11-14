use crate as pallet_ipm_consensus;
use polkadot_sdk::frame_support::{derive_impl, parameter_types, PalletId};
use polkadot_sdk::sp_runtime::BuildStorage;
use polkadot_sdk::{frame_system, pallet_balances, sp_io};

type Block = frame_system::mocking::MockBlock<Test>;

polkadot_sdk::frame_support::construct_runtime!(
    pub enum Test {
        System: frame_system,
        Balances: pallet_balances,
        Registry: pallet_subnet_registry,
        Weights: pallet_weights,
        Work: pallet_work_verify,
        Consensus: pallet_ipm_consensus,
    }
);

#[derive_impl(frame_system::config_preludes::TestDefaultConfig)]
impl frame_system::Config for Test {
    type Block = Block;
    type AccountData = pallet_balances::AccountData<u128>;
}

#[derive_impl(pallet_balances::config_preludes::TestDefaultConfig)]
impl pallet_balances::Config for Test {
    type Balance = u128;
    type AccountStore = System;
    type ExistentialDeposit = polkadot_sdk::frame_support::traits::ConstU128<1>;
}

pub const POT: u64 = 2_000;

parameter_types! {
    pub const RegistryPalletId: PalletId = PalletId(*b"regipall");
    pub const WorkPalletId: PalletId = PalletId(*b"workpall");
    pub const StakePalletId: PalletId = PalletId(*b"stakpall");
    pub const LaunchPalletId: PalletId = PalletId(*b"lnchpall");
    pub const DepositFloor: u128 = 1_000;
    pub const DecayBlocks: u32 = 100;
    pub const ReturnPpm: u32 = 900_000;
    pub const NeuronFee: u128 = 1;
    pub const MaxNeurons: u16 = 16;
    pub const EpochLength: u32 = 10;
    pub const MaxWeights: u32 = 16;
    pub const InsuranceFund: u64 = 999;
    pub const PoolBond: u128 = 5_000;
    pub const MaxPoolFeePpm: u32 = 50_000;
    pub const FeeChangeDelay: u64 = 2;
    pub const SlashPpm: u32 = 100_000;
    pub const QuorumPpm: u32 = 333_334;
    pub const MaxFailures: u32 = 3;
    pub const FailureWindow: u64 = 600;
    pub const UnbondingEpochs: u64 = 3;
    pub const MaxReporters: u32 = 16;
    pub const MaxPayees: u32 = 64;
    pub const SubnetPot: u64 = POT;
    pub const MinerSharePpm: u32 = 666_667;
    pub const KappaPpm: u32 = 500_000;
    pub const MaxValidatorPpm: u32 = 100_000;
    pub const MinValidatorStake: u128 = 100;
    pub const LaunchCliff: u64 = 20;
    pub const LaunchStep: u64 = 20;
    pub const LaunchStepPpm: u32 = 100_000;
}

impl pallet_subnet_registry::Config for Test {
    type RuntimeEvent = RuntimeEvent;
    type Currency = Balances;
    type PalletId = RegistryPalletId;
    type DepositFloor = DepositFloor;
    type DecayBlocks = DecayBlocks;
    type ReturnPpm = ReturnPpm;
    type NeuronFee = NeuronFee;
    type MaxNeurons = MaxNeurons;
}

impl pallet_weights::Config for Test {
    type RuntimeEvent = RuntimeEvent;
    type EpochLength = EpochLength;
    type MaxWeights = MaxWeights;
}

impl pallet_work_verify::Config for Test {
    type RuntimeEvent = RuntimeEvent;
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

impl pallet_ipm_consensus::Config for Test {
    type RuntimeEvent = RuntimeEvent;
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

pub fn new_test_ext() -> sp_io::TestExternalities {
    let mut t = frame_system::GenesisConfig::<Test>::default()
        .build_storage()
        .unwrap();
    pallet_balances::GenesisConfig::<Test> {
        balances: (1..=10).map(|a| (a, 1_000_000)).collect(),
        ..Default::default()
    }
    .assimilate_storage(&mut t)
    .unwrap();
    pallet_subnet_registry::GenesisConfig::<Test>::default()
        .assimilate_storage(&mut t)
        .unwrap();
    let mut ext = sp_io::TestExternalities::new(t);
    ext.execute_with(|| System::set_block_number(1));
    ext
}

pub const OPERATOR: u64 = 7;

/// Launch period of 25 blocks (epochs 0-2 start inside it) run by OPERATOR.
pub fn new_launch_ext() -> sp_io::TestExternalities {
    let mut t = frame_system::GenesisConfig::<Test>::default()
        .build_storage()
        .unwrap();
    pallet_balances::GenesisConfig::<Test> {
        balances: (1..=10).map(|a| (a, 1_000_000)).collect(),
        ..Default::default()
    }
    .assimilate_storage(&mut t)
    .unwrap();
    pallet_subnet_registry::GenesisConfig::<Test> {
        bootstrap_blocks: 25,
        bootstrap_operator: Some(OPERATOR),
        ..Default::default()
    }
    .assimilate_storage(&mut t)
    .unwrap();
    let mut ext = sp_io::TestExternalities::new(t);
    ext.execute_with(|| System::set_block_number(1));
    ext
}
