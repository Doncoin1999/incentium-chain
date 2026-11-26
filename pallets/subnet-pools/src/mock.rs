use crate as pallet_subnet_pools;
use crate::{IssuanceInfo, NetUid, SubnetInfo};
use polkadot_sdk::frame_support::{derive_impl, parameter_types, PalletId};
use polkadot_sdk::sp_runtime::BuildStorage;
use polkadot_sdk::{frame_system, pallet_balances, sp_io};
use std::cell::RefCell;

type Block = frame_system::mocking::MockBlock<Test>;

polkadot_sdk::frame_support::construct_runtime!(
    pub enum Test {
        System: frame_system,
        Balances: pallet_balances,
        Pools: pallet_subnet_pools,
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

pub const UNIT: u128 = 1_000_000_000;
pub const OWNER: u64 = 100;
pub const VALI: u64 = 10;
pub const MINER: u64 = 20;
pub const ALICE: u64 = 1;
pub const BOB: u64 = 2;
pub const TREASURY: u64 = 999;
pub const SELLER: u64 = 50;

thread_local! {
    pub static HAS_OWNER: RefCell<bool> = const { RefCell::new(true) };
}

pub struct MockSubnets;
impl SubnetInfo<u64> for MockSubnets {
    fn exists(netuid: NetUid) -> bool {
        netuid == 1 || netuid == 2
    }
    fn owner(_: NetUid) -> Option<u64> {
        if HAS_OWNER.with(|h| *h.borrow()) { Some(OWNER) } else { None }
    }
    fn is_neuron(netuid: NetUid, hotkey: &u64) -> bool {
        Self::exists(netuid) && [VALI, MINER, OWNER].contains(hotkey)
    }
    fn name(netuid: NetUid) -> Vec<u8> {
        format!("SN{netuid}").into_bytes()
    }
}

thread_local! {
    pub static SUBNET_ICT: RefCell<u128> = const { RefCell::new(288 * UNIT) };
}

pub struct MockIssuance;
impl IssuanceInfo for MockIssuance {
    fn subnet_ict_per_day() -> u128 {
        SUBNET_ICT.with(|s| *s.borrow())
    }
    fn days_to_ict_halving() -> u64 {
        1_280
    }
}

parameter_types! {
    pub const Treasury: u64 = TREASURY;
    pub const PoolsPalletId: PalletId = PalletId(*b"alphapol");
    pub const FeePpm: u32 = 5_000;
    pub const FeeBurnPpm: u32 = 500_000;
    pub const MinTrade: u128 = UNIT / 100;
}

impl pallet_subnet_pools::Config for Test {
    type RuntimeEvent = RuntimeEvent;
    type Currency = Balances;
    type PalletId = PoolsPalletId;
    type Subnets = MockSubnets;
    type Issuance = MockIssuance;
    type Treasury = Treasury;
    type FeePpm = FeePpm;
    type FeeBurnPpm = FeeBurnPpm;
    type MinTrade = MinTrade;
}

pub fn new_test_ext() -> sp_io::TestExternalities {
    let mut t = frame_system::GenesisConfig::<Test>::default().build_storage().unwrap();
    pallet_balances::GenesisConfig::<Test> {
        balances: vec![(ALICE, 100_000 * UNIT), (BOB, 100_000 * UNIT), (OWNER, 10 * UNIT), (SELLER, 100_000 * UNIT), (VALI, UNIT), (TREASURY, UNIT)],
        ..Default::default()
    }
    .assimilate_storage(&mut t)
    .unwrap();
    let mut ext = sp_io::TestExternalities::new(t);
    ext.execute_with(|| System::set_block_number(1));
    ext
}
