//! Open block production (docs/OPEN-BLOCK-PRODUCTION.md, approved 26 Nov 2026).
//!
//! Anyone can bond ICT and stand as a block producer, or back one; the chain elects the set
//! every era (one day). The four genesis keys keep reserved slots until independent producers
//! have carried the network for 30 eras (pallet_block_producers).

use super::*;
use alloc::vec::Vec;
use frame_election_provider_support::{bounds::ElectionBoundsBuilder, onchain, SequentialPhragmen};
use frame_support::{
    parameter_types,
    traits::{
        fungible::{Credit, Inspect, Mutate},
        tokens::{Fortitude, Precision, Preservation},
        ConstU32, OnUnbalanced,
    },
};
use sp_consensus_aura::sr25519::AuthorityId as AuraId;
use sp_runtime::{traits::OpaqueKeys, Perbill};
use sp_staking::{EraIndex, SessionIndex};

parameter_types! {
    /// A session lasts 6 hours.
    pub const SessionPeriod: BlockNumber = 1_800;
    pub const SessionOffset: BlockNumber = 0;
    /// Four sessions make an era of one day.
    pub const SessionsPerEra: SessionIndex = 4;
    pub const BondingDuration: EraIndex = 7;
    pub const SlashDeferDuration: EraIndex = 6;
    pub const MaxValidatorSet: u32 = 100;
    pub const MaxBackersPerWinner: u32 = 512;
    pub const MaxExposurePageSize: u32 = 64;
    /// Up to 1,000 voters and 1,000 candidates in one on-chain election.
    pub ElectionBounds: frame_election_provider_support::bounds::ElectionBounds =
        ElectionBoundsBuilder::default().voters_count(1_000.into()).targets_count(1_000.into()).build();
    /// Equivocation reports stay valid for as long as a slash can be applied.
    pub const ReportLongevity: u64 =
        BondingDuration::get() as u64 * SessionsPerEra::get() as u64 * SessionPeriod::get() as u64;
    /// GRANDPA equivocation is slashed at least 10 %.
    pub const MinSlash: Perbill = Perbill::from_percent(10);
    pub const BagThresholds: &'static [u64] = &BAG_THRESHOLDS;
    pub const ValidatorPotId: frame_support::PalletId = frame_support::PalletId(*b"ict/vpot");
    pub ValidatorPot: AccountId = sp_runtime::traits::AccountIdConversion::<AccountId>::into_account_truncating(&ValidatorPotId::get());
}

/// GRANDPA equivocation reports are submitted as unsigned (bare) extrinsics.
impl<C> frame_system::offchain::CreateTransactionBase<C> for Runtime
where
    RuntimeCall: From<C>,
{
    type Extrinsic = UncheckedExtrinsic;
    type RuntimeCall = RuntimeCall;
}

impl<C> frame_system::offchain::CreateBare<C> for Runtime
where
    RuntimeCall: From<C>,
{
    fn create_bare(call: RuntimeCall) -> UncheckedExtrinsic {
        // the plain format, converted (the testnet wraps it for EVM transactions)
        sp_runtime::generic::UncheckedExtrinsic::new_bare(call).into()
    }
}

/// Elected slots while the genesis keys keep their reserve (24 - 4).
pub const ELECTED_SLOTS: u32 = 20;
pub const MIN_VALIDATOR_BOND: Balance = 100 * UNIT;
pub const MIN_NOMINATOR_BOND: Balance = 10 * UNIT;

impl pallet_authorship::Config for Runtime {
    type FindAuthor = pallet_session::FindAccountFromAuthorIndex<Self, Aura>;
    type EventHandler = Staking;
}

impl pallet_session::Config for Runtime {
    type RuntimeEvent = RuntimeEvent;
    type ValidatorId = AccountId;
    type ValidatorIdOf = sp_runtime::traits::ConvertInto;
    type ShouldEndSession = pallet_session::PeriodicSessions<SessionPeriod, SessionOffset>;
    type NextSessionRotation = pallet_session::PeriodicSessions<SessionPeriod, SessionOffset>;
    type SessionManager = pallet_session::historical::NoteHistoricalRoot<Self, BlockProducers>;
    type SessionHandler = <SessionKeys as OpaqueKeys>::KeyTypeIdProviders;
    type Keys = SessionKeys;
    type DisablingStrategy = pallet_session::disabling::UpToLimitWithReEnablingDisablingStrategy;
    type WeightInfo = ();
    type Currency = Balances;
    type KeyDeposit = ();
}

