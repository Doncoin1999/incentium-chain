use crate::{
    mock::*, CurrentParams, Entries, Error, Kind, LastSettled, OwnerCount, Params, Tallies,
};
use polkadot_sdk::frame_support::{
    assert_noop, assert_ok,
    traits::fungible::{Inspect, InspectHold},
};
use polkadot_sdk::sp_runtime::DispatchError;

fn rpc(who: u64, url: &str) -> u32 {
    assert_ok!(Endpoints::register(
        RuntimeOrigin::signed(who),
        Kind::Rpc,
        url.as_bytes().to_vec(),
        b"eu".to_vec()
    ));
    crate::NextId::<Test>::get() - 1
}

fn archive(who: u64, url: &str) -> u32 {
    assert_ok!(Endpoints::register(
        RuntimeOrigin::signed(who),
        Kind::Archive,
        url.as_bytes().to_vec(),
        b"us".to_vec()
    ));
    crate::NextId::<Test>::get() - 1
}

fn held(who: u64) -> u128 {
    <Balances as InspectHold<u64>>::balance_on_hold(&crate::HoldReason::Endpoint.into(), &who)
}

fn bal(who: u64) -> u128 {
    <Balances as Inspect<u64>>::balance(&who)
}

/// All three reporters report the same scores for yesterday.
fn report_all(day: u32, scores: &[(u32, u8)]) {
    for r in [R1, R2, R3] {
        assert_ok!(Endpoints::report(
            RuntimeOrigin::signed(r),
            day,
            scores.to_vec()
        ));
    }
}

fn open_cap() {
    assert_ok!(Endpoints::set_params(
        RuntimeOrigin::root(),
        Params {
            deposit: 100 * ICT,
            min_score: 90,
            operator_cap_ppm: 1_000_000
        }
    ));
}

#[test]
fn register_holds_the_deposit_and_checks_the_url() {
    new_test_ext().execute_with(|| {
        let id = rpc(ALICE, "wss://rpc.alice.example");
        assert_eq!(held(ALICE), 100 * ICT);
        let e = Entries::<Test>::get(id).unwrap();
        assert!(e.active);
        assert_eq!(e.kind, Kind::Rpc);
        // a URL can be registered once
        assert_noop!(
            Endpoints::register(
                RuntimeOrigin::signed(BOB),
                Kind::Rpc,
                b"wss://rpc.alice.example".to_vec(),
                vec![]
            ),
            Error::<Test>::UrlTaken
        );
        // RPC and archive need wss:// or https://; bootnodes a multiaddress
        assert_noop!(
            Endpoints::register(
                RuntimeOrigin::signed(BOB),
                Kind::Rpc,
                b"http://plain.example".to_vec(),
                vec![]
            ),
            Error::<Test>::BadUrl
        );
        assert_noop!(
            Endpoints::register(
                RuntimeOrigin::signed(BOB),
                Kind::Bootnode,
                b"wss://x.example".to_vec(),
                vec![]
            ),
            Error::<Test>::BadUrl
        );
        assert_ok!(Endpoints::register(
            RuntimeOrigin::signed(BOB),
            Kind::Bootnode,
            b"/dns/boot.bob.example/tcp/30333/p2p/12D3KooW".to_vec(),
            vec![]
        ));
        assert_noop!(
            Endpoints::register(
                RuntimeOrigin::signed(BOB),
                Kind::Rpc,
                b"wss://has space".to_vec(),
                vec![]
            ),
            Error::<Test>::BadUrl
        );
    });
}

#[test]
fn at_most_four_entries_per_owner() {
    new_test_ext().execute_with(|| {
        for i in 0..4 {
            rpc(ALICE, &format!("wss://rpc{i}.alice.example"));
        }
        assert_eq!(OwnerCount::<Test>::get(ALICE), 4);
        assert_noop!(
            Endpoints::register(
                RuntimeOrigin::signed(ALICE),
                Kind::Rpc,
                b"wss://rpc4.alice.example".to_vec(),
                vec![]
            ),
            Error::<Test>::TooManyForOwner
        );
    });
}

