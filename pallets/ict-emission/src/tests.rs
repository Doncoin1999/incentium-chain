use crate::{mock::*, EndpointShare, Error, FundShare, Halvings, TotalIssued, ValidatorShare};
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
        // 10 % outside the subnets: 8 % to the fund, 2 % to the block producers
        assert_eq!(Balances::free_balance(FUND), 8 * UNIT / 10);
        assert_eq!(Balances::free_balance(VPOT), 2 * UNIT / 10);
        assert_eq!(Balances::free_balance(POT), 9 * UNIT);
    });
}

#[test]
fn validator_share_only_by_root_and_never_above_the_fund_share() {
    new_test_ext().execute_with(|| {
        assert_noop!(
            Emission::set_validator_share(RuntimeOrigin::signed(1), 10_000),
            DispatchError::BadOrigin
        );
        assert_noop!(
            Emission::set_validator_share(RuntimeOrigin::root(), 100_001),
            Error::<Test>::ValidatorShareTooHigh
        );
        assert_ok!(Emission::set_validator_share(RuntimeOrigin::root(), 0));
        assert_eq!(ValidatorShare::<Test>::get(), 0);
        run_blocks(10);
        assert_eq!(Balances::free_balance(FUND), UNIT);
        assert_eq!(Balances::free_balance(VPOT), 0);
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
        // 5 % of 20 ICT outside the subnets; the producers' 2 % comes out of it
        assert_eq!(Balances::free_balance(FUND), 6 * UNIT / 10);
        assert_eq!(Balances::free_balance(VPOT), 4 * UNIT / 10);
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

#[test]
fn endpoint_share_starts_at_zero_and_is_carved_out_of_the_fund() {
    new_test_ext().execute_with(|| {
        assert_eq!(EndpointShare::<Test>::get(), 0);
        run_blocks(10);
        assert_eq!(Balances::free_balance(EPOT), 0);
        assert_noop!(
            Emission::set_endpoint_share(RuntimeOrigin::signed(1), 50_000),
            DispatchError::BadOrigin
        );
        // 2 % to producers + 9 % to endpoints would exceed the fund's 10 %
        assert_noop!(
            Emission::set_endpoint_share(RuntimeOrigin::root(), 90_000),
            Error::<Test>::EndpointShareTooHigh
        );
        assert_ok!(Emission::set_endpoint_share(RuntimeOrigin::root(), 50_000));
        let (f0, v0, p0) = (
            Balances::free_balance(FUND),
            Balances::free_balance(VPOT),
            Balances::free_balance(POT),
        );
        run_blocks(10);
        // of 10 ICT: 0.3 to the fund, 0.2 to producers, 0.5 to endpoints, 9 to subnets
        assert_eq!(Balances::free_balance(FUND) - f0, 3 * UNIT / 10);
        assert_eq!(Balances::free_balance(VPOT) - v0, 2 * UNIT / 10);
        assert_eq!(Balances::free_balance(EPOT), 5 * UNIT / 10);
        assert_eq!(Balances::free_balance(POT) - p0, 9 * UNIT);
        // and the producers' share can no longer be raised past what is left
        assert_noop!(
            Emission::set_validator_share(RuntimeOrigin::root(), 60_000),
            Error::<Test>::ValidatorShareTooHigh
        );
    });
}
