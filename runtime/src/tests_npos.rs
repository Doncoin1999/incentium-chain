//! Open block production (docs/OPEN-BLOCK-PRODUCTION.md): election, genesis reserve, offline
//! rule, rewards from the pot, minimum slash and the live-chain migration.

use crate::block_production::*;
use crate::*;
use codec::Encode;
use frame_support::{assert_ok, traits::{Hooks, OnRuntimeUpgrade}};
use sp_keyring::{Ed25519Keyring as E, Sr25519Keyring as K};

const ERA: BlockNumber = 4 * 1_800;

fn ext() -> sp_io::TestExternalities {
    crate::tests::ext_from_preset(sp_genesis_builder::DEV_RUNTIME_PRESET)
}

fn run_to(n: BlockNumber) {
    run_with_author(n, None)
}

fn run_with_author(n: BlockNumber, online: Option<AccountId>) {
    run_with_authors(n, online.into_iter().collect())
}

/// Runs blocks; each of `online` writes a block whenever it is in the session set.
fn run_with_authors(n: BlockNumber, online: Vec<AccountId>) {
    while System::block_number() < n {
        let b = System::block_number() + 1;
        System::set_block_number(b);
        pallet_timestamp::Now::<Runtime>::put(b as u64 * 12_000);
        Emission::on_initialize(b);
        Session::on_initialize(b);
        let set = Session::validators();
        for who in online.iter().filter(|w| set.contains(w)) {
            <Staking as pallet_authorship::EventHandler<AccountId, BlockNumber>>::note_author(who.clone());
        }
        Staking::on_finalize(b);
    }
}

fn acc(k: K) -> AccountId {
    k.to_account_id()
}

/// Registers session keys for `k` without the signed ownership proof (test only).
fn keys_for(k: K, e: E) {
    let who = acc(k);
    let keys = SessionKeys { aura: k.public().into(), grandpa: e.public().into() };
    pallet_session::NextKeys::<Runtime>::insert(&who, &keys);
    pallet_session::KeyOwner::<Runtime>::insert((sp_core::crypto::key_types::AURA, keys.aura.encode()), &who);
    pallet_session::KeyOwner::<Runtime>::insert((sp_core::crypto::key_types::GRANDPA, keys.grandpa.encode()), &who);
}

fn stand(k: K, e: E, bond: Balance) {
    keys_for(k, e);
    assert_ok!(Staking::bond(
        RuntimeOrigin::signed(acc(k)),
        bond,
        pallet_staking::RewardDestination::Stash
    ));
    assert_ok!(Staking::validate(RuntimeOrigin::signed(acc(k)), Default::default()));
}

fn alice_validator() -> AccountId {
    genesis_config_presets::validator_id(&K::Alice.public().into())
}

#[test]
fn genesis_starts_with_the_genesis_key_in_reserve_and_the_approved_parameters() {
    ext().execute_with(|| {
        assert_eq!(Session::validators(), vec![alice_validator()]);
        assert_eq!(pallet_block_producers::GenesisValidators::<Runtime>::get().into_inner(), vec![alice_validator()]);
        assert!(pallet_block_producers::ReserveActive::<Runtime>::get());
        assert_eq!(pallet_staking::ValidatorCount::<Runtime>::get(), 20);
        assert_eq!(pallet_staking::MinValidatorBond::<Runtime>::get(), 100 * UNIT);
        assert_eq!(pallet_staking::MinNominatorBond::<Runtime>::get(), 10 * UNIT);
        assert_eq!(pallet_aura::Authorities::<Runtime>::get().len(), 1);
    });
}

#[test]
fn a_candidate_below_the_minimum_bond_cannot_validate() {
    ext().execute_with(|| {
        assert_ok!(Staking::bond(RuntimeOrigin::signed(acc(K::Bob)), 50 * UNIT, pallet_staking::RewardDestination::Stash));
        assert!(Staking::validate(RuntimeOrigin::signed(acc(K::Bob)), Default::default()).is_err());
    });
}

#[test]
fn an_elected_candidate_joins_the_genesis_key_and_produces_blocks() {
    ext().execute_with(|| {
        stand(K::Bob, E::Bob, 1_000 * UNIT);
        assert_ok!(Staking::nominate(RuntimeOrigin::signed(acc(K::Charlie)), vec![sp_runtime::MultiAddress::Id(acc(K::Bob))]).or_else(|_| {
            Staking::bond(RuntimeOrigin::signed(acc(K::Charlie)), 50 * UNIT, pallet_staking::RewardDestination::Stash)?;
            Staking::nominate(RuntimeOrigin::signed(acc(K::Charlie)), vec![sp_runtime::MultiAddress::Id(acc(K::Bob))])
        }));
        run_with_author(2 * ERA + 1, Some(acc(K::Bob)));
        let set = Session::validators();
        assert!(set.contains(&acc(K::Bob)), "Bob elected: {set:?}");
        assert!(set.contains(&alice_validator()), "genesis key kept in reserve");
        let aura = pallet_aura::Authorities::<Runtime>::get();
        assert_eq!(aura.len(), 2);
    });
}