#[test]
fn owner_updates_and_deregisters_and_gets_the_deposit_back() {
    new_test_ext().execute_with(|| {
        let id = rpc(ALICE, "wss://old.alice.example");
        assert_noop!(
            Endpoints::update(
                RuntimeOrigin::signed(BOB),
                id,
                b"wss://new.alice.example".to_vec(),
                vec![]
            ),
            Error::<Test>::NotOwner
        );
        assert_ok!(Endpoints::update(
            RuntimeOrigin::signed(ALICE),
            id,
            b"wss://new.alice.example".to_vec(),
            b"no".to_vec()
        ));
        assert_eq!(
            Entries::<Test>::get(id).unwrap().url.to_vec(),
            b"wss://new.alice.example".to_vec()
        );
        // the old URL is free again
        rpc(BOB, "wss://old.alice.example");
        assert_noop!(
            Endpoints::deregister(RuntimeOrigin::signed(BOB), id),
            Error::<Test>::NotOwner
        );
        assert_ok!(Endpoints::deregister(RuntimeOrigin::signed(ALICE), id));
        assert_eq!(held(ALICE), 0);
        assert_eq!(bal(ALICE), 1_000 * ICT);
        assert!(Entries::<Test>::get(id).is_none());
        assert_eq!(OwnerCount::<Test>::get(ALICE), 0);
    });
}

#[test]
fn only_reporters_report_once_for_yesterday() {
    new_test_ext().execute_with(|| {
        let id = rpc(ALICE, "wss://rpc.alice.example");
        run_to(DAY + 10); // day 1
        assert_noop!(
            Endpoints::report(RuntimeOrigin::signed(ALICE), 0, vec![(id, 100)]),
            Error::<Test>::NotReporter
        );
        assert_noop!(
            Endpoints::report(RuntimeOrigin::signed(R1), 1, vec![(id, 100)]),
            Error::<Test>::WrongDay
        );
        assert_noop!(
            Endpoints::report(RuntimeOrigin::signed(R1), 0, vec![(id, 101)]),
            Error::<Test>::BadScore
        );
        assert_noop!(
            Endpoints::report(RuntimeOrigin::signed(R1), 0, vec![(id, 90), (id, 90)]),
            Error::<Test>::BadScore
        );
        assert_noop!(
            Endpoints::report(RuntimeOrigin::signed(R1), 0, vec![(77, 90)]),
            Error::<Test>::UnknownEndpoint
        );
        assert_ok!(Endpoints::report(
            RuntimeOrigin::signed(R1),
            0,
            vec![(id, 100)]
        ));
        assert_noop!(
            Endpoints::report(RuntimeOrigin::signed(R1), 0, vec![(id, 100)]),
            Error::<Test>::AlreadyReported
        );
        assert_eq!(Tallies::<Test>::get(0, id).stake, 100);
    });
}

