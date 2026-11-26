use crate as pallet_ict_consensus;
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
        Consensus: pallet_ict_consensus,
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
    /// Lost payout: half of the active validator stake; slash: 2/3.
    pub const QuorumPpm: u32 = 500_000;
    pub const SlashQuorumPpm: u32 = 666_667;
    pub const MaxFailures: u32 = 3;
    pub const FailureWindow: u64 = 600;
    pub const UnbondingEpochs: u64 = 3;
    pub const MaxReporters: u32 = 16;
    pub const MaxPayees: u32 = 64;
    pub const SubnetPot: u64 = POT;
    pub const MinerSharePpm: u32 = 666_667;
    pub const Sn0MinerSharePpm: u32 = 568_182;
    /// 50 % of issuance with a 10 % fund share: 0.5 / 0.9 of the pot.
    pub const Sn0MinPotPpm: u32 = 555_556;
    pub const KappaPpm: u32 = 500_000;
    pub const MaxValidatorPpm: u32 = 100_000;
    pub const MinValidatorStake: u128 = 100;
    pub const LaunchCliff: u64 = 20;
    pub const LaunchStep: u64 = 20;
    pub const LaunchStepPpm: u32 = 100_000;
    /// Launch-period rewards: 25 % paid at once, 75 % locked in the launch vault.
    pub const LaunchImmediatePpm: u32 = 250_000;
}

impl pallet_subnet_registry::Config for Test {
    type RuntimeEvent = RuntimeEvent;
    type WeightInfo = ();
    type Currency = Balances;
    type PalletId = RegistryPalletId;
    type TreasuryAccount = RegistryTreasury;
    type Unlock = ();
    type StakeInfo = Consensus;
    type OnRemoved = Consensus;
    type Sale = ();
    type NeuronFee = NeuronFee;
    type MaxNeurons = MaxNeurons;
}

impl pallet_weights::Config for Test {
    type RuntimeEvent = RuntimeEvent;
    type WeightInfo = ();
    type EpochLength = EpochLength;
    type MaxWeights = MaxWeights;
}

impl pallet_work_verify::Config for Test {
    type RuntimeEvent = RuntimeEvent;
    type WeightInfo = ();
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

impl pallet_ict_consensus::Config for Test {
    type RuntimeEvent = RuntimeEvent;
    type WeightInfo = ();
    type StakeCurrency = Balances;
    type Alpha = MockAlpha;
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

// ---------------------------------------------------------------- Alpha pools stand-in

pub const DIVS: u64 = 4_242;

thread_local! {
    pub static ALPHA_ON: std::cell::RefCell<bool> = const { std::cell::RefCell::new(false) };
    pub static ALPHA_ICT: std::cell::RefCell<std::collections::BTreeMap<u16, u128>> = Default::default();
    pub static ALPHA_STAKE: std::cell::RefCell<std::collections::BTreeMap<(u16, u64), u128>> = Default::default();
    pub static ALPHA_OWN: std::cell::RefCell<std::collections::BTreeMap<(u16, u64), u128>> = Default::default();
    pub static ALPHA_EPOCHS: std::cell::RefCell<Vec<(u16, u32, Vec<(u64, u128)>, Vec<(u64, u128)>)>> = Default::default();
    pub static ALPHA_PAID: std::cell::RefCell<Vec<(u16, u64, u128)>> = Default::default();
    pub static ALPHA_REMOVED: std::cell::RefCell<Vec<u16>> = Default::default();
}

/// Records what consensus asks of the Alpha module; validator pay goes to DIVS.
pub struct MockAlpha;
impl crate::AlphaHooks<u64> for MockAlpha {
    fn active(_netuid: u16) -> bool {
        ALPHA_ON.with(|a| *a.borrow())
    }
    fn stake_ict(netuid: u16) -> u128 {
        ALPHA_ICT.with(|m| m.borrow().get(&netuid).copied().unwrap_or(0))
    }
    fn alpha_stake(netuid: u16, hotkey: &u64) -> u128 {
        ALPHA_STAKE.with(|m| m.borrow().get(&(netuid, *hotkey)).copied().unwrap_or(0))
    }
    fn total_alpha(netuid: u16) -> u128 {
        ALPHA_STAKE.with(|m| m.borrow().iter().filter(|((n, _), _)| *n == netuid).map(|(_, v)| *v).sum())
    }
    fn staker_alpha(netuid: u16, who: &u64) -> u128 {
        ALPHA_OWN.with(|m| m.borrow().get(&(netuid, *who)).copied().unwrap_or(0))
    }
    fn on_epoch(netuid: u16, blocks: u32, miners: &[(u64, u128)], validators: &[(u64, u128)]) {
        ALPHA_EPOCHS.with(|e| e.borrow_mut().push((netuid, blocks, miners.to_vec(), validators.to_vec())));
    }
    fn pay_validator(netuid: u16, source: &u64, hotkey: &u64, amount: u128) -> bool {
        use polkadot_sdk::frame_support::traits::{fungible::Mutate, tokens::Preservation};
        let ok = <Balances as Mutate<u64>>::transfer(source, &DIVS, amount, Preservation::Expendable).is_ok();
        if ok {
            ALPHA_PAID.with(|p| p.borrow_mut().push((netuid, *hotkey, amount)));
        }
        ok
    }
    fn subnet_removed(netuid: u16) {
        ALPHA_REMOVED.with(|r| r.borrow_mut().push(netuid));
    }
}