impl pallet_session::historical::Config for Runtime {
    type RuntimeEvent = RuntimeEvent;
    type FullIdentification = ();
    type FullIdentificationOf = pallet_staking::UnitIdentificationOf<Self>;
}

impl pallet_offences::Config for Runtime {
    type RuntimeEvent = RuntimeEvent;
    type IdentificationTuple = pallet_session::historical::IdentificationTuple<Self>;
    type OnOffenceHandler = pallet_block_producers::MinSlash<Staking, MinSlash>;
}

type VoterBagsListInstance = pallet_bags_list::Instance1;
impl pallet_bags_list::Config<VoterBagsListInstance> for Runtime {
    type RuntimeEvent = RuntimeEvent;
    type WeightInfo = ();
    type ScoreProvider = Staking;
    type BagThresholds = BagThresholds;
    type MaxAutoRebagPerBlock = ConstU32<0>;
    type Score = sp_npos_elections::VoteWeight;
}

pub struct OnChainSeqPhragmen;
impl onchain::Config for OnChainSeqPhragmen {
    type System = Runtime;
    type Solver = SequentialPhragmen<AccountId, sp_runtime::Perbill>;
    type DataProvider = Staking;
    type WeightInfo = ();
    type MaxBackersPerWinner = MaxBackersPerWinner;
    type MaxWinnersPerPage = MaxValidatorSet;
    type Bounds = ElectionBounds;
    type Sort = frame_support::traits::ConstBool<true>;
}

/// The era's validator payout is whatever the reward pot holds (2 % of issuance from the fund,
/// plus all transaction fees). The pot is burned here and staking mints the same amount again
/// when rewards are claimed, so the supply is unchanged.
pub struct PotPayout;
impl sp_staking::EraPayout<Balance> for PotPayout {
    fn era_payout(_staked: Balance, _issuance: Balance, _era_ms: u64) -> (Balance, Balance) {
        let pot = ValidatorPot::get();
        let amount = <Balances as Inspect<AccountId>>::reducible_balance(
            &pot,
            Preservation::Preserve,
            Fortitude::Polite,
        );
        if amount == 0 {
            return (0, 0);
        }
        match <Balances as Mutate<AccountId>>::burn_from(
            &pot,
            amount,
            Preservation::Preserve,
            Precision::Exact,
            Fortitude::Polite,
        ) {
            Ok(burned) => (burned, 0),
            Err(_) => (0, 0),
        }
    }
}

/// Transaction fees go to the block producers' reward pot.
pub struct FeesToPot;
impl OnUnbalanced<Credit<AccountId, Balances>> for FeesToPot {
    fn on_nonzero_unbalanced(credit: Credit<AccountId, Balances>) {
        use frame_support::traits::fungible::Balanced;
        let _ = <Balances as Balanced<AccountId>>::resolve(&ValidatorPot::get(), credit);
    }
}

/// Sizes for staking benchmarks (pallet_staking's own test config exists only with std, so
/// the on-chain build needs its own).
pub struct StakingBenchmarkingConfig;
impl pallet_staking::BenchmarkingConfig for StakingBenchmarkingConfig {
    type MaxValidators = ConstU32<100>;
    type MaxNominators = ConstU32<1_000>;
}

impl pallet_staking::Config for Runtime {
    type OldCurrency = Balances;
    type Currency = Balances;
    type RuntimeHoldReason = RuntimeHoldReason;
    type CurrencyBalance = Balance;
    type UnixTime = Timestamp;
    type CurrencyToVote = sp_staking::currency_to_vote::U128CurrencyToVote;
    type ElectionProvider = onchain::OnChainExecution<OnChainSeqPhragmen>;
    type GenesisElectionProvider = onchain::OnChainExecution<OnChainSeqPhragmen>;
    type NominationsQuota = pallet_staking::FixedNominationsQuota<16>;
    type HistoryDepth = ConstU32<84>;
    type RewardRemainder = ();
    type RuntimeEvent = RuntimeEvent;
    /// Slashed ICT is burned.
    type Slash = ();
    type Reward = ();
    type SessionsPerEra = SessionsPerEra;
    type BondingDuration = BondingDuration;
    type SlashDeferDuration = SlashDeferDuration;
    /// Root (a referendum on the Root track) may cancel a deferred slash or change the slots.
    type AdminOrigin = frame_system::EnsureRoot<AccountId>;
    type SessionInterface = Self;
    type EraPayout = PotPayout;
    type NextNewSession = Session;
    type MaxExposurePageSize = MaxExposurePageSize;
    type MaxValidatorSet = MaxValidatorSet;
    type VoterList = VoterList;
    type TargetList = pallet_staking::UseValidatorsMap<Self>;
    type MaxUnlockingChunks = ConstU32<32>;
    type MaxControllersInDeprecationBatch = ConstU32<100>;
    type EventListeners = ();
    type Filter = frame_support::traits::Nothing;
    type BenchmarkingConfig = StakingBenchmarkingConfig;
    type WeightInfo = ();
}

