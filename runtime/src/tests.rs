//! End-to-end runtime test of the Phase 1 loop on SN0:
//! issuance -> neurons and stake -> pool -> commit/reveal -> epoch settlement -> pool payout.

use crate::*;
use frame_support::{assert_ok, traits::Hooks};
use sp_core::H256;
use sp_keyring::Sr25519Keyring as K;

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
            issued / 10
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
    use genesis_config_presets::{BOOTSTRAP_ALLOCATION, BOOTSTRAP_BLOCKS, BOOTSTRAP_PRESET};
    ext_from_preset(BOOTSTRAP_PRESET).execute_with(|| {
        System::set_block_number(1);
        let op = K::Alice; // placeholder operator
        assert_eq!(free(op), BOOTSTRAP_ALLOCATION);
        assert_eq!(Balances::total_issuance(), BOOTSTRAP_ALLOCATION);
        assert_eq!(
            pallet_ict_emission::TotalIssued::<Runtime>::get(),
            BOOTSTRAP_ALLOCATION
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

        // the whole pot was earned by the operator's neurons and is locked in the vault
        let issued_since =
            pallet_ict_emission::TotalIssued::<Runtime>::get() - BOOTSTRAP_ALLOCATION;
        let pot = issued_since - issued_since / 10;
        let locked = |k: K| pallet_ict_consensus::LaunchLocked::<Runtime>::get(acc(k));
        let got = locked(m1) + locked(m2) + locked(v);
        assert!(got.abs_diff(pot) <= 3, "{got} vs {pot}");
        assert_eq!(free(m1), 0);

        // after 90 days registration opens to everyone, with the normal fee
        System::set_block_number(BOOTSTRAP_BLOCKS);
        assert!(!Registry::in_bootstrap());
        assert_ok!(Balances::transfer_allow_death(
            signed(op),
            acc(K::Bob).into(),
            10 * UNIT
        ));
        assert_ok!(Registry::register_neuron(signed(K::Bob), 0, acc(K::Bob)));

        // locked for 2 months after the period; 10 % at 4 months; everything at 22 months
        let month = 30 * DAYS;
        System::set_block_number(BOOTSTRAP_BLOCKS + 4 * month - 1);
        assert!(Consensus::claim_launch_rewards(signed(m1)).is_err());
        System::set_block_number(BOOTSTRAP_BLOCKS + 4 * month);
        assert_ok!(Consensus::claim_launch_rewards(signed(m1)));
        assert!(free(m1).abs_diff(locked(m1) / 10) <= 1);
        System::set_block_number(BOOTSTRAP_BLOCKS + 22 * month);
        assert_ok!(Consensus::claim_launch_rewards(signed(m1)));
        assert_eq!(free(m1), locked(m1));
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
