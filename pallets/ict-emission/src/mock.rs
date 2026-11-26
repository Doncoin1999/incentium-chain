use crate as pallet_ict_emission;
use polkadot_sdk::frame_support::{derive_impl, parameter_types};
use polkadot_sdk::sp_runtime::BuildStorage;
use polkadot_sdk::{frame_system, pallet_balances, sp_io};

type Block = frame_system::mocking::MockBlock<Test>;

polkadot_sdk::frame_support::construct_runtime!(
    pub enum Test {
        System: frame_system,
        Balances: pallet_balances,
        Emission: pallet_ict_emission,
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

pub const FUND: u64 = 1_000;
pub const POT: u64 = 2_000;
pub const VPOT: u64 = 3_000;

parameter_types! {
    pub const FundAccount: u64 = FUND;
    pub const SubnetPot: u64 = POT;
    pub const ValidatorPot: u64 = VPOT;
}

impl pallet_ict_emission::Config for Test {
    type RuntimeEvent = RuntimeEvent;
    type WeightInfo = ();
    type Currency = Balances;
    type FundAccount = FundAccount;
    type SubnetPot = SubnetPot;
    type ValidatorPot = ValidatorPot;
}

pub fn new_test_ext() -> sp_io::TestExternalities {
    let t = frame_system::GenesisConfig::<Test>::default()
        .build_storage()
        .unwrap();
    let mut ext = sp_io::TestExternalities::new(t);
    ext.execute_with(|| System::set_block_number(1));
    ext
}