#[test]
fn a_reporter_key_reports_with_its_principals_stake_once_a_day() {
    use crate::{PrincipalOf, ReporterKey};
    use polkadot_sdk::frame_support::dispatch::Pays;
    const KEY: u64 = 21;
    const KEY2: u64 = 22;
    new_test_ext().execute_with(|| {
        let id = rpc(ALICE, "wss://rpc.alice.example");
        // R1 (a stash with stake) names KEY; KEY has no stake of its own.
        assert_ok!(Endpoints::set_reporter(RuntimeOrigin::signed(R1), Some(KEY)));
        assert_eq!(PrincipalOf::<Test>::get(KEY), Some(R1));
        // KEY cannot serve R2 too, cannot name a key itself, and R1 cannot be its own key.
        assert_noop!(
            Endpoints::set_reporter(RuntimeOrigin::signed(R2), Some(KEY)),
            Error::<Test>::ReporterInUse
        );
        assert_noop!(
            Endpoints::set_reporter(RuntimeOrigin::signed(KEY), Some(KEY2)),
            Error::<Test>::ReporterInUse
        );
        assert_noop!(
            Endpoints::set_reporter(RuntimeOrigin::signed(R2), Some(R1)),
            Error::<Test>::ReporterInUse
        );
        run_to(DAY + 10);
        let post = Endpoints::report(RuntimeOrigin::signed(KEY), 0, vec![(id, 100)]).unwrap();
        assert_eq!(post.pays_fee, Pays::No);
        assert_eq!(Tallies::<Test>::get(0, id).stake, 100);
        // The principal already reported through its key.
        assert_noop!(
            Endpoints::report(RuntimeOrigin::signed(R1), 0, vec![(id, 100)]),
            Error::<Test>::AlreadyReported
        );
        // A new key replaces the old one, which no longer reports for R1.
        assert_ok!(Endpoints::set_reporter(RuntimeOrigin::signed(R1), Some(KEY2)));
        assert_eq!(PrincipalOf::<Test>::get(KEY), None);
        assert_eq!(ReporterKey::<Test>::get(R1), Some(KEY2));
        run_to(2 * DAY + 10);
        assert_noop!(
            Endpoints::report(RuntimeOrigin::signed(KEY), 1, vec![(id, 100)]),
            Error::<Test>::NotReporter
        );
        assert_ok!(Endpoints::report(RuntimeOrigin::signed(KEY2), 1, vec![(id, 90)]));
        assert_ok!(Endpoints::set_reporter(RuntimeOrigin::signed(R1), None));
        assert_eq!(PrincipalOf::<Test>::get(KEY2), None);
    });
}

#[test]
fn a_day_pays_by_weight_and_score_and_archive_counts_double() {
    new_test_ext().execute_with(|| {
        open_cap();
        let a = rpc(ALICE, "wss://rpc.alice.example");
        let b = archive(BOB, "wss://archive.bob.example");
        fund_pot(300 * ICT);
        run_to(DAY + 10);
        report_all(0, &[(a, 100), (b, 100)]);
        let (a0, b0, f0) = (bal(ALICE), bal(BOB), bal(FUND));
        run_to(2 * DAY); // settles day 0
        assert_eq!(LastSettled::<Test>::get(), Some(0));
        assert_eq!(bal(ALICE) - a0, 100 * ICT);
        assert_eq!(bal(BOB) - b0, 200 * ICT);
        assert_eq!(bal(FUND), f0); // nothing left over
        assert_eq!(bal(POT), 0);
        assert_eq!(Entries::<Test>::get(a).unwrap().last_score, 100);
        // a day is settled once
        run_to(2 * DAY + 1);
        assert_eq!(LastSettled::<Test>::get(), Some(0));
    });
}

#[test]
fn the_operator_cap_sends_the_rest_to_the_fund() {
    new_test_ext().execute_with(|| {
        // default cap: 10 % of the day's pot per owner
        let a = rpc(ALICE, "wss://rpc.alice.example");
        let a2 = rpc(ALICE, "wss://rpc2.alice.example");
        fund_pot(1_000 * ICT);
        run_to(DAY + 10);
        report_all(0, &[(a, 100), (a2, 100)]);
        let (a0, f0) = (bal(ALICE), bal(FUND));
        run_to(2 * DAY);
        assert_eq!(bal(ALICE) - a0, 100 * ICT); // two entries, one cap
        assert_eq!(bal(FUND) - f0, 900 * ICT);
    });
}

#[test]
fn a_minority_of_reporters_cannot_move_a_score() {
    new_test_ext().execute_with(|| {
        open_cap();
        let a = rpc(ALICE, "wss://rpc.alice.example");
        let b = rpc(BOB, "wss://rpc.bob.example");
        fund_pot(200 * ICT);
        run_to(DAY + 10);
        // R3 lies: says Alice is down and Bob is perfect, against the other two
        assert_ok!(Endpoints::report(
            RuntimeOrigin::signed(R1),
            0,
            vec![(a, 100), (b, 40)]
        ));
        assert_ok!(Endpoints::report(
            RuntimeOrigin::signed(R2),
            0,
            vec![(a, 98), (b, 45)]
        ));
        assert_ok!(Endpoints::report(
            RuntimeOrigin::signed(R3),
            0,
            vec![(a, 0), (b, 100)]
        ));
        let (a0, b0) = (bal(ALICE), bal(BOB));
        run_to(2 * DAY);
        assert_eq!(Entries::<Test>::get(a).unwrap().last_score, 98);
        assert_eq!(Entries::<Test>::get(b).unwrap().last_score, 45);
        assert_eq!(bal(ALICE) - a0, 200 * ICT);
        assert_eq!(bal(BOB), b0);
    });
}

