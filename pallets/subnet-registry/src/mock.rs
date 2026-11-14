use crate as pallet_subnet_registry;
use ipm_economics::UNIT;
use polkadot_sdk::frame_support::{derive_impl, parameter_types, PalletId};
use polkadot_sdk::sp_runtime::BuildStorage;
use polkadot_sdk::{frame_system, pallet_balances, sp_io};

type Block = frame_system::mocking::MockBlock<Test>;

polkadot_sdk::frame_support::construct_runtime!(
    pub enum Test {
        System: frame_system,
        Balances: pallet_balances,
        Registry: pallet_subnet_registry,
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

pub const DAY: u32 = 7_200;

parameter_types! {
    pub const RegistryPalletId: PalletId = PalletId(*b"regipall");
    pub const DepositFloor: u128 = 1_000 * UNIT;
    pub const DecayBlocks: u32 = 14 * DAY;
    pub const ReturnPpm: u32 = 900_000;
    pub const NeuronFee: u128 = UNIT;
    pub const MaxNeurons: u16 = 4;
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

pub const ALICE: u64 = 1;
pub const BOB: u64 = 2;

pub fn new_test_ext(phase2: bool) -> sp_io::TestExternalities {
    let mut t = frame_system::GenesisConfig::<Test>::default()
        .build_storage()
        .unwrap();
    pallet_balances::GenesisConfig::<Test> {
        balances: vec![(ALICE, 100_000 * UNIT), (BOB, 100_000 * UNIT)],
        ..Default::default()
    }
    .assimilate_storage(&mut t)
    .unwrap();
    pallet_subnet_registry::GenesisConfig::<Test> {
        phase2_enabled: phase2,
        ..Default::default()
    }
    .assimilate_storage(&mut t)
    .unwrap();
    let mut ext = sp_io::TestExternalities::new(t);
    ext.execute_with(|| System::set_block_number(1));
    ext
}

pub const OPERATOR: u64 = 7;

/// Genesis with a 100-block bootstrap run by OPERATOR (who has no balance: fees are waived).
pub fn new_bootstrap_ext() -> sp_io::TestExternalities {
    let mut t = frame_system::GenesisConfig::<Test>::default()
        .build_storage()
        .unwrap();
    pallet_balances::GenesisConfig::<Test> {
        balances: vec![(ALICE, 100_000 * UNIT)],
        ..Default::default()
    }
    .assimilate_storage(&mut t)
    .unwrap();
    pallet_subnet_registry::GenesisConfig::<Test> {
        phase2_enabled: false,
        bootstrap_blocks: 100,
        bootstrap_operator: Some(OPERATOR),
        ..Default::default()
    }
    .assimilate_storage(&mut t)
    .unwrap();
    let mut ext = sp_io::TestExternalities::new(t);
    ext.execute_with(|| System::set_block_number(1));
    ext
}
