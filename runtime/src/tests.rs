//! End-to-end runtime test of the Phase 1 loop on SN0:
//! issuance -> neurons and stake -> pool -> commit/reveal -> epoch settlement -> pool payout.

use crate::*;
use frame_support::{assert_ok, traits::Hooks};
use sp_core::H256;
use sp_keyring::Sr25519Keyring as K;
use sp_runtime::traits::AccountIdConversion;

type Emission = pallet_ict_emission::Pallet<Runtime>;

fn acc(k: K) -> AccountId {
    k.to_account_id()
}

fn signed(k: K) -> RuntimeOrigin {
    RuntimeOrigin::signed(acc(k))
}

/// Presets are JSON patches over the default genesis; merge them as the node does.
fn merge(a: &mut serde_json::Value, b: serde_json::Value) {
    match (a, b) {
        (serde_json::Value::Object(a), serde_json::Value::Object(b)) => {
            for (k, v) in b {
                merge(a.entry(k).or_insert(serde_json::Value::Null), v);
            }
        }
        (a, b) => *a = b,
    }
}

pub(crate) fn ext_from_preset(preset: &str) -> sp_io::TestExternalities {
    let patch: serde_json::Value = serde_json::from_slice(
        &genesis_config_presets::get_preset(&preset.into()).expect("preset exists"),
    )
    .unwrap();
    let mut full = serde_json::to_value(RuntimeGenesisConfig::default()).unwrap();
    merge(&mut full, patch);
    let json = serde_json::to_vec(&full).unwrap();
    let mut ext = sp_io::TestExternalities::default();
    ext.execute_with(|| {
        frame_support::genesis_builder_helper::build_state::<RuntimeGenesisConfig>(json)
            .expect("genesis builds");
    });
    ext
}

fn run_to(n: BlockNumber) {
    while System::block_number() < n {
        let b = System::block_number() + 1;
        System::set_block_number(b);
        Emission::on_initialize(b);
        Consensus::on_initialize(b);
    }
}

fn free(k: K) -> Balance {
    Balances::free_balance(acc(k))
}

#[test]
fn presets_build_with_sn0_and_phase2_off() {
    for p in [
        sp_genesis_builder::DEV_RUNTIME_PRESET,
        sp_genesis_builder::LOCAL_TESTNET_RUNTIME_PRESET,
    ] {
        ext_from_preset(p).execute_with(|| {
            assert!(pallet_subnet_registry::Subnets::<Runtime>::get(0).is_some());
            assert!(!pallet_subnet_registry::Phase2Enabled::<Runtime>::get());
            assert_eq!(free(K::Alice), 100_000 * UNIT);
            // Tier table: only tier 0 (2B, CPU) open at launch.
            let tiers = pallet_subnet_registry::Tiers::<Runtime>::iter().collect::<Vec<_>>();
            assert_eq!(tiers.len(), 4);
            assert!(tiers.iter().all(|(t, i)| i.open == (*t == 0)));
        });
    }
}

#[test]
fn phase1_loop_end_to_end() {
    ext_from_preset(sp_genesis_builder::LOCAL_TESTNET_RUNTIME_PRESET).execute_with(|| {
        System::set_block_number(1);
        // uids: Alice 0, Bob 1 (validators); Charlie 2 (solo miner); Dave 3 (pool)
        for k in [K::Alice, K::Bob, K::Charlie, K::Dave] {
            assert_ok!(Registry::register_neuron(signed(k), 0, acc(k)));
        }
        assert_ok!(Consensus::stake(signed(K::Alice), 10_000 * UNIT));
        assert_ok!(Consensus::stake(signed(K::Bob), 10_000 * UNIT));
        assert_ok!(WorkVerify::bond(signed(K::Dave), 5_000 * UNIT));
        assert_ok!(WorkVerify::register_pool(signed(K::Dave), 20_000)); // 2 % fee

        // epoch 0: both validators measured Dave's pool doing 3x Charlie's work
        let salt = [9u8; 32];
        let (uids, w) = (vec![2u16, 3], vec![1u16, 3]);
        let h: H256 = Weights::commit_hash(0, &uids, &w, &salt);
        assert_ok!(Weights::commit_weights(signed(K::Alice), 0, h));
        assert_ok!(Weights::commit_weights(signed(K::Bob), 0, h));

        run_to(EPOCH_LENGTH); // epoch 1: reveal
        assert_ok!(Weights::reveal_weights(
            signed(K::Alice),
            0,
            uids.clone(),
            w.clone(),
            salt
        ));
        assert_ok!(Weights::reveal_weights(signed(K::Bob), 0, uids, w, salt));

        let (c0, a0) = (free(K::Charlie), free(K::Alice));
        run_to(2 * EPOCH_LENGTH); // epoch 2 starts: settle epoch 0

        // issuance so far: 1 ICT per block from block 2..=720 (block 1 was set, not initialised)
        let issued = pallet_ict_emission::TotalIssued::<Runtime>::get();
        assert_eq!(issued, (2 * EPOCH_LENGTH as u128 - 1) * UNIT);
        assert_eq!(
            Balances::free_balance(configs::FundAccount::get()),
            issued * 8 / 100 + EXISTENTIAL_DEPOSIT // the treasury opens the fund account with the ED
        );
        // runtime 12: 2 % of issuance goes to the block producers' reward pot
        assert_eq!(
            Balances::free_balance(block_production::ValidatorPot::get()),
            issued * 2 / 100
        );

        // the whole pot at settlement time is paid out: 2/3 miners (1:3), 1/3 validators (1:1)
        // nothing was paid before, so the pot at settlement is everything minted to it
        let pot = issued - issued / 10;
        assert_eq!(Balances::free_balance(configs::SubnetPot::get()), 0);
        let miners = ict_economics::mul_div(pot, 666_667, 1_000_000);
        let charlie_gain = free(K::Charlie) - c0;
        let pool_balance = pallet_work_verify::Pools::<Runtime>::get(acc(K::Dave))
            .unwrap()
            .balance;
        assert!(
            charlie_gain.abs_diff(miners / 4) <= 2,
            "{charlie_gain} vs {}",
            miners / 4
        );
        assert!(pool_balance.abs_diff(miners * 3 / 4) <= 2);
        assert!((free(K::Alice) - a0).abs_diff((pot - miners) / 2) <= 2);

        // the pool pays its sub-workers 3:1 after its 2 % fee
        let (e0, f0) = (free(K::Eve), free(K::Ferdie));
        assert_ok!(WorkVerify::distribute(
            signed(K::Dave),
            0,
            H256::repeat_byte(1),
            vec![(acc(K::Eve), 30), (acc(K::Ferdie), 10)]
        ));
        let shared = pool_balance - pool_balance * 2 / 100;
        assert!((free(K::Eve) - e0).abs_diff(shared * 3 / 4) <= 2);
        assert!((free(K::Ferdie) - f0).abs_diff(shared / 4) <= 2);
    });
}

