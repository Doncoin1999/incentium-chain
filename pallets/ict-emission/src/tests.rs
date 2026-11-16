use crate::{mock::*, Error, FundShare, Halvings, TotalIssued};
use ict_economics::{emission::next_threshold, UNIT};
use polkadot_sdk::frame_support::{assert_noop, assert_ok, traits::Hooks};
use polkadot_sdk::sp_runtime::DispatchError;

fn run_blocks(n: u64) {
    for _ in 0..n {
        let b = System::block_number() + 1;
        System::set_block_number(b);
        Emission::on_initialize(b);
    }
}

#[test]
fn issues_one_ict_per_block_split_ten_percent() {
    new_test_ext().execute_with(|| {
        run_blocks(10);
        assert_eq!(TotalIssued::<Test>::get(), 10 * UNIT);
        assert_eq!(Balances::free_balance(FUND), UNIT);
        assert_eq!(Balances::free_balance(POT), 9 * UNIT);
    });
}

#[test]
fn halving_crossed_exactly_and_event_emitted() {
    new_test_ext().execute_with(|| {
        TotalIssued::<Test>::put(next_threshold(0) - UNIT / 4);
        run_blocks(1);
        assert_eq!(Halvings::<Test>::get(), 1);
        assert_eq!(TotalIssued::<Test>::get(), next_threshold(0) + 3 * UNIT / 8);
        System::assert_last_event(
            crate::Event::Halved {
                halvings: 1,
                total_issued: next_threshold(0) + 3 * UNIT / 8,
            }
            .into(),
        );
    });
}

#[test]
fn fund_share_only_by_root_and_bounded() {
    new_test_ext().execute_with(|| {
        assert_noop!(
            Emission::set_fund_share(RuntimeOrigin::signed(1), 50_000),
            DispatchError::BadOrigin
        );
        assert_noop!(
            Emission::set_fund_share(RuntimeOrigin::root(), 300_000),
            Error::<Test>::FundShareTooHigh
        );
        assert_ok!(Emission::set_fund_share(RuntimeOrigin::root(), 50_000));
        assert_eq!(FundShare::<Test>::get(), 50_000);
        run_blocks(20);
        assert_eq!(Balances::free_balance(FUND), UNIT); // 5 % of 20 ICT
    });
}

#[test]
fn stops_at_max_supply() {
    new_test_ext().execute_with(|| {
        let max = ict_economics::emission::MAX_SUPPLY;
        TotalIssued::<Test>::put(max - 1);
        Halvings::<Test>::put(40);
        run_blocks(5);
        assert!(TotalIssued::<Test>::get() <= max);
    });
}

#[test]
fn genesis_allocation_counts_against_supply() {
    use polkadot_sdk::sp_runtime::BuildStorage;
    let mut t = polkadot_sdk::frame_system::GenesisConfig::<Test>::default()
        .build_storage()
        .unwrap();
    crate::GenesisConfig::<Test> {
        pre_issued: 100 * UNIT,
        ..Default::default()
    }
    .assimilate_storage(&mut t)
    .unwrap();
    polkadot_sdk::sp_io::TestExternalities::new(t).execute_with(|| {
        System::set_block_number(1);
        run_blocks(1);
        assert_eq!(TotalIssued::<Test>::get(), 101 * UNIT);
    });
}
