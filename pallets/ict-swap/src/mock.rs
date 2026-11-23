use crate as pallet_ict_swap;
use polkadot_sdk::frame_support::{derive_impl, parameter_types, traits::ConstU64};
use polkadot_sdk::sp_runtime::BuildStorage;
use polkadot_sdk::{frame_system, pallet_balances, pallet_timestamp, sp_io};

type Block = frame_system::mocking::MockBlock<Test>;

polkadot_sdk::frame_support::construct_runtime!(
    pub enum Test {
        System: frame_system,
        Timestamp: pallet_timestamp,
        Balances: pallet_balances,
        Swap: pallet_ict_swap,
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

impl pallet_timestamp::Config for Test {
    type Moment = u64;
    type OnTimestampSet = ();
    type MinimumPeriod = ConstU64<1>;
    type WeightInfo = ();
}

parameter_types! {
    pub const BlocksPerDay: u32 = 7_200;
    pub const MinFillUsdc: u128 = 5_000_000;
    pub const QtyStep: u128 = 10_000_000;
    pub const MaxOpenReservations: u8 = 3;
    pub const OrderTtlMax: u64 = 24 * 3600;
    pub const ReservationTtl: u64 = 19 * 3600;
    pub const LockTtlMax: u64 = 12 * 3600;
}

impl pallet_ict_swap::Config for Test {
    type RuntimeEvent = RuntimeEvent;
    type WeightInfo = ();
    type RuntimeHoldReason = RuntimeHoldReason;
    type Currency = Balances;
    type UnixTime = Timestamp;
    type BlocksPerDay = BlocksPerDay;
    type MinFillUsdc = MinFillUsdc;
    type QtyStep = QtyStep;
    type MaxOpenReservations = MaxOpenReservations;
    type OrderTtlMax = OrderTtlMax;
    type ReservationTtl = ReservationTtl;
    type LockTtlMax = LockTtlMax;
}

pub const ICT: u128 = 1_000_000_000;
pub const SELLER: u64 = 1;
pub const BUYER: u64 = 2;
pub const KEEPER: u64 = 3;
pub const T0: u64 = 1_795_000_000;

pub fn new_test_ext() -> sp_io::TestExternalities {
    let mut t = frame_system::GenesisConfig::<Test>::default()
        .build_storage()
        .unwrap();
    pallet_balances::GenesisConfig::<Test> {
        balances: vec![
            (SELLER, 10_000 * ICT),
            (BUYER, 10_000 * ICT),
            (KEEPER, 10 * ICT),
        ],
        ..Default::default()
    }
    .assimilate_storage(&mut t)
    .unwrap();
    let mut ext = sp_io::TestExternalities::new(t);
    ext.execute_with(|| {
        System::set_block_number(2 * 7_200);
        set_time(T0);
    });
    ext
}

/// Chain time in seconds.
pub fn set_time(secs: u64) {
    pallet_timestamp::Now::<Test>::put(secs * 1000);
}

pub fn advance(secs: u64) {
    let now = pallet_timestamp::Now::<Test>::get() / 1000;
    set_time(now + secs);
}