#[test]
fn bootstrap_period_operator_runs_everything_then_opens() {
    use genesis_config_presets::{
        BOOTSTRAP_ALLOCATION, BOOTSTRAP_BLOCKS, BOOTSTRAP_PRESET, BOOTSTRAP_PRE_ISSUED,
    };
    ext_from_preset(BOOTSTRAP_PRESET).execute_with(|| {
        System::set_block_number(1);
        let op = K::Alice; // placeholder operator
        assert_eq!(free(op), BOOTSTRAP_ALLOCATION);
        assert_eq!(Balances::total_issuance(), BOOTSTRAP_PRE_ISSUED);
        assert_eq!(
            pallet_ict_emission::TotalIssued::<Runtime>::get(),
            BOOTSTRAP_PRE_ISSUED
        );

        // others cannot register; the operator registers its own validator and two miners
        assert!(Registry::register_neuron(signed(K::Bob), 0, acc(K::Bob)).is_err());
        let (v, m1, m2) = (K::Ferdie, K::Eve, K::Dave);
        for k in [v, m1, m2] {
            assert_ok!(Registry::register_neuron(signed(op), 0, acc(k)));
        }
        // the validator stakes 10 ICT, below the normal 1,000 ICT minimum
        assert_ok!(Balances::transfer_allow_death(
            signed(op),
            acc(v).into(),
            20 * UNIT
        ));
        assert_ok!(Consensus::stake(signed(v), 10 * UNIT));

        let salt = [3u8; 32];
        let (uids, w) = (vec![1u16, 2], vec![1u16, 1]);
        assert_ok!(Weights::commit_weights(
            signed(v),
            0,
            Weights::commit_hash(0, &uids, &w, &salt)
        ));
        run_to(EPOCH_LENGTH);
        assert_ok!(Weights::reveal_weights(signed(v), 0, uids, w, salt));
        run_to(2 * EPOCH_LENGTH);

        // the whole pot was earned by the operator's neurons: 25 % paid at once, 75 % locked
        let issued_since =
            pallet_ict_emission::TotalIssued::<Runtime>::get() - BOOTSTRAP_PRE_ISSUED;
        let pot = issued_since - issued_since / 10;
        let locked = |k: K| pallet_ict_consensus::LaunchLocked::<Runtime>::get(acc(k));
        let got = locked(m1) + locked(m2) + locked(v);
        assert!(got.abs_diff(pot * 3 / 4) <= 3, "{got} vs {}", pot * 3 / 4);
        let paid_m1 = free(m1);
        assert!(
            paid_m1.abs_diff(locked(m1) / 3) <= 3,
            "{paid_m1} vs {}",
            locked(m1) / 3
        );

        // after 90 days registration opens to everyone, with the normal fee
        System::set_block_number(BOOTSTRAP_BLOCKS);
        assert!(!Registry::in_bootstrap());
        assert_ok!(Balances::transfer_allow_death(
            signed(op),
            acc(K::Bob).into(),
            10 * UNIT
        ));
        assert_ok!(Registry::register_neuron(signed(K::Bob), 0, acc(K::Bob)));

        // the locked part: first 10 % at 2 months after the period (day 150), then 10 % every
        // 2 months, everything at 20 months
        let month = 30 * DAYS;
        System::set_block_number(BOOTSTRAP_BLOCKS + 2 * month - 1);
        assert!(Consensus::claim_launch_rewards(signed(m1)).is_err());
        System::set_block_number(BOOTSTRAP_BLOCKS + 2 * month);
        assert_ok!(Consensus::claim_launch_rewards(signed(m1)));
        assert!((free(m1) - paid_m1).abs_diff(locked(m1) / 10) <= 1);
        System::set_block_number(BOOTSTRAP_BLOCKS + 20 * month);
        assert_ok!(Consensus::claim_launch_rewards(signed(m1)));
        assert_eq!(free(m1) - paid_m1, locked(m1));
    });
}

