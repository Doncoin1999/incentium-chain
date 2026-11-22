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

fn ext_from_preset(preset: &str) -> sp_io::TestExternalities {
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
            issued / 10 + EXISTENTIAL_DEPOSIT // the treasury opens the fund account with the ED
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
        assert_eq!(fund_share, 9 * UNIT / 10, "10 % of 9 blocks");
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
        assert_ok!(Balances::transfer_keep_alive(signed(K::Alice), fund.clone().into(), 10 * UNIT));
        frame_system::Account::<Runtime>::remove(&revive);
        assert!(!System::account_exists(&revive));

        let fund_before = Balances::free_balance(&fund);
        let issuance_before = Balances::total_issuance();
        CreateReviveAccount::on_runtime_upgrade();

        assert!(System::account_exists(&revive));
        assert_eq!(Balances::free_balance(&revive), EXISTENTIAL_DEPOSIT);
        assert_eq!(Balances::free_balance(&fund), fund_before - EXISTENTIAL_DEPOSIT);
        assert_eq!(Balances::total_issuance(), issuance_before);

        // Running it again does nothing.
        CreateReviveAccount::on_runtime_upgrade();
        assert_eq!(Balances::free_balance(&revive), EXISTENTIAL_DEPOSIT);
    });
}
