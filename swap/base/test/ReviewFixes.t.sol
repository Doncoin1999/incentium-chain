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

/// Regression tests for the security review of 24 Nov 2026 (finding H2), as changed for version 2:
/// an unclaimed reservation's deposit is burned (kept in the contract for good), with no arbiter.
contract ReviewFixes is Test {
    IncentiumSwap swap;
    BlacklistUSDC usdc;
    address buyer = address(0xB0B);
    address attacker = address(0xBAD);
    address seller = address(0x5E11);
    uint256 constant GENESIS = 1_795_000_000;
    uint128 constant ICT = 1e9;
    uint128 constant PRICE = 250_000; // 0.25 USDC per ICT

    function setUp() public {
        vm.warp(GENESIS + 2 days);
        usdc = new BlacklistUSDC();
        swap = new IncentiumSwap(usdc, GENESIS);
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

    /// H2: blocking an order costs the attacker the deposit; the maker gets all its USDC back
    /// and gains nothing from the failed trade.
    function test_h2_blocking_costs_the_attacker_and_nobody_gains() public {
        bytes32 id = place(); // 25 USDC
        vm.prank(attacker);
        swap.reserve(id, 0, 100 * ICT, attacker); // whole order, never locks ICT
        assertEq(usdc.balanceOf(attacker), 1_000e6 - 5e5); // 2 % of 25 USDC < 0.5 USDC minimum
        vm.warp(block.timestamp + 19 hours);
        swap.expireReservation(id, 0);
        vm.warp(block.timestamp + 6 hours);
        swap.refundExpired(id);
        assertEq(usdc.balanceOf(buyer), 1_000e6); // all 25 USDC back, nothing more
        assertEq(usdc.balanceOf(attacker), 1_000e6 - 5e5); // deposit gone
        assertEq(swap.burnedDeposits(), 5e5);
        assertEq(usdc.balanceOf(address(swap)), 5e5);
        (,,,,,, IncentiumSwap.DepositState ds,) = swap.reservations(id, 0);
        assertEq(uint8(ds), uint8(IncentiumSwap.DepositState.Burned));
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

    /// The deposit comes back in full when the reservation is claimed; 2 % above the minimum.
    function test_h2_deposit_returned_on_claim() public {
        bytes32 id = place();
        vm.prank(seller);
        swap.reserve(id, 0, 100 * ICT, seller); // 25 USDC fill: 2 % = 0.5 USDC
        vm.prank(attacker); // anyone may relay the secret
        swap.claimReservation(id, 0, secret(0));
        assertEq(usdc.balanceOf(seller), 1_000e6 + 25e6); // paid in full, deposit back
        assertEq(swap.burnedDeposits(), 0);
        assertEq(swap.depositFor(100e6), 2e6);
    }

    /// Expiry needs no transfer of the deposit, so a blacklisted address cannot block it and the
    /// maker always gets its USDC back.
    function test_expiry_works_whoever_is_blacklisted() public {
        bytes32 id = place();
        vm.prank(seller);
        swap.reserve(id, 0, 40 * ICT, seller);
        usdc.blockAddress(seller);
        usdc.blockAddress(address(0xdEaD));
        vm.warp(block.timestamp + 25 hours);
        swap.refundExpired(id); // 15 USDC
        swap.expireReservation(id, 0); // 10 USDC
        assertEq(usdc.balanceOf(buyer), 1_000e6);
    }
}