#[test]
fn ordinary_fees_are_a_fraction_of_an_ict() {
    use frame_support::dispatch::GetDispatchInfo;
    ext_from_preset(sp_genesis_builder::DEV_RUNTIME_PRESET).execute_with(|| {
        let call = RuntimeCall::Balances(pallet_balances::Call::transfer_keep_alive {
            dest: acc(K::Bob).into(),
            value: UNIT,
        });
        let fee = pallet_transaction_payment::Pallet::<Runtime>::compute_fee(
            150,
            &call.get_dispatch_info(),
            0,
        );
        assert!(fee > 0, "fee must not be zero");
        assert!(
            fee < UNIT / 100,
            "transfer fee {fee} should be below 0.01 ICT"
        );
    });
}

// ---------------------------------------------------------------- governance and the fund

#[test]
fn issuance_share_goes_to_the_treasury() {
    ext_from_preset(sp_genesis_builder::DEV_RUNTIME_PRESET).execute_with(|| {
        System::set_block_number(1);
        assert_eq!(
            Treasury::account_id(),
            configs::FundAccount::get(),
            "the treasury must be the fund account ict-emission pays into"
        );
        let before = Balances::free_balance(Treasury::account_id());
        run_to(10);
        let fund_share = Balances::free_balance(Treasury::account_id()) - before;
        assert_eq!(fund_share, 72 * UNIT / 100, "8 % of 9 blocks (2 % goes to the block producers)");
    });
}

#[test]
fn tracks_are_consistent_and_spenders_are_capped() {
    use frame_support::traits::{EnsureOrigin, OriginTrait};
    use governance::{CustomOrigin, TreasurySpender, SMALL_SPEND};
    use pallet_referenda::TracksInfo as _;
    assert!(governance::TracksInfo::check_integrity().is_ok());
    let root: OriginCaller = frame_system::RawOrigin::Root.into();
    assert_eq!(governance::TracksInfo::track_for(&root), Ok(0));
    let small: OriginCaller = CustomOrigin::SmallSpender.into();
    assert_eq!(governance::TracksInfo::track_for(&small), Ok(2));
    let signed_caller = RuntimeOrigin::signed(acc(K::Alice));
    assert!(governance::TracksInfo::track_for(signed_caller.caller()).is_err());
    assert_eq!(
        TreasurySpender::try_origin(RuntimeOrigin::from(CustomOrigin::SmallSpender)).ok(),
        Some(SMALL_SPEND)
    );
    assert_eq!(
        TreasurySpender::try_origin(RuntimeOrigin::root()).ok(),
        Some(Balance::MAX)
    );
    assert!(TreasurySpender::try_origin(RuntimeOrigin::signed(acc(K::Alice))).is_err());
}

#[test]
fn launch_vault_and_fund_cannot_vote() {
    ext_from_preset(sp_genesis_builder::DEV_RUNTIME_PRESET).execute_with(|| {
        System::set_block_number(1);
        use frame_support::traits::Get;
        let votable = governance::VotableIssuance::get();
        let vault: AccountId = configs::LaunchPalletId::get().into_account_truncating();
        assert_ok!(Balances::force_set_balance(
            RuntimeOrigin::root(),
            vault.into(),
            1_000 * UNIT
        ));
        assert_eq!(governance::VotableIssuance::get(), votable);
    });
}

