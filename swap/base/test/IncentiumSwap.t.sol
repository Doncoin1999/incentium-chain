// SPDX-License-Identifier: Apache-2.0
pragma solidity 0.8.28;

import {Test} from "forge-std/Test.sol";
import {IncentiumSwap, IERC20} from "../src/IncentiumSwap.sol";

contract MockUSDC is IERC20 {
    mapping(address => uint256) public balanceOf;
    mapping(address => mapping(address => uint256)) public allowance;

    function mint(address to, uint256 a) external {
        balanceOf[to] += a;
    }

    function approve(address s, uint256 a) external returns (bool) {
        allowance[msg.sender][s] = a;
        return true;
    }

    function transfer(address to, uint256 a) public virtual returns (bool) {
        require(balanceOf[msg.sender] >= a, "balance");
        balanceOf[msg.sender] -= a;
        balanceOf[to] += a;
        return true;
    }

    function transferFrom(address f, address to, uint256 a) external returns (bool) {
        require(allowance[f][msg.sender] >= a, "allowance");
        require(balanceOf[f] >= a, "balance");
        allowance[f][msg.sender] -= a;
        balanceOf[f] -= a;
        balanceOf[to] += a;
        return true;
    }
}

contract IncentiumSwapTest is Test {
    MockUSDC usdc;
    IncentiumSwap swap;
    address buyer = address(0xB0B);
    address seller = address(0x5E11);
    address keeper = address(0x4EE9);
    bytes32 constant ICT_RECIPIENT = bytes32(uint256(0x1234));
    uint256 constant GENESIS = 1_795_000_000;
    uint128 constant ICT = 1e9;
    uint128 constant PRICE = 250_000; // 0.25 USDC per ICT

    function setUp() public {
        vm.warp(GENESIS + 2 days);
        usdc = new MockUSDC();
        swap = new IncentiumSwap(usdc, GENESIS);
        usdc.mint(buyer, 1_000_000e6);
        vm.prank(buyer);
        usdc.approve(address(swap), type(uint256).max);
        // whoever reserves pays the deposit (review fix H2): the keeper and this test contract
        usdc.mint(keeper, 1_000e6);
        vm.prank(keeper);
        usdc.approve(address(swap), type(uint256).max);
        usdc.mint(address(this), 1_000e6);
        usdc.approve(address(swap), type(uint256).max);
    }

    function secret(uint256 i) internal pure returns (bytes32) {
        return keccak256(abi.encode("test secret", i));
    }

    function hashesFor() internal pure returns (bytes32[16] memory h) {
        for (uint256 i = 0; i < 16; i++) {
            h[i] = sha256(abi.encodePacked(secret(i)));
        }
    }

    function place(uint128 qty) internal returns (bytes32 id) {
        bytes32[16] memory h = hashesFor();
        vm.prank(buyer);
        id = swap.placeBuy(1, qty, PRICE, ICT_RECIPIENT, uint64(block.timestamp + 24 hours), h);
    }

    // ---------------------------------------------------------------- hashing

    function test_sha256_vector_shared_with_pallet() public view {
        // Same vector is asserted in pallets/ict-swap tests: sha256 of 32 bytes 0x01..0x20.
        bytes32 s = 0x0102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f20;
        assertEq(swap.hashOf(s), 0xae216c2ef5247a3782c135efa279a3e4cdc61094270f5d2be58c6204b7a612c9);
    }

    // ---------------------------------------------------------------- buy orders

    function test_place_locks_usdc() public {
        bytes32 id = place(100 * ICT);
        assertEq(usdc.balanceOf(address(swap)), 25e6);
        (address maker,, bool open,,,,,, uint128 remaining, uint128 free) = swap.buyOrders(id);
        assertEq(maker, buyer);
        assertTrue(open);
        assertEq(remaining, 100 * ICT);
        assertEq(free, 25e6);
    }

    function test_caps_by_day() public {
        assertEq(swap.maxOrderQty(), 100 * ICT);
        bytes32[16] memory h = hashesFor();
        vm.prank(buyer);
        vm.expectRevert(IncentiumSwap.OverCap.selector);
        swap.placeBuy(9, 101 * ICT, PRICE, ICT_RECIPIENT, uint64(block.timestamp + 1 hours), h);
        vm.warp(GENESIS + 90 days);
        assertEq(swap.maxOrderQty(), 1_000 * ICT);
        vm.warp(GENESIS + 119 days + 23 hours);
        assertEq(swap.maxOrderQty(), 1_000 * ICT);
        vm.warp(GENESIS + 120 days);
        assertEq(swap.maxOrderQty(), 5_000 * ICT);
        place(5_000 * ICT);
    }

    function test_rejects_bad_orders() public {
        bytes32[16] memory h = hashesFor();
        vm.startPrank(buyer);
        vm.expectRevert(IncentiumSwap.BadQuantity.selector);
        swap.placeBuy(1, 1, PRICE, ICT_RECIPIENT, uint64(block.timestamp + 1 hours), h);
        vm.expectRevert(IncentiumSwap.BadExpiry.selector);
        swap.placeBuy(1, 50 * ICT, PRICE, ICT_RECIPIENT, uint64(block.timestamp + 25 hours), h);
        vm.expectRevert(IncentiumSwap.TooSmall.selector);
        swap.placeBuy(1, 10 * ICT, PRICE, ICT_RECIPIENT, uint64(block.timestamp + 1 hours), h); // 2.5 USDC
        vm.expectRevert(IncentiumSwap.BadRecipient.selector);
        swap.placeBuy(1, 50 * ICT, PRICE, bytes32(0), uint64(block.timestamp + 1 hours), h);
        swap.placeBuy(1, 50 * ICT, PRICE, ICT_RECIPIENT, uint64(block.timestamp + 1 hours), h);
        vm.expectRevert(IncentiumSwap.OrderExists.selector);
        swap.placeBuy(1, 50 * ICT, PRICE, ICT_RECIPIENT, uint64(block.timestamp + 1 hours), h);
        vm.stopPrank();
    }

    function test_cancel_returns_unreserved() public {
        bytes32 id = place(100 * ICT);
        vm.prank(keeper);
        swap.reserve(id, 0, 40 * ICT, seller); // 10 USDC reserved
        vm.prank(seller);
        vm.expectRevert(IncentiumSwap.NotMaker.selector);
        swap.cancelBuy(id);
        uint256 before = usdc.balanceOf(buyer);
        vm.prank(buyer);
        swap.cancelBuy(id);
        assertEq(usdc.balanceOf(buyer) - before, 15e6);
        // the reservation still completes after the cancel
        vm.prank(keeper);
        swap.claimReservation(id, 0, secret(0));
        assertEq(usdc.balanceOf(seller), 10e6); // no fee
        assertEq(usdc.balanceOf(keeper), 1_000e6); // deposit returned
        assertEq(usdc.balanceOf(address(swap)), 0);
    }

    // ---------------------------------------------------------------- reservations

    function test_reserve_and_claim_pays_seller_in_full() public {
        bytes32 id = place(100 * ICT);
        vm.prank(keeper);
        swap.reserve(id, 3, 60 * ICT, seller); // 15 USDC
        vm.prank(keeper);
        vm.expectRevert(IncentiumSwap.WrongSecret.selector);
        swap.claimReservation(id, 3, secret(4));
        vm.prank(keeper);
        swap.claimReservation(id, 3, secret(3));
        assertEq(usdc.balanceOf(seller), 15e6);
        vm.expectRevert(IncentiumSwap.NotActive.selector);
        swap.claimReservation(id, 3, secret(3));
    }

    function test_fill_slot_used_once() public {
        bytes32 id = place(100 * ICT);
        swap.reserve(id, 0, 20 * ICT, seller);
        vm.expectRevert(IncentiumSwap.FillTaken.selector);
        swap.reserve(id, 0, 20 * ICT, seller);
        vm.warp(block.timestamp + 19 hours);
        swap.expireReservation(id, 0);
        // an expired slot stays used: its secret might have been revealed on the other chain
        vm.warp(block.timestamp - 19 hours + 1 hours);
        vm.expectRevert(IncentiumSwap.FillTaken.selector);
        swap.reserve(id, 0, 20 * ICT, seller);
    }

    function test_max_open_reservations() public {
        bytes32 id = place(100 * ICT);
        // one open reservation per payee (review fix H2), so three sellers
        swap.reserve(id, 0, 20 * ICT, seller);
        swap.reserve(id, 1, 20 * ICT, address(0x5E12));
        swap.reserve(id, 2, 20 * ICT, address(0x5E13));
        vm.expectRevert(IncentiumSwap.TooManyOpenReservations.selector);
        swap.reserve(id, 3, 20 * ICT, address(0x5E14));
        swap.claimReservation(id, 1, secret(1));
        swap.reserve(id, 3, 20 * ICT, address(0x5E12));
    }

    function test_reservation_deadlines() public {
        bytes32 id = place(100 * ICT);
        swap.reserve(id, 0, 40 * ICT, seller);
        vm.expectRevert(IncentiumSwap.TooEarly.selector);
        swap.expireReservation(id, 0);
        vm.warp(block.timestamp + 19 hours);
        vm.expectRevert(IncentiumSwap.TooLate.selector);
        swap.claimReservation(id, 0, secret(0));
        swap.expireReservation(id, 0); // order still open (24 h): quantity goes back
        (,,,,,,,, uint128 remaining, uint128 free) = swap.buyOrders(id);
        assertEq(remaining, 100 * ICT);
        assertEq(free, 25e6);
    }

    function test_expired_reservation_after_order_end_refunds_maker() public {
        bytes32 id = place(100 * ICT);
        vm.warp(block.timestamp + 20 hours);
        swap.reserve(id, 0, 40 * ICT, seller); // reservation outlives the order
        vm.warp(block.timestamp + 5 hours);
        swap.refundExpired(id); // 15 USDC back
        vm.warp(block.timestamp + 15 hours);
        uint256 before = usdc.balanceOf(buyer);
        swap.expireReservation(id, 0); // 10 USDC back to the maker
        assertEq(usdc.balanceOf(buyer) - before, 10e6);
        assertEq(usdc.balanceOf(address(swap)), 25e4); // only the burned half of the 0.5 USDC deposit is left
        assertEq(swap.burnedDeposits(), 25e4);
    }

    function test_no_reserve_after_expiry_or_over_remaining() public {
        bytes32 id = place(100 * ICT);
        vm.expectRevert(IncentiumSwap.BadQuantity.selector);
        swap.reserve(id, 0, 101 * ICT, seller);
        vm.expectRevert(IncentiumSwap.TooSmall.selector);
        swap.reserve(id, 0, 10 * ICT, seller);
        vm.warp(block.timestamp + 24 hours);
        vm.expectRevert(IncentiumSwap.OrderExpired.selector);
        swap.reserve(id, 0, 40 * ICT, seller);
    }

    // ---------------------------------------------------------------- locks

    function lockFor(uint64 ttl) internal returns (bytes32 id) {
        bytes32 h = sha256(abi.encodePacked(secret(5)));
        vm.prank(buyer);
        id = swap.lock(bytes32("inc-order"), 5, h, seller, 80 * ICT, PRICE, uint64(block.timestamp + ttl));
    }

    function test_lock_claim_pays_maker() public {
        bytes32 id = lockFor(12 hours);
        assertEq(usdc.balanceOf(address(swap)), 20e6);
        vm.prank(keeper);
        swap.claimLock(id, secret(5));
        assertEq(usdc.balanceOf(seller), 20e6);
        vm.expectRevert(IncentiumSwap.NotActive.selector);
        swap.refundLock(id);
    }

    function test_lock_refund_after_expiry() public {
        bytes32 id = lockFor(12 hours);
        vm.expectRevert(IncentiumSwap.TooEarly.selector);
        swap.refundLock(id);
        vm.warp(block.timestamp + 12 hours);
        vm.expectRevert(IncentiumSwap.TooLate.selector);
        swap.claimLock(id, secret(5));
        uint256 before = usdc.balanceOf(buyer);
        swap.refundLock(id);
        assertEq(usdc.balanceOf(buyer) - before, 20e6);
    }

    function test_lock_limits() public {
        bytes32 h = sha256(abi.encodePacked(secret(1)));
        vm.startPrank(buyer);
        vm.expectRevert(IncentiumSwap.BadExpiry.selector);
        swap.lock(bytes32("o"), 1, h, seller, 80 * ICT, PRICE, uint64(block.timestamp + 12 hours + 1));
        vm.expectRevert(IncentiumSwap.OverCap.selector);
        swap.lock(bytes32("o"), 1, h, seller, 200 * ICT, PRICE, uint64(block.timestamp + 1 hours));
        swap.lock(bytes32("o"), 1, h, seller, 80 * ICT, PRICE, uint64(block.timestamp + 1 hours));
        vm.expectRevert(IncentiumSwap.LockExists.selector);
        swap.lock(bytes32("o"), 1, h, seller, 80 * ICT, PRICE, uint64(block.timestamp + 1 hours));
        vm.stopPrank();
    }

    // ---------------------------------------------------------------- no privileged address

    function test_no_fee_no_pause_no_privileged_calls() public {
        // The v1 selectors for fees, pause and the deposit arbiter are gone.
        bytes4[6] memory gone = [
            bytes4(keccak256("withdrawFees()")),
            bytes4(keccak256("setPaused(bool)")),
            bytes4(keccak256("returnDeposit(bytes32,uint8)")),
            bytes4(keccak256("payDepositToMaker(bytes32,uint8)")),
            bytes4(keccak256("feeRecipient()")),
            bytes4(keccak256("pauseGuardian()"))
        ];
        for (uint256 i = 0; i < gone.length; i++) {
            (bool ok,) = address(swap).call(abi.encodeWithSelector(gone[i]));
            assertFalse(ok);
        }
    }

    // ---------------------------------------------------------------- fuzz: money is conserved

    function testFuzz_conservation(uint8 nFills, uint64 seed) public {
        nFills = uint8(bound(nFills, 1, 10));
        bytes32 id = place(100 * ICT);
        for (uint8 i = 0; i < nFills; i++) {
            (,,,,,,,, uint128 remaining,) = swap.buyOrders(id);
            uint128 q = uint128(20 * ICT + (uint256(keccak256(abi.encode(seed, i))) % (20 * ICT)) / 1e7 * 1e7);
            if (q > remaining || remaining < 20 * ICT) break;
            swap.reserve(id, i, q, seller);
            if (uint256(keccak256(abi.encode(seed, i, "claim"))) % 2 == 0) {
                swap.claimReservation(id, i, secret(i));
            } else {
                vm.warp(block.timestamp + 19 hours);
                swap.expireReservation(id, i);
                vm.warp(block.timestamp - 19 hours);
            }
        }
        vm.warp(block.timestamp + 48 hours);
        (,, bool open,,,,,,,) = swap.buyOrders(id);
        if (open) swap.refundExpired(id);
        // deposits of unclaimed reservations were burned at expiry
        assertEq(
            usdc.balanceOf(seller) + swap.burnedDeposits() + usdc.balanceOf(buyer) + usdc.balanceOf(address(this)),
            1_000_000e6 + 1_000e6
        );
        assertEq(usdc.balanceOf(address(swap)), swap.burnedDeposits());
    }
}