#[test]
fn without_a_stake_majority_nothing_is_paid() {
    new_test_ext().execute_with(|| {
        open_cap();
        let a = rpc(ALICE, "wss://rpc.alice.example");
        fund_pot(50 * ICT);
        run_to(DAY + 10);
        assert_ok!(Endpoints::report(
            RuntimeOrigin::signed(R1),
            0,
            vec![(a, 100)]
        )); // 1/3 of the stake
        let (a0, f0) = (bal(ALICE), bal(FUND));
        run_to(2 * DAY);
        assert_eq!(bal(ALICE), a0);
        assert_eq!(bal(FUND) - f0, 50 * ICT);
    });
}

#[test]
fn below_the_minimum_score_gets_nothing() {
    new_test_ext().execute_with(|| {
        open_cap();
        let a = rpc(ALICE, "wss://rpc.alice.example");
        let b = rpc(BOB, "wss://rpc.bob.example");
        fund_pot(100 * ICT);
        run_to(DAY + 10);
        report_all(0, &[(a, 89), (b, 90)]);
        let (a0, b0) = (bal(ALICE), bal(BOB));
        run_to(2 * DAY);
        assert_eq!(bal(ALICE), a0);
        assert_eq!(bal(BOB) - b0, 100 * ICT);
    });
}

#[test]
fn a_week_below_half_marks_inactive_and_one_good_day_restores() {
    new_test_ext().execute_with(|| {
        let a = rpc(ALICE, "wss://rpc.alice.example");
        for d in 0..7u32 {
            run_to((d as u64 + 1) * DAY + 10);
            report_all(d, &[(a, 20)]);
            run_to((d as u64 + 2) * DAY);
            assert_eq!(Entries::<Test>::get(a).unwrap().active, d < 6, "day {d}");
        }
        run_to(8 * DAY + 10);
        report_all(7, &[(a, 95)]);
        run_to(9 * DAY);
        let e = Entries::<Test>::get(a).unwrap();
        assert!(e.active);
        assert_eq!(e.failing_days, 0);
    });
}

#[test]
fn params_only_by_the_params_origin_and_within_bounds() {
    new_test_ext().execute_with(|| {
        let p = Params {
            deposit: 50 * ICT,
            min_score: 95,
            operator_cap_ppm: 200_000,
        };
        assert_noop!(
            Endpoints::set_params(RuntimeOrigin::signed(ALICE), p),
            DispatchError::BadOrigin
        );
        assert_noop!(
            Endpoints::set_params(
                RuntimeOrigin::root(),
                Params {
                    min_score: 101,
                    ..p
                }
            ),
            Error::<Test>::BadParams
        );
        assert_noop!(
            Endpoints::set_params(
                RuntimeOrigin::root(),
                Params {
                    operator_cap_ppm: 0,
                    ..p
                }
            ),
            Error::<Test>::BadParams
        );
        assert_ok!(Endpoints::set_params(RuntimeOrigin::root(), p));
        assert_eq!(CurrentParams::<Test>::get(), p);
        // the new deposit applies to new entries; existing entries keep theirs
        rpc(ALICE, "wss://rpc.alice.example");
        assert_eq!(held(ALICE), 50 * ICT);
    });
}

#[test]
fn an_empty_day_sends_the_whole_pot_to_the_fund() {
    new_test_ext().execute_with(|| {
        fund_pot(70 * ICT);
        let f0 = bal(FUND);
        run_to(2 * DAY);
        assert_eq!(bal(FUND) - f0, 70 * ICT);
        assert_eq!(bal(POT), 0);
    });
}