#[test]
fn launch_end_creates_the_disclosed_bounties_and_removes_sudo() {
    use genesis_config_presets::{BOOTSTRAP_PRESET, LISTING_BOUNTY, MARKETING_BOUNTY};
    use pallet_bounties::BountyStatus;
    ext_from_preset(BOOTSTRAP_PRESET).execute_with(|| {
        System::set_block_number(1);
        let op = K::Alice; // placeholder operator and curator
        assert_eq!(pallet_sudo::Key::<Runtime>::get(), Some(acc(op)));
        // The fund has what it would hold around day 90 (about 64,800 ICT).
        let fund = Treasury::account_id();
        assert_ok!(Balances::force_set_balance(
            RuntimeOrigin::root(),
            fund.clone().into(),
            64_800 * UNIT
        ));

        // Nothing happens while the launch period runs.
        Launch::on_initialize(2);
        assert!(!governance::pallet_launch::Done::<Runtime>::get());

        // The operator ends the period early; the next block runs the end-of-launch steps.
        System::set_block_number(100);
        assert_ok!(Registry::end_bootstrap(signed(op)));
        System::set_block_number(101);
        Launch::on_initialize(101);
        assert!(governance::pallet_launch::Done::<Runtime>::get());
        assert_eq!(pallet_sudo::Key::<Runtime>::get(), None, "sudo removed");
        assert!(Sudo::sudo(
            signed(op),
            Box::new(RuntimeCall::System(SystemCall::remark { remark: vec![] }))
        )
        .is_err());
        assert_eq!(pallet_bounties::BountyCount::<Runtime>::get(), 2);
        let status = |i| {
            pallet_bounties::Bounties::<Runtime>::get(i)
                .unwrap()
                .get_status()
        };
        assert_eq!(
            status(0),
            BountyStatus::ApprovedWithCurator { curator: acc(op) }
        );
        assert_eq!(
            pallet_bounties::Bounties::<Runtime>::get(0).unwrap().value,
            MARKETING_BOUNTY
        );
        assert_eq!(
            pallet_bounties::Bounties::<Runtime>::get(1).unwrap().value,
            LISTING_BOUNTY
        );
        // It runs once only.
        Launch::on_initialize(102);
        assert_eq!(pallet_bounties::BountyCount::<Runtime>::get(), 2);

        // At the next spend period the fund pays the marketing bounty; the listing bounty
        // waits until the fund has 50,000 ICT more.
        System::set_block_number(DAYS);
        Treasury::on_initialize(DAYS);
        assert_eq!(
            status(0),
            BountyStatus::CuratorProposed { curator: acc(op) }
        );
        assert!(matches!(
            status(1),
            BountyStatus::ApprovedWithCurator { .. }
        ));
        assert_eq!(
            Balances::free_balance(Bounties::bounty_account_id(0)),
            MARKETING_BOUNTY
        );

        // The curator accepts and splits the bounty into a child bounty for a creator.
        assert_ok!(Bounties::accept_curator(signed(op), 0));
        assert_ok!(ChildBounties::add_child_bounty(
            signed(op),
            0,
            300 * UNIT,
            b"Video: how to mine ICT".to_vec()
        ));
        assert_ok!(ChildBounties::propose_curator(
            signed(op),
            0,
            0,
            acc(op).into(),
            0
        ));
        assert_ok!(ChildBounties::accept_curator(signed(op), 0, 0));
        assert_ok!(ChildBounties::award_child_bounty(
            signed(op),
            0,
            0,
            acc(K::Eve).into()
        ));
        // Paid only after the 3-day delay, during which governance can step in.
        assert!(ChildBounties::claim_child_bounty(signed(K::Eve), 0, 0).is_err());
        System::set_block_number(DAYS + 3 * DAYS + 1);
        let before = free(K::Eve);
        assert_ok!(ChildBounties::claim_child_bounty(signed(K::Eve), 0, 0));
        assert_eq!(free(K::Eve) - before, 300 * UNIT);

        // Later the fund grows and the listing bounty is funded too.
        assert_ok!(Balances::force_set_balance(
            RuntimeOrigin::root(),
            fund.clone().into(),
            60_000 * UNIT
        ));
        System::set_block_number(5 * DAYS);
        Treasury::on_initialize(5 * DAYS);
        assert_eq!(
            status(1),
            BountyStatus::CuratorProposed { curator: acc(op) }
        );
    });
}

#[test]
fn devnet_without_a_launch_period_never_runs_the_end_of_launch() {
    ext_from_preset(sp_genesis_builder::LOCAL_TESTNET_RUNTIME_PRESET).execute_with(|| {
        System::set_block_number(1_000_000);
        Launch::on_initialize(1_000_000);
        assert!(!governance::pallet_launch::Done::<Runtime>::get());
        assert!(pallet_sudo::Key::<Runtime>::get().is_some());
    });
}

// ---------------------------------------------------------------- EVM (pallet-revive)

#[test]
fn evm_config_passes_the_pallet_integrity_test() {
    use frame_support::traits::Get;
    ext_from_preset(sp_genesis_builder::DEV_RUNTIME_PRESET).execute_with(|| {
        <Revive as Hooks<BlockNumber>>::integrity_test();
        assert_eq!(
            <<Runtime as pallet_revive::Config>::ChainId as Get<u64>>::get(),
            configs::EVM_CHAIN_ID
        );
    });
}

#[test]
fn weight_fee_is_unchanged_by_the_evm_fee_type() {
    use frame_support::weights::{Weight, WeightToFee};
    // Same as the old IctWeightToFee: ref_time / 100. Proof size adds nothing noticeable.
    let w = Weight::from_parts(200_000_000, 10_000);
    assert_eq!(configs::IctWeightToFee::weight_to_fee(&w), 2_000_000);
}

