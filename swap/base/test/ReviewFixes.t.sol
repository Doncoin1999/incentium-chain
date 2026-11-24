// SPDX-License-Identifier: Apache-2.0
pragma solidity 0.8.28;

import {Test} from "forge-std/Test.sol";
import {IncentiumSwap} from "../src/IncentiumSwap.sol";
import {MockUSDC} from "./IncentiumSwap.t.sol";

/// USDC-like token with a blacklist, as Circle's USDC has.
contract BlacklistUSDC is MockUSDC {
    mapping(address => bool) public blocked;

    function blockAddress(address a) external {
        blocked[a] = true;
    }

    function transfer(address to, uint256 a) public override returns (bool) {
        require(!blocked[to], "blacklisted");
        return super.transfer(to, a);
    }
}

/// Regression tests for the AI-assisted security review of 24 Nov 2026 (findings H2 and M8).
contract ReviewFixes is Test {
    IncentiumSwap swap;
    BlacklistUSDC usdc;
    address buyer = address(0xB0B);
    address attacker = address(0xBAD);
    address seller = address(0x5E11);
    address fees = address(0xFEE);
    address arbiter = address(0xA2B1);
    uint256 constant GENESIS = 1_795_000_000;
    uint128 constant ICT = 1e9;
    uint128 constant PRICE = 250_000; // 0.25 USDC per ICT

    function setUp() public {
        vm.warp(GENESIS + 2 days);
        usdc = new BlacklistUSDC();
        swap = new IncentiumSwap(usdc, GENESIS, fees, address(0x6A2D), arbiter);
        for (uint256 i = 0; i < 3; i++) {
            address a = [buyer, attacker, seller][i];
            usdc.mint(a, 1_000e6);
            vm.prank(a);
            usdc.approve(address(swap), type(uint256).max);
        }
    }

    function secret(uint256 i) internal pure returns (bytes32) {
        return keccak256(abi.encode("s", i));
    }

    function place() internal returns (bytes32 id) {
        bytes32[16] memory h;
        for (uint256 i = 0; i < 16; i++) {
            h[i] = sha256(abi.encodePacked(secret(i)));
        }
        vm.prank(buyer);
        id = swap.placeBuy(1, 100 * ICT, PRICE, bytes32(uint256(1)), uint64(block.timestamp + 24 hours), h);
    }

    /// H2: blocking an order now costs a deposit, which goes to the maker after the window.
    function test_h2_blocking_costs_the_attacker_and_pays_the_maker() public {
        bytes32 id = place(); // 25 USDC
        vm.prank(attacker);
        swap.reserve(id, 0, 100 * ICT, attacker); // whole order, never locks ICT
        assertEq(usdc.balanceOf(attacker), 1_000e6 - 5e5); // 2 % of 25 USDC < 0.5 USDC minimum
        vm.warp(block.timestamp + 19 hours);
        swap.expireReservation(id, 0);
        vm.expectRevert(IncentiumSwap.TooEarly.selector);
        swap.payDepositToMaker(id, 0);
        vm.warp(block.timestamp + 24 hours);
        uint256 before = usdc.balanceOf(buyer);
        swap.payDepositToMaker(id, 0);
        assertEq(usdc.balanceOf(buyer) - before, 5e5);
    }

    /// H2: one open reservation per payee per order.
    function test_h2_one_open_reservation_per_payee() public {
        bytes32 id = place();
        vm.startPrank(attacker);
        swap.reserve(id, 0, 20 * ICT, attacker);
        vm.expectRevert(IncentiumSwap.PayeeHasOpenReservation.selector);
        swap.reserve(id, 1, 20 * ICT, attacker);
        vm.stopPrank();
    }

    /// H2: if the seller locked ICT and only the buy maker failed to confirm, the arbiter returns
    /// the deposit; nobody else can, and not after the window.
    function test_h2_arbiter_returns_deposit_when_the_maker_did_not_confirm() public {
        bytes32 id = place();
        vm.prank(seller);
        swap.reserve(id, 0, 40 * ICT, seller);
        vm.warp(block.timestamp + 19 hours);
        swap.expireReservation(id, 0);
        vm.prank(attacker);
        vm.expectRevert(IncentiumSwap.NotArbiter.selector);
        swap.returnDeposit(id, 0);
        vm.prank(arbiter);
        swap.returnDeposit(id, 0);
        assertEq(usdc.balanceOf(seller), 1_000e6);
        vm.expectRevert(IncentiumSwap.BadDepositState.selector);
        swap.payDepositToMaker(id, 0);
    }

    function test_h2_arbiter_cannot_return_after_the_window() public {
        bytes32 id = place();
        vm.prank(seller);
        swap.reserve(id, 0, 40 * ICT, seller);
        vm.warp(block.timestamp + 19 hours);
        swap.expireReservation(id, 0);
        vm.warp(block.timestamp + 24 hours);
        vm.prank(arbiter);
        vm.expectRevert(IncentiumSwap.BadDepositState.selector);
        swap.returnDeposit(id, 0);
    }

    /// M8: a blacklisted fee recipient no longer blocks claims; only the fee withdrawal waits.
    function test_m8_blacklisted_fee_recipient_does_not_block_claims() public {
        bytes32 id = place();
        vm.prank(seller);
        swap.reserve(id, 0, 100 * ICT, seller);
        usdc.blockAddress(fees);
        swap.claimReservation(id, 0, secret(0));
        assertEq(usdc.balanceOf(seller), 1_000e6 + 25e6 - 25_000); // paid, deposit back
        assertEq(swap.accruedFees(), 25_000);
        vm.expectRevert(bytes("blacklisted"));
        swap.withdrawFees();
    }
}
