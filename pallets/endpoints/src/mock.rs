use crate as pallet_endpoints;
use polkadot_sdk::frame_support::{derive_impl, parameter_types};
use polkadot_sdk::frame_system::EnsureRoot;
use polkadot_sdk::sp_runtime::BuildStorage;
use polkadot_sdk::{frame_system, pallet_balances, sp_io};
use std::cell::RefCell;

type Block = frame_system::mocking::MockBlock<Test>;

polkadot_sdk::frame_support::construct_runtime!(
    pub enum Test {
        System: frame_system,
        Balances: pallet_balances,
        Endpoints: pallet_endpoints,
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
    type RuntimeHoldReason = RuntimeHoldReason;
}

pub const ICT: u128 = 1_000_000_000;
pub const DAY: u64 = 100;
pub const POT: u64 = 900;
pub const FUND: u64 = 901;
// Operators
pub const ALICE: u64 = 1;
pub const BOB: u64 = 2;
pub const CAROL: u64 = 3;
// Reporters (block producers / SN0 validators)
pub const R1: u64 = 11;
pub const R2: u64 = 12;
pub const R3: u64 = 13;

thread_local! {
    pub static STAKES: RefCell<Vec<(u64, u128)>> = RefCell::new(vec![(R1, 100), (R2, 100), (R3, 100)]);
}

pub struct MockReporters;
impl crate::ReporterStake<u64> for MockReporters {
    fn stake_of(who: &u64) -> u128 {
        STAKES.with(|s| {
            s.borrow()
                .iter()
                .find(|(a, _)| a == who)
                .map(|(_, b)| *b)
                .unwrap_or(0)
        })
    }
    fn total() -> u128 {
        STAKES.with(|s| s.borrow().iter().map(|(_, b)| *b).sum())
    }
}

parameter_types! {
    pub const PotAccount: u64 = POT;
    pub const FundAccount: u64 = FUND;
    pub const BlocksPerDay: u32 = DAY as u32;
    pub const MaxUrlLen: u32 = 128;
    pub const MaxPerOwner: u32 = 4;
    pub const MaxEndpoints: u32 = 64;
    pub const MaxReporters: u32 = 16;
    pub const InactiveAfterDays: u8 = 7;
}

impl pallet_endpoints::Config for Test {
    type RuntimeEvent = RuntimeEvent;
    type WeightInfo = ();
    type RuntimeHoldReason = RuntimeHoldReason;
    type Currency = Balances;
    type Reporters = MockReporters;
    type Pot = PotAccount;
    type FundAccount = FundAccount;
    type ParamsOrigin = EnsureRoot<u64>;
    type BlocksPerDay = BlocksPerDay;
    type MaxUrlLen = MaxUrlLen;
    type MaxPerOwner = MaxPerOwner;
    type MaxEndpoints = MaxEndpoints;
    type MaxReporters = MaxReporters;
    type InactiveAfterDays = InactiveAfterDays;
}

pub fn new_test_ext() -> sp_io::TestExternalities {
    let mut t = frame_system::GenesisConfig::<Test>::default()
        .build_storage()
        .unwrap();
    pallet_balances::GenesisConfig::<Test> {
        balances: vec![
            (ALICE, 1_000 * ICT),
            (BOB, 1_000 * ICT),
            (CAROL, 1_000 * ICT),
            (R1, ICT),
            (R2, ICT),
            (R3, ICT),
            (FUND, 1),
        ],
        ..Default::default()
    }
    .assimilate_storage(&mut t)
    .unwrap();
    let mut ext = sp_io::TestExternalities::new(t);
    ext.execute_with(|| System::set_block_number(1));
    ext
}

/// Run blocks up to `n`, calling the pallet's hook at each.
pub fn run_to(n: u64) {
    use polkadot_sdk::frame_support::traits::Hooks;
    while System::block_number() < n {
        let b = System::block_number() + 1;
        System::set_block_number(b);
        <Endpoints as Hooks<u64>>::on_initialize(b);
    }
}

pub fn fund_pot(amount: u128) {
    use polkadot_sdk::frame_support::traits::fungible::Mutate;
    let _ = <Balances as Mutate<u64>>::mint_into(&POT, amount);
}