#[test]
fn mapped_substrate_account_has_an_evm_address_and_18_decimal_balance() {
    use pallet_revive::AddressMapper;
    ext_from_preset(sp_genesis_builder::DEV_RUNTIME_PRESET).execute_with(|| {
        System::set_block_number(1);
        type Mapper = <Runtime as pallet_revive::Config>::AddressMapper;
        assert!(!Mapper::is_mapped(&acc(K::Alice)));
        assert_ok!(Revive::map_account(signed(K::Alice)));
        assert!(Mapper::is_mapped(&acc(K::Alice)));

        let addr = Mapper::to_address(&acc(K::Alice));
        assert_eq!(Mapper::to_account_id(&addr), acc(K::Alice));

        // 1 ICT (10^9 base units) shows as 10^18 wei in the EVM.
        assert_eq!(
            Revive::convert_native_to_evm(UNIT),
            sp_core::U256::from(1_000_000_000_000_000_000u128)
        );
        // Alice's spendable ICT is visible from the EVM side.
        let evm = Revive::evm_balance(&addr);
        assert!(evm > sp_core::U256::zero());
        assert!(evm <= Revive::convert_native_to_evm(Balances::free_balance(acc(K::Alice))));
    });
}

#[test]
fn upgrade_creates_the_revive_account_from_the_fund_without_minting() {
    use frame_support::traits::OnRuntimeUpgrade;
    ext_from_preset(sp_genesis_builder::DEV_RUNTIME_PRESET).execute_with(|| {
        System::set_block_number(1);
        let revive = pallet_revive::Pallet::<Runtime>::account_id();
        let fund = pallet_treasury::Pallet::<Runtime>::account_id();
        // Top up the fund, then simulate a chain that started before the EVM existed.
        assert_ok!(Balances::transfer_keep_alive(
            signed(K::Alice),
            fund.clone().into(),
            10 * UNIT
        ));
        frame_system::Account::<Runtime>::remove(&revive);
        assert!(!System::account_exists(&revive));

        let fund_before = Balances::free_balance(&fund);
        let issuance_before = Balances::total_issuance();
        CreateReviveAccount::on_runtime_upgrade();

        assert!(System::account_exists(&revive));
        assert_eq!(Balances::free_balance(&revive), EXISTENTIAL_DEPOSIT);
        assert_eq!(
            Balances::free_balance(&fund),
            fund_before - EXISTENTIAL_DEPOSIT
        );
        assert_eq!(Balances::total_issuance(), issuance_before);

        // Running it again does nothing.
        CreateReviveAccount::on_runtime_upgrade();
        assert_eq!(Balances::free_balance(&revive), EXISTENTIAL_DEPOSIT);
    });
}

#[test]
fn subnet_unlock_sees_sudo_and_approved_referenda() {
    use genesis_config_presets::BOOTSTRAP_PRESET;
    use pallet_subnet_registry::UnlockSignals;
    type S = governance::GovernanceSignals;
    ext_from_preset(BOOTSTRAP_PRESET).execute_with(|| {
        System::set_block_number(1);
        assert!(!S::sudo_removed());
        assert_eq!(S::approved_referenda(3), 0);
        let status = Registry::unlock_status();
        assert!(!status.unlocked && !status.sudo_removed && !status.swap_attested);
        assert_eq!(status.min_block, 180 * DAYS);

        governance::pallet_launch::Pallet::<Runtime>::end_launch();
        assert!(S::sudo_removed());

        // Three approved referenda (as the referenda pallet stores them) are counted.
        for i in 0..3u32 {
            pallet_referenda::ReferendumInfoFor::<Runtime>::insert(
                i,
                pallet_referenda::ReferendumInfo::Approved(1, None, None),
            );
        }
        pallet_referenda::ReferendumInfoFor::<Runtime>::insert(
            3,
            pallet_referenda::ReferendumInfo::Rejected(1, None, None),
        );
        assert_eq!(S::approved_referenda(10), 3);
        assert_eq!(S::approved_referenda(2), 2);
        // Still locked: day 180, the miners and the swap attestation are missing.
        assert!(!Registry::criteria_met(&Registry::unlock_status()));
    });
}

#[test]
fn sn0_keeps_half_of_all_issuance_whatever_the_fund_share() {
    use frame_support::traits::Get;
    ext_from_preset(sp_genesis_builder::DEV_RUNTIME_PRESET).execute_with(|| {
        // 10 % fund: 0.5 / 0.9 of the pot.
        assert_eq!(configs::Sn0MinPotPpm::get(), 555_556);
        assert_ok!(Emission::set_fund_share(RuntimeOrigin::root(), 200_000));
        assert_eq!(configs::Sn0MinPotPpm::get(), 625_000);
        assert_ok!(Emission::set_fund_share(RuntimeOrigin::root(), 0));
        assert_eq!(configs::Sn0MinPotPpm::get(), 500_000);
    });
}

#[test]
fn subnet_multisig_account_matches_pallet_multisig() {
    ext_from_preset(sp_genesis_builder::DEV_RUNTIME_PRESET).execute_with(|| {
        let mut who = vec![acc(K::Charlie), acc(K::Alice), acc(K::Bob)];
        who.sort();
        assert_eq!(
            Registry::multi_account_id(&who, 2),
            pallet_multisig::Pallet::<Runtime>::multi_account_id(&who, 2)
        );
    });
}