parameter_types! {
    pub const MinIndependent: u32 = 7;
    pub const StreakEras: u32 = 30;
    pub const MaxGenesis: u32 = 8;
}

impl pallet_block_producers::Config for Runtime {
    type RuntimeEvent = RuntimeEvent;
    type MinIndependent = MinIndependent;
    type StreakEras = StreakEras;
    type MaxGenesis = MaxGenesis;
}

/// Live-chain migration: seed session keys from the current Aura/GRANDPA authorities, keep them
/// as the genesis reserve, and set the staking parameters. Runs once (when session is empty).
pub struct InitBlockProduction;
impl frame_support::traits::OnRuntimeUpgrade for InitBlockProduction {
    fn on_runtime_upgrade() -> frame_support::weights::Weight {
        let db = <Runtime as frame_system::Config>::DbWeight::get();
        if !pallet_session::Validators::<Runtime>::get().is_empty() {
            return db.reads(1);
        }
        let aura: Vec<AuraId> = pallet_aura::Authorities::<Runtime>::get().into_inner();
        let grandpa: Vec<sp_consensus_grandpa::AuthorityId> =
            pallet_grandpa::Pallet::<Runtime>::grandpa_authorities()
                .into_iter()
                .map(|(id, _)| id)
                .collect();
        let validators = seed_session(&aura, &grandpa);
        let n = validators.len() as u64;
        crate::BlockProducers::set_genesis_validators(validators);
        set_staking_params();
        db.reads_writes(3, 6 + 4 * n)
    }
}

/// Writes session storage for the given authorities (paired by index); returns validator ids.
pub fn seed_session(aura: &[AuraId], grandpa: &[sp_consensus_grandpa::AuthorityId]) -> Vec<AccountId> {
    use codec::Encode;
    let mut validators = Vec::new();
    let mut queued = Vec::new();
    for (a, g) in aura.iter().zip(grandpa.iter()) {
        let who: AccountId = crate::genesis_config_presets::validator_id(a);
        let keys = SessionKeys { aura: a.clone(), grandpa: g.clone() };
        pallet_session::NextKeys::<Runtime>::insert(&who, &keys);
        pallet_session::KeyOwner::<Runtime>::insert((sp_core::crypto::key_types::AURA, a.encode()), &who);
        pallet_session::KeyOwner::<Runtime>::insert((sp_core::crypto::key_types::GRANDPA, g.encode()), &who);
        validators.push(who.clone());
        queued.push((who, keys));
    }
    pallet_session::Validators::<Runtime>::put(&validators);
    pallet_session::QueuedKeys::<Runtime>::put(queued);
    validators
}

pub fn set_staking_params() {
    pallet_staking::ValidatorCount::<Runtime>::put(ELECTED_SLOTS);
    pallet_staking::MinimumValidatorCount::<Runtime>::put(1);
    pallet_staking::MinValidatorBond::<Runtime>::put(MIN_VALIDATOR_BOND);
    pallet_staking::MinNominatorBond::<Runtime>::put(MIN_NOMINATOR_BOND);
}

