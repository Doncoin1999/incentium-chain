use crate as pallet_weights;
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
    pub const DepositFloor: u128 = 1_000;
    pub const DecayBlocks: u32 = 100;
    pub const ReturnPpm: u32 = 900_000;
    pub const NeuronFee: u128 = 1;
    pub const MaxNeurons: u16 = 16;
    pub const EpochLength: u32 = 10;
    pub const MaxWeights: u32 = 16;
}

impl pallet_subnet_registry::Config for Test {
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

impl pallet_weights::Config for Test {
    type RuntimeEvent = RuntimeEvent;
    type WeightInfo = ();
    type EpochLength = EpochLength;
    type MaxWeights = MaxWeights;
}

pub fn new_test_ext() -> sp_io::TestExternalities {
    let mut t = frame_system::GenesisConfig::<Test>::default()
        .build_storage()
        .unwrap();
    pallet_balances::GenesisConfig::<Test> {
        balances: (1..=5).map(|a| (a, 1_000_000)).collect(),
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