#[test]
fn subnet_api_reports_status_auction_and_sn0() {
    ext_from_preset(sp_genesis_builder::DEV_RUNTIME_PRESET).execute_with(|| {
        System::set_block_number(10);
        let st = Registry::unlock_status();
        assert!(!st.unlocked);
        assert_eq!(st.min_miners, 300);
        let a = Registry::auction_info();
        assert_eq!(a.price, None);
        assert_eq!(a.start_price, 50_000 * UNIT);
        assert_eq!(a.floor, 1_000 * UNIT);
        assert_eq!(a.max_subnets, 8);
        let subnets = subnet_api::subnets_with_emission();
        assert_eq!(subnets.len(), 1);
        assert_eq!(subnets[0].netuid, 0);
        // Before the unlock SN0 gets the whole pot: 90 % of issuance.
        assert_eq!(subnets[0].emission_ppm, 900_000);
    });
}

/// Writes the API's SCALE output for the page's decoder test when INCENTIUM_API_DUMP names a
/// folder (see site/subnets/test_decode.js).
#[test]
fn subnet_api_output_for_the_page() {
    use codec::Encode;
    ext_from_preset(sp_genesis_builder::DEV_RUNTIME_PRESET).execute_with(|| {
        System::set_block_number(200 * DAYS);
        pallet_subnet_registry::Phase2Enabled::<Runtime>::put(true);
        pallet_subnet_registry::UnlockedAt::<Runtime>::put(200 * DAYS);
        pallet_subnet_registry::AuctionStart::<Runtime>::put(200 * DAYS);
        assert_ok!(Registry::register_subnet(
            signed(K::Alice),
            u128::MAX,
            b"Translate".to_vec(),
            b"Open translation".to_vec(),
            [7; 32]
        ));
        let status = Registry::unlock_status();
        let auction = Registry::auction_info();
        let price = Registry::price_at(200 * DAYS + 60 * DAYS + DAYS);
        let subnets = subnet_api::subnets_with_emission();
        assert_eq!(subnets.len(), 2);
        assert_eq!(subnets[1].name, b"Translate".to_vec());
        if let Ok(dir) = std::env::var("INCENTIUM_API_DUMP") {
            let w = |n: &str, b: Vec<u8>| {
                std::fs::write(
                    format!("{dir}/{n}.hex"),
                    b.iter()
                        .fold(String::from("0x"), |s, x| s + &format!("{x:02x}")),
                )
                .unwrap()
            };
            w("unlock_status", status.encode());
            w("auction", auction.encode());
            w("price_at", price.encode());
            w("subnets", subnets.encode());
        }
    });
}

#[test]
fn emergency_origin_can_only_authorize_an_upgrade() {
    use frame_support::traits::EnsureOrigin;
    use governance::{CustomOrigin, TreasurySpender};
    use pallet_referenda::TracksInfo as _;
    ext_from_preset(sp_genesis_builder::DEV_RUNTIME_PRESET).execute_with(|| {
        System::set_block_number(1);
        let em: OriginCaller = CustomOrigin::Emergency.into();
        assert_eq!(governance::TracksInfo::track_for(&em), Ok(3));
        assert!(TreasurySpender::try_origin(RuntimeOrigin::from(CustomOrigin::Emergency)).is_err());
        let hash = sp_core::H256::repeat_byte(7);
        assert!(Emergency::authorize_upgrade(signed(K::Alice), hash).is_err());
        assert!(Emergency::authorize_upgrade(RuntimeOrigin::root(), hash).is_err());
        assert_ok!(Emergency::authorize_upgrade(
            RuntimeOrigin::from(CustomOrigin::Emergency),
            hash
        ));
        assert_eq!(
            System::authorized_upgrade().map(|a| *a.code_hash()),
            Some(hash)
        );
    });
}

