use crate::{mock::*, Error, Revealed};
use polkadot_sdk::frame_support::{assert_noop, assert_ok};

const SALT: [u8; 32] = [7u8; 32];

fn setup() {
    // validator 1, miners 2 and 3 in SN0
    for a in 1..=3u64 {
        assert_ok!(Registry::register_neuron(RuntimeOrigin::signed(a), 0, a));
    }
}

#[test]
fn commit_then_reveal_next_epoch() {
    new_test_ext().execute_with(|| {
        setup();
        let (uids, w) = (vec![1u16, 2], vec![3u16, 1]);
        let h = Weights::commit_hash(0, &uids, &w, &SALT);
        assert_ok!(Weights::commit_weights(RuntimeOrigin::signed(1), 0, h));
        // too early
        assert_noop!(
            Weights::reveal_weights(RuntimeOrigin::signed(1), 0, uids.clone(), w.clone(), SALT),
            Error::<Test>::WrongRevealEpoch
        );
        System::set_block_number(10); // epoch 1
        assert_ok!(Weights::reveal_weights(
            RuntimeOrigin::signed(1),
            0,
            uids,
            w,
            SALT
        ));
        assert_eq!(
            Revealed::<Test>::get((0, 0), 1).unwrap().into_inner(),
            vec![(1, 3), (2, 1)]
        );
        assert_eq!(Weights::revealed(0, 0).len(), 1);
    });
}

#[test]
fn wrong_salt_or_weights_rejected() {
    new_test_ext().execute_with(|| {
        setup();
        let h = Weights::commit_hash(0, &[1], &[5], &SALT);
        assert_ok!(Weights::commit_weights(RuntimeOrigin::signed(1), 0, h));
        System::set_block_number(10);
        assert_noop!(
            Weights::reveal_weights(RuntimeOrigin::signed(1), 0, vec![1], vec![6], SALT),
            Error::<Test>::HashMismatch
        );
        assert_noop!(
            Weights::reveal_weights(RuntimeOrigin::signed(1), 0, vec![1], vec![5], [0u8; 32]),
            Error::<Test>::HashMismatch
        );
    });
}

#[test]
fn only_registered_neurons_commit_and_uids_in_range() {
    new_test_ext().execute_with(|| {
        assert_noop!(
            Weights::commit_weights(RuntimeOrigin::signed(4), 0, Default::default()),
            Error::<Test>::NotRegistered
        );
        setup();
        let h = Weights::commit_hash(0, &[9], &[1], &SALT);
        assert_ok!(Weights::commit_weights(RuntimeOrigin::signed(1), 0, h));
        System::set_block_number(10);
        assert_noop!(
            Weights::reveal_weights(RuntimeOrigin::signed(1), 0, vec![9], vec![1], SALT),
            Error::<Test>::UidOutOfRange
        );
    });
}

/// Shared test vector with the Python reference validator (ipm-neuron/tests).
#[test]
fn commit_hash_test_vector() {
    let h = Weights::commit_hash(0, &[1, 2], &[3, 1], &[7u8; 32]);
    assert_eq!(
        format!(
            "{}",
            polkadot_sdk::sp_core::hexdisplay::HexDisplay::from(&h.0)
        ),
        "cd1d13b27f1aa27e8b3635619fdd1181413a77f391b43b65abacd699d1296b92"
    );
}