#[test]
fn a_validator_that_writes_no_block_in_an_era_is_chilled() {
    ext().execute_with(|| {
        stand(K::Bob, E::Bob, 1_000 * UNIT);
        run_to(2 * ERA + 1);
        assert!(Session::validators().contains(&acc(K::Bob)));
        // no digests in tests, so Bob gets no era points
        run_to(4 * ERA + 1);
        assert!(!pallet_staking::Validators::<Runtime>::contains_key(acc(K::Bob)));
        assert!(System::events().iter().any(|e| matches!(
            e.event,
            RuntimeEvent::BlockProducers(pallet_block_producers::Event::ChilledOffline { .. })
        )));
    });
}

#[test]
fn the_era_payout_is_the_pot_and_supply_is_unchanged() {
    ext().execute_with(|| {
        stand(K::Bob, E::Bob, 1_000 * UNIT);
        run_with_author(2 * ERA + 1, Some(acc(K::Bob)));
        let era = pallet_staking::ActiveEra::<Runtime>::get().unwrap().index;
        assert!(Session::validators().contains(&acc(K::Bob)));
        let pot_before_end = Balances::free_balance(ValidatorPot::get());
        assert!(pot_before_end > 0);
        let issuance_before = Balances::total_issuance();
        run_with_author(3 * ERA + 1, Some(acc(K::Bob)));
        let paid = pallet_staking::ErasValidatorReward::<Runtime>::get(era).expect("era paid");
        assert!(paid >= pot_before_end, "payout {paid} covers the pot {pot_before_end}");
        let bob_before = Balances::free_balance(acc(K::Bob));
        assert_ok!(Staking::payout_stakers(RuntimeOrigin::signed(acc(K::Dave)), acc(K::Bob), era));
        let bob_gain = Balances::free_balance(acc(K::Bob)) - bob_before;
        assert!(bob_gain > 0 && bob_gain <= paid);
        // the pot was burned and minted again: issuance grew only by new emission
        let emission = (3 * ERA - 2 * ERA) as u128 * UNIT;
        let diff = Balances::total_issuance() as i128 - (issuance_before + emission) as i128;
        assert!(diff.unsigned_abs() <= paid, "supply drift {diff}");
    });
}

#[test]
fn root_can_end_the_reserve_and_the_genesis_key_leaves_at_the_next_set() {
    ext().execute_with(|| {
        stand(K::Bob, E::Bob, 1_000 * UNIT);
        run_with_author(2 * ERA + 1, Some(acc(K::Bob)));
        assert_ok!(BlockProducers::end_reserve(RuntimeOrigin::root()));
        assert!(!pallet_block_producers::ReserveActive::<Runtime>::get());
        assert_eq!(pallet_staking::ValidatorCount::<Runtime>::get(), 21);
        assert!(BlockProducers::end_reserve(RuntimeOrigin::root()).is_err(), "never restarted");
        // Bob stays online, so he is not chilled
        run_with_author(5 * ERA + 1, Some(acc(K::Bob)));
        let set = Session::validators();
        let evs: Vec<_> = System::events().into_iter().filter_map(|e| match e.event {
            RuntimeEvent::BlockProducers(x) => Some(format!("{x:?}")),
            RuntimeEvent::Staking(x) => Some(format!("{x:?}")),
            _ => None }).collect();
        assert!(!set.contains(&alice_validator()), "genesis key gone: {set:?}\n{evs:#?}\nera {:?} current {:?}", pallet_staking::ActiveEra::<Runtime>::get(), pallet_staking::CurrentEra::<Runtime>::get());
        assert!(set.contains(&acc(K::Bob)));
    });
}

#[test]
fn grandpa_offences_are_slashed_at_least_ten_percent() {
    use sp_staking::offence::{OffenceDetails, OnOffenceHandler};
    struct Record;
    thread_local! { static SEEN: core::cell::RefCell<Vec<sp_runtime::Perbill>> = Default::default(); }
    impl OnOffenceHandler<AccountId, (AccountId, ()), frame_support::weights::Weight> for Record {
        fn on_offence(_: &[OffenceDetails<AccountId, (AccountId, ())>], f: &[sp_runtime::Perbill], _: u32) -> frame_support::weights::Weight {
            SEEN.with(|s| *s.borrow_mut() = f.to_vec());
            Default::default()
        }
    }
    type H = pallet_block_producers::MinSlash<Record, MinSlash>;
    let d = OffenceDetails { offender: (acc(K::Bob), ()), reporters: vec![] };
    <H as OnOffenceHandler<_, _, _>>::on_offence(
        &[d.clone(), d],
        &[sp_runtime::Perbill::from_percent(1), sp_runtime::Perbill::from_percent(30)],
        0,
    );
    SEEN.with(|s| assert_eq!(*s.borrow(), vec![sp_runtime::Perbill::from_percent(10), sp_runtime::Perbill::from_percent(30)]));
}

