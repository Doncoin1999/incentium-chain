use crate as pallet_subnet_registry;
use ict_economics::UNIT;
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
    pub const NeuronFee: u128 = UNIT;
    pub const MaxNeurons: u16 = 4;
    pub const Treasury: u64 = TREASURY;
    pub static SudoRemoved: bool = false;
    pub static ApprovedReferenda: u32 = 0;
}

pub const TREASURY: u64 = 99;

std::thread_local! {
    pub static STAKES: std::cell::RefCell<std::collections::BTreeMap<(u16, u64), u128>> =
        Default::default();
}

pub fn set_stake(netuid: u16, who: u64, amount: u128) {
    STAKES.with(|s| s.borrow_mut().insert((netuid, who), amount));
}

pub struct MockStake;
impl pallet_subnet_registry::SubnetStakeInfo<u64> for MockStake {
    fn stake_of(netuid: u16, who: &u64) -> u128 {
        STAKES.with(|s| s.borrow().get(&(netuid, *who)).copied().unwrap_or(0))
    }
    fn total(netuid: u16) -> u128 {
        STAKES.with(|s| {
            s.borrow()
                .iter()
                .filter(|((n, _), _)| *n == netuid)
                .map(|(_, v)| *v)
                .sum()
        })
    }
}

pub struct MockSignals;
impl pallet_subnet_registry::UnlockSignals for MockSignals {
    fn sudo_removed() -> bool {
        SudoRemoved::get()
    }
    fn approved_referenda(_at_least: u32) -> u32 {
        ApprovedReferenda::get()
    }
}

impl pallet_subnet_registry::Config for Test {
    type RuntimeEvent = RuntimeEvent;
    type WeightInfo = ();
    type Currency = Balances;
    type PalletId = RegistryPalletId;
    type TreasuryAccount = Treasury;
    type Unlock = MockSignals;
    type StakeInfo = MockStake;
    type OnRemoved = ();
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
        balances: vec![
            (ALICE, 10_000_000 * UNIT),
            (BOB, 10_000_000 * UNIT),
            (TREASURY, UNIT),
        ],
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