/// 200 bags, geometric from 10 ICT to 21,000,000 ICT (vote weight = base units).
pub const BAG_THRESHOLDS: [u64; 200] = [
    10_000_000_000, 10_762_927_144, 11_584_060_070, 12_467_839_457, 13_419_044_772, 14_442_820_122,
    15_544_702_073, 16_730_649_589, 18_007_076_260, 19_380_884_987, 20_859_505_310, 22_450_933_591,
    24_163_776_256, 26_007_296_337, 27_991_463_569, 30_127_008_305, 32_425_479_545, 34_899_307_396,
    37_561_870_288, 40_427_567_330, 43_511_896_179, 46_831_536_847, 50_404_441_913, 54_249_933_605,
    58_388_808_296, 62_843_448_972, 67_637_946_277, 72_798_228_795, 78_352_203_274, 84_329_905_542,
    90_763_662_941, 97_688_269_157, 105_141_172_376, 113_162_677_813, 121_796_165_673, 131_088_325_756,
    141_089_409_955, 151_853_504_015, 163_438_820_028, 175_908_011_248, 189_328_510_912, 203_772_896_924,
    219_319_284_353, 236_051_747_878, 254_060_776_464, 273_443_762_725, 294_305_529_620, 316_758_897_340,
    340_925_293_431, 366_935_409_476, 394_929_907_877, 425_060_182_550, 457_489_177_663, 492_392_268_838,
    529_958_211_581, 570_390_162_065, 613_906_775_800, 660_743_390_118, 711_153_296_876, 765_409_112_254,
    823_804_251_060, 886_654_513_513, 954_299_793_090, 1_027_105_914_663, 1_105_466_612_876, 1_189_805_661_458,
    1_280_579_164_987, 1_378_278_025_497, 1_483_430_597_269, 1_596_605_544_168, 1_718_414_914_970, 1_849_517_453_310,
    1_990_622_160_166, 2_142_492_128_123, 2_305_948_668_172, 2_481_875_751_350, 2_671_224_789_241, 2_875_019_779_202,
    3_094_362_842_131, 3_330_440_182_716, 3_584_528_504_425, 3_858_001_913_895, 4_152_339_352_093, 4_469_132_592_402,
    4_810_094_848_920, 5_177_070_041_497, 5_572_042_767_637, 5_997_149_035_170, 6_454_687_813_765, 6_947_133_467_725,
    7_477_149_137_324, 8_047_601_141_034, 8_661_574_476_546, 9_322_389_504_397, 10_033_619_904_444, 10_799_112_002_280,
    11_623_005_570_116, 12_509_756_214_624, 13_464_159_472_804, 14_491_376_746_189, 15_596_963_213_646, 16_786_897_873_717,
    18_067_615_878_971, 19_446_043_337_235, 20_929_634_767_903, 22_526_413_415_888, 24_245_014_641_233, 26_094_732_619_043,
    28_085_570_602_267, 30_228_295_019_176, 32_534_493_698_075, 35_016_638_534_149, 37_688_152_937_318, 40_563_484_425_881,
    43_658_182_758_526, 46_988_984_027_238, 50_573_901_165_889, 54_432_321_363_972, 58_585_110_912_287, 63_054_728_047_604,
    67_865_344_406_532, 73_042_975_745_452, 78_615_622_633_416, 84_613_421_878_893, 91_068_809_509_273, 98_016_696_184_523,
    105_494_655_993_618, 113_543_129_654_773, 122_205_643_218_368, 131_529_043_455_315, 141_563_751_203_839, 152_364_034_044_775,
    163_988_299_780_007, 176_499_412_301_164, 189_965_031_556_808, 204_457_979_456_633, 220_056_633_691_490, 236_845_351_599_016,
    254_914_926_367_116, 274_363_078_042_449, 295_294_981_999_286, 317_823_837_726_691, 342_071_481_010_017, 368_169_042_817_394,
    396_257_658_454_452, 426_489_230_822_387, 459_027_251_907_056, 494_047_686_941_696, 531_739_926_024_832, 572_307_808_339_688,
    615_970_724_514_076, 662_964_803_082_320, 713_544_187_465_578, 767_982_410_376_455, 826_573_873_080_679, 889_635_437_515_669,
    957_508_139_876_137, 1_030_559_034_933_804, 1_109_183_181_065_259, 1_193_805_776_722_993, 1_284_884_459_903_597, 1_382_911_783_048_637,
    1_488_417_876_762_435, 1_601_973_316_752_108, 1_724_192_209_494_241, 1_855_735_513_315_452, 1_997_314_612_847_211, 2_149_695_166_185_504,
    2_313_701_245_560_772, 2_490_219_793_910_781, 2_680_205_421_457_543, 2_884_685_568_228_109, 3_104_766_060_438_083, 3_341_637_090_786_720,
    3_596_579_655_004_968, 3_870_972_479_465_233, 4_166_299_477_317_293, 4_484_157_773_473_071, 4_826_266_341_839_264, 5_194_475_301_507_906,
    5_590_775_922_178_539, 6_017_311_392_921_195, 6_476_388_412_517_552, 6_970_489_664_060_655, 7_502_287_241_275_733, 8_074_657_099_171_459,
    8_690_694_607_170_629, 9_353_731_288_831_400, 10_067_352_838_687_288, 10_835_418_513_640_800, 11_662_082_003_779_136, 12_551_813_895_481_044,
    13_509_425_848_295_108, 14_540_096_616_337_662, 15_649_400_054_933_830, 16_843_335_263_961_188, 18_128_359_030_910_984, 19_511_420_749_118_508,
    21_000_000_000_000_000, 18_446_744_073_709_551_615,
];