#[test]
fn an_emergency_referendum_authorizes_an_upgrade_within_hours() {
    use frame_support::traits::{schedule::DispatchTime, StorePreimage};
    use pallet_conviction_voting::{AccountVote, Conviction, Vote};
    ext_from_preset(sp_genesis_builder::DEV_RUNTIME_PRESET).execute_with(|| {
        System::set_block_number(1);
        let hash = sp_core::H256::repeat_byte(9);
        let call = RuntimeCall::Emergency(governance::pallet_emergency::Call::authorize_upgrade {
            code_hash: hash,
        });
        let proposal = <Preimage as StorePreimage>::bound(call).unwrap();
        assert!(matches!(
            proposal,
            frame_support::traits::Bounded::Inline(_)
        ));
        let origin: OriginCaller = governance::CustomOrigin::Emergency.into();
        assert_ok!(Referenda::submit(
            signed(K::Alice),
            Box::new(origin),
            proposal,
            DispatchTime::After(0)
        ));
        assert_ok!(Referenda::place_decision_deposit(signed(K::Alice), 0));
        for k in [K::Alice, K::Bob, K::Charlie, K::Dave, K::Eve, K::Ferdie] {
            let bal = Balances::free_balance(acc(k));
            if bal < 2 * UNIT {
                continue;
            }
            assert_ok!(ConvictionVoting::vote(
                signed(k),
                0,
                AccountVote::Standard {
                    vote: Vote {
                        aye: true,
                        conviction: Conviction::Locked1x
                    },
                    balance: bal - UNIT,
                }
            ));
        }
        let start = System::block_number();
        let mut done_at = None;
        for _ in 0..(2 * DAYS) {
            let b = System::block_number() + 1;
            System::set_block_number(b);
            Scheduler::on_initialize(b);
            if System::authorized_upgrade().is_some() {
                done_at = Some(b);
                break;
            }
        }
        let at = done_at.expect("the emergency referendum did not pass");
        // Prepare 1 h + confirm 3 h + enactment 1 h, plus alarm scheduling: about 5 h 8 min.
        assert!(
            at - start <= 5 * HOURS + HOURS / 2,
            "took {} blocks",
            at - start
        );
        assert_eq!(
            System::authorized_upgrade().map(|a| *a.code_hash()),
            Some(hash)
        );
    });
}

/// Raw storage of a real referendum, shared with scripts/test_governance_page.js so the
/// governance page decodes exactly what the chain stores.
#[test]
fn governance_page_fixture() {
    use frame_support::traits::{Bounded, StorePreimage};
    use pallet_conviction_voting::{AccountVote, Conviction, Vote};
    ext_from_preset(sp_genesis_builder::DEV_RUNTIME_PRESET).execute_with(|| {
        System::set_block_number(1);
        let call = RuntimeCall::System(frame_system::Call::authorize_upgrade {
            code_hash: sp_core::H256::repeat_byte(7),
        });
        let proposal = <Preimage as StorePreimage>::bound(call).unwrap();
        assert!(matches!(proposal, Bounded::Inline(_)));
        assert_ok!(Referenda::submit(
            signed(K::Alice),
            Box::new(frame_system::RawOrigin::Root.into()),
            proposal,
            frame_support::traits::schedule::DispatchTime::After(100)
        ));
        assert_ok!(Referenda::place_decision_deposit(signed(K::Bob), 0));
        assert_ok!(ConvictionVoting::vote(
            signed(K::Charlie),
            0,
            AccountVote::Standard {
                vote: Vote { aye: true, conviction: Conviction::Locked1x },
                balance: 5_000 * UNIT,
            }
        ));
        let key = pallet_referenda::ReferendumInfoFor::<Runtime>::hashed_key_for(0u32);
        let raw = frame_support::storage::unhashed::get_raw(&key).unwrap();
        let hex = |b: &[u8]| format!("0x{}", sp_core::hexdisplay::HexDisplay::from(&b.to_vec()));
        println!("KEY={}\nVALUE={}", hex(&key), hex(&raw));
        let fund = configs::FundAccount::get();
        println!("FUNDKEY={}", hex(&frame_system::Account::<Runtime>::hashed_key_for(&fund)));
    });
}

// ---------------------------------------------------------------- parameters track

#[test]
fn the_parameters_track_changes_listed_parameters_only() {
    ext_from_preset(sp_genesis_builder::DEV_RUNTIME_PRESET).execute_with(|| {
        use governance::CustomOrigin;
        let origin = || RuntimeOrigin::from(CustomOrigin::Parameters);
        let set = |c: RuntimeCall| Parameters::set(origin(), Box::new(c));
        // listed: carried out with Root
        assert_ok!(set(RuntimeCall::Emission(pallet_ict_emission::Call::set_validator_share { ppm: 10_000 })));
        assert_eq!(pallet_ict_emission::ValidatorShare::<Runtime>::get(), 10_000);
        assert_ok!(set(RuntimeCall::Staking(pallet_staking::Call::set_validator_count { new: 30 })));
        assert_eq!(pallet_staking::ValidatorCount::<Runtime>::get(), 30);
        // the pallet's own bounds still hold
        assert!(set(RuntimeCall::Emission(pallet_ict_emission::Call::set_fund_share { ppm: 900_000 })).is_err());
        // not listed: refused
        for c in [
            RuntimeCall::System(frame_system::Call::authorize_upgrade { code_hash: sp_core::H256::zero() }),
            RuntimeCall::Registry(pallet_subnet_registry::Call::deregister_subnet { netuid: 1 }),
            RuntimeCall::BlockProducers(pallet_block_producers::Call::end_reserve {}),
            RuntimeCall::Balances(pallet_balances::Call::force_set_balance { who: acc(K::Bob).into(), new_free: 1 }),
        ] {
            assert_eq!(
                set(c),
                Err(governance::pallet_parameters::Error::<Runtime>::NotAParameter.into())
            );
        }
        // other origins cannot use it
        assert!(Parameters::set(signed(K::Alice), Box::new(RuntimeCall::Staking(pallet_staking::Call::set_validator_count { new: 5 }))).is_err());
        assert!(Parameters::set(RuntimeOrigin::from(CustomOrigin::Emergency), Box::new(RuntimeCall::Staking(pallet_staking::Call::set_validator_count { new: 5 }))).is_err());
    });
}

