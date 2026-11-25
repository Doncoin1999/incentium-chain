use crate as pallet_work_verify;
use core::cell::RefCell;
use polkadot_sdk::frame_support::{derive_impl, parameter_types, PalletId};
use polkadot_sdk::sp_runtime::BuildStorage;
use polkadot_sdk::{frame_system, pallet_balances, sp_io};
use std::collections::BTreeMap;

type Block = frame_system::mocking::MockBlock<Test>;

polkadot_sdk::frame_support::construct_runtime!(
    pub enum Test {
        System: frame_system,
        Balances: pallet_balances,
        Registry: pallet_subnet_registry,
        Weights: pallet_weights,
        Work: pallet_work_verify,
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

parameter_types! {
    pub const RegistryPalletId: PalletId = PalletId(*b"regipall");
    pub const WorkPalletId: PalletId = PalletId(*b"workpall");
    pub const RegistryTreasury: u64 = 999;
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
}

impl pallet_subnet_registry::Config for Test {
    type RuntimeEvent = RuntimeEvent;
    type WeightInfo = ();
    type Currency = Balances;
    type PalletId = RegistryPalletId;
    type TreasuryAccount = RegistryTreasury;
    type Unlock = ();
    type StakeInfo = ();
    type OnRemoved = ();
    type NeuronFee = NeuronFee;
    type MaxNeurons = MaxNeurons;
}

impl pallet_weights::Config for Test {
    type RuntimeEvent = RuntimeEvent;
    type WeightInfo = ();
    type EpochLength = EpochLength;
    type MaxWeights = MaxWeights;
}

thread_local! {
    pub static STAKES: RefCell<BTreeMap<u64, u128>> = const { RefCell::new(BTreeMap::new()) };
}

pub struct MockStake;
impl crate::StakeOracle<u64> for MockStake {
    fn stake_of(who: &u64) -> u128 {
        STAKES.with(|s| s.borrow().get(who).copied().unwrap_or(0))
    }
    fn total_stake() -> u128 {
        STAKES.with(|s| s.borrow().values().sum())
    }
    #[cfg(feature = "runtime-benchmarks")]
    fn set_stake(who: &u64, amount: u128) {
        set_stake(*who, amount);
    }
}

pub fn set_stake(who: u64, amount: u128) {
    STAKES.with(|s| s.borrow_mut().insert(who, amount));
}

impl pallet_work_verify::Config for Test {
    type RuntimeEvent = RuntimeEvent;
    type WeightInfo = ();
    type WorkCurrency = Balances;
    type Stake = MockStake;
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

pub fn new_test_ext() -> sp_io::TestExternalities {
    STAKES.with(|s| s.borrow_mut().clear());
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