#[test]
fn the_migration_seeds_session_from_the_running_authorities() {
    ext().execute_with(|| {
        // make it look like a chain from before runtime 12
        pallet_session::Validators::<Runtime>::kill();
        pallet_session::QueuedKeys::<Runtime>::kill();
        let _ = pallet_session::NextKeys::<Runtime>::clear(u32::MAX, None);
        let _ = pallet_session::KeyOwner::<Runtime>::clear(u32::MAX, None);
        pallet_block_producers::GenesisValidators::<Runtime>::kill();
        pallet_block_producers::ReserveActive::<Runtime>::kill();
        pallet_staking::ValidatorCount::<Runtime>::kill();
        pallet_staking::MinValidatorBond::<Runtime>::kill();

        InitBlockProduction::on_runtime_upgrade();
        assert_eq!(Session::validators(), vec![alice_validator()]);
        assert!(pallet_session::NextKeys::<Runtime>::contains_key(alice_validator()));
        assert!(pallet_block_producers::ReserveActive::<Runtime>::get());
        assert_eq!(pallet_staking::ValidatorCount::<Runtime>::get(), 20);
        assert_eq!(pallet_staking::MinValidatorBond::<Runtime>::get(), 100 * UNIT);
        // runs once
        InitBlockProduction::on_runtime_upgrade();
        assert_eq!(Session::validators(), vec![alice_validator()]);
        // block production continues through session changes
        run_to(2 * ERA + 1);
        assert_eq!(
            pallet_aura::Authorities::<Runtime>::get().into_inner(),
            vec![K::Alice.public().into()]
        );
    });
}

#[test]
fn seven_independent_producers_for_thirty_eras_end_the_reserve_by_themselves() {
    ext().execute_with(|| {
        let seven = [
            (K::Bob, E::Bob),
            (K::Charlie, E::Charlie),
            (K::Dave, E::Dave),
            (K::Eve, E::Eve),
            (K::Ferdie, E::Ferdie),
            (K::BobStash, E::BobStash),
            (K::CharlieStash, E::CharlieStash),
        ];
        for (k, e) in seven {
            stand(k, e, 1_000 * UNIT);
        }
        let online: Vec<AccountId> = seven.iter().map(|(k, _)| acc(*k)).collect();
        // 29 qualifying eras already behind us; the next one with 7 independents completes it
        pallet_block_producers::Streak::<Runtime>::put(29);
        run_with_authors(2 * ERA + 1, online.clone());
        assert!(!pallet_block_producers::ReserveActive::<Runtime>::get(), "reserve ended");
        assert_eq!(pallet_staking::ValidatorCount::<Runtime>::get(), 21);
        run_with_authors(4 * ERA + 1, online.clone());
        let set = Session::validators();
        assert_eq!(set.len(), 7);
        assert!(!set.contains(&alice_validator()));
    });
}

#[test]
fn fewer_than_seven_independents_reset_the_streak() {
    ext().execute_with(|| {
        stand(K::Bob, E::Bob, 1_000 * UNIT);
        pallet_block_producers::Streak::<Runtime>::put(29);
        run_with_author(2 * ERA + 1, Some(acc(K::Bob)));
        assert_eq!(pallet_block_producers::Streak::<Runtime>::get(), 0);
        assert!(pallet_block_producers::ReserveActive::<Runtime>::get());
    });
}

#[test]
fn the_read_api_shows_candidates_the_reserve_and_how_many_could_halt_the_chain() {
    ext().execute_with(|| {
        stand(K::Bob, E::Bob, 1_000 * UNIT);
        run_with_author(2 * ERA + 1, Some(acc(K::Bob)));
        let o = BlockProducers::overview(7 * UNIT);
        assert_eq!((o.elected, o.producing, o.candidates), (1, 2, 1));
        assert!(o.reserve_active);
        assert_eq!((o.streak_needed, o.min_independent), (30, 7));
        assert_eq!(o.min_validator_bond, 100 * UNIT);
        assert_eq!(o.total_backing, 1_000 * UNIT);
        assert_eq!(o.to_halt, 1);
        assert_eq!(o.pot, 7 * UNIT);
        let v = BlockProducers::validators();
        let bob = v.iter().find(|x| x.stash == acc(K::Bob)).unwrap();
        assert!(bob.candidate && bob.producing && !bob.genesis);
        assert_eq!(bob.self_bond, 1_000 * UNIT);
        assert!(bob.points_this_era > 0);
        let alice = v.iter().find(|x| x.stash == alice_validator()).unwrap();
        assert!(alice.genesis && alice.producing && !alice.candidate);
    });
}