#[test]
fn a_parameters_referendum_passes_and_takes_effect() {
    use frame_support::traits::{schedule::DispatchTime, StorePreimage};
    use pallet_conviction_voting::{AccountVote, Conviction, Vote};
    ext_from_preset(sp_genesis_builder::DEV_RUNTIME_PRESET).execute_with(|| {
        System::set_block_number(1);
        let call = RuntimeCall::Parameters(governance::pallet_parameters::Call::set {
            call: Box::new(RuntimeCall::Staking(pallet_staking::Call::set_validator_count { new: 33 })),
        });
        let proposal = <Preimage as StorePreimage>::bound(call).unwrap();
        let origin: OriginCaller = governance::CustomOrigin::Parameters.into();
        assert_ok!(Referenda::submit(signed(K::Alice), Box::new(origin), proposal, DispatchTime::After(0)));
        assert_eq!(
            pallet_referenda::ReferendumInfoFor::<Runtime>::get(0).map(|i| match i {
                pallet_referenda::ReferendumInfo::Ongoing(s) => s.track,
                _ => 99,
            }),
            Some(governance::TRACK_PARAMETERS)
        );
        assert_ok!(Referenda::place_decision_deposit(signed(K::Alice), 0));
        for k in [K::Alice, K::Bob, K::Charlie, K::Dave, K::Eve, K::Ferdie] {
            let bal = Balances::free_balance(acc(k));
            if bal < 2 * UNIT {
                continue;
            }
            assert_ok!(ConvictionVoting::vote(
                signed(k),
                0,
                AccountVote::Standard { vote: Vote { aye: true, conviction: Conviction::Locked1x }, balance: bal - UNIT }
            ));
        }
        for _ in 0..(4 * DAYS) {
            let b = System::block_number() + 1;
            System::set_block_number(b);
            Scheduler::on_initialize(b);
            if pallet_staking::ValidatorCount::<Runtime>::get() == 33 {
                break;
            }
        }
        assert_eq!(pallet_staking::ValidatorCount::<Runtime>::get(), 33, "the referendum took effect");
        // prepare 1 day + confirm 1 day + enactment 1 day, plus scheduling
        assert!(System::block_number() <= 3 * DAYS + DAYS / 2, "took {} blocks", System::block_number());
    });
}

/// Metadata and the exact calls the governance page's wallet panel builds, for
/// scripts/test_wallet_calls.js.
#[test]
fn wallet_calls_fixture() {
    use codec::Encode;
    use pallet_conviction_voting::{AccountVote, Conviction, Vote};
    ext_from_preset(sp_genesis_builder::DEV_RUNTIME_PRESET).execute_with(|| {
    let md = Runtime::metadata_at_version(15).expect("v15");
    std::fs::write("/tmp/ict_metadata.bin", &md[..]).unwrap();
    let vote = RuntimeCall::ConvictionVoting(pallet_conviction_voting::Call::vote {
        poll_index: 3,
        vote: AccountVote::Standard { vote: Vote { aye: true, conviction: Conviction::Locked2x }, balance: 100 * UNIT + 5 },
    });
    let nay = RuntimeCall::ConvictionVoting(pallet_conviction_voting::Call::vote {
        poll_index: 3,
        vote: AccountVote::Standard { vote: Vote { aye: false, conviction: Conviction::None }, balance: UNIT },
    });
    let remove = RuntimeCall::ConvictionVoting(pallet_conviction_voting::Call::remove_vote { class: None, index: 3 });
    let dep = RuntimeCall::Referenda(pallet_referenda::Call::place_decision_deposit { index: 3 });
    let params = RuntimeCall::Parameters(governance::pallet_parameters::Call::set {
        call: Box::new(RuntimeCall::Staking(pallet_staking::Call::set_validator_count { new: 33 })),
    });
    let out: Vec<String> = [vote, nay, remove, dep, params].iter().map(|c| sp_core::hexdisplay::HexDisplay::from(&c.encode()).to_string()).collect();
    std::fs::write("/tmp/ict_calls.txt", out.join("\n")).unwrap();
    });
}

#[test]
fn subnet_pools_are_in_the_runtime_and_empty_until_a_subnet_is_sold() {
    ext_from_preset(sp_genesis_builder::DEV_RUNTIME_PRESET).execute_with(|| {
        assert!(crate::SubnetPools::summaries().is_empty());
        assert_eq!(crate::SubnetPools::quote_stake(1, UNIT), None);
        // SN0 has no Alpha: nothing can be staked there through the pools
        assert!(crate::SubnetPools::stake(signed(K::Alice), 0, acc(K::Alice), UNIT, 0).is_err());
        assert_eq!(
            <crate::SubnetPools as polkadot_sdk::frame_support::traits::PalletInfoAccess>::index(),
            32
        );
    });
}
