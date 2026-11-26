// SPDX-License-Identifier: Apache-2.0
pragma solidity 0.8.28;

/// @title IncentiumSwap
/// @notice USDC side of ICT <-> USDC atomic swaps between Base and the Incentium chain
///         (Incentium Swap Technical Specification v0.1, Section 6).
///
/// Buy orders live here with their USDC locked and 16 secret hashes committed. A seller fills
/// one by having fill `i` reserved for his Base address, then locking ICT on Incentium under
/// hash `i`. Buyers who fill sell orders that live on Incentium lock USDC here under the sell
/// order's hash. Every payout goes to an address fixed when the order, reservation or lock was
/// made, and needs either the secret (before the deadline) or the deadline to have passed.
///
/// No owner, no upgrade, no pause, no fee and no privileged address of any kind: after deployment
/// nobody has rights over this contract that any other address does not have.
///
/// Version 2 (26 Nov 2026), replacing the first mainnet deployment:
/// - No fee (was 0.1 % to a fixed address) and no pause guardian.
/// - Reservation deposit (review fix H2): whoever reserves a fill pays 2 % of it in USDC, at least
///   0.5 USDC. It is returned when the reservation is claimed. If the reservation expires unclaimed,
///   half goes back to whoever paid it and half is burned: the burned half stays in this contract
///   for good, counted in `burnedDeposits`, and no call can move it. Nobody gains from a trade that
///   did not complete, so no arbiter is needed; an honest taker whose maker never confirmed loses
///   1 %, and so does a taker who blocks an order. (Kept here rather than sent to a dead address,
///   so a USDC blacklist of that address cannot block expiry.) A deposit refund that USDC refuses,
///   e.g. to a blacklisted address, is recorded in `owed` instead of blocking the claim or expiry.
///   One open reservation per payee per order.
contract IncentiumSwap {
    // ------------------------------------------------------------------ constants

    uint256 public constant ICT_UNIT = 1e9; // ICT has 9 decimals
    uint256 public constant QTY_STEP = 1e7; // fills in steps of 0.01 ICT
    uint256 public constant MIN_FILL_USDC = 5e6; // 5 USDC (6 decimals)
    uint256 public constant MAX_FILLS = 16; // committed hashes per order
    uint256 public constant MAX_OPEN_RESERVATIONS = 3; // per order, reserved but not claimed or expired
    uint256 public constant ORDER_TTL_MAX = 24 hours;
    uint256 public constant RESERVATION_TTL = 19 hours; // 1 h to lock + 12 h to confirm + 6 h margin
    uint256 public constant LOCK_TTL_MAX = 12 hours;
    uint256 public constant DEPOSIT_BPS = 200; // reservation deposit: 2 % of the fill
    uint256 public constant MIN_DEPOSIT = 5e5; // at least 0.5 USDC

    // ------------------------------------------------------------------ types

    enum State {
        None,
        Active,
        Claimed,
        Expired
    }

    struct BuyOrder {
        address maker;
        uint64 expiry;
        bool open;
        uint8 openReservations;
        uint16 usedFills; // bit i set = hash i taken by a reservation
        bytes32 ictRecipient; // Incentium account (32-byte public key) that receives the ICT
        uint128 maxQty; // ICT base units
        uint128 price; // USDC base units per 1 ICT
        uint128 remainingQty; // not yet reserved or filled
        uint128 freeUsdc; // USDC held for the remaining quantity
    }

    enum DepositState {
        None,
        Held,
        Returned, // in full: the reservation was claimed
        HalfBurned // the reservation expired: half returned, half burned
    }

    struct Reservation {
        State state;
        uint64 expiry;
        address payee; // the seller's Base address
        uint128 qty;
        uint128 amount; // USDC
        address depositor; // paid the deposit (the seller or a keeper)
        DepositState depositState;
        uint128 deposit; // USDC
    }

    struct Lock {
        State state;
        uint64 expiry;
        address taker; // buyer, refunded after expiry
        address payee; // the sell maker's Base address
        uint128 qty;
        uint128 amount; // USDC
        bytes32 hash;
        bytes32 incOrderId; // sell order on Incentium
        uint8 fill;
    }

    // ------------------------------------------------------------------ storage

    IERC20 public immutable usdc;
    uint256 public immutable genesisTime; // Incentium mainnet genesis (block 1), for the size caps
    uint256 public burnedDeposits; // burned halves of unclaimed reservations' deposits, locked here for good
    mapping(address => uint256) public owed; // deposit refunds USDC refused to deliver; claimOwed retries

    mapping(bytes32 => BuyOrder) public buyOrders;
    mapping(bytes32 => bytes32[MAX_FILLS]) internal _hashes;
    mapping(bytes32 => mapping(uint8 => Reservation)) public reservations;
    mapping(bytes32 => Lock) public locks;
    mapping(bytes32 => mapping(address => bool)) public openByPayee; // one open reservation per payee per order

    uint256 private _entered = 1;

    // ------------------------------------------------------------------ events

    event BuyPlaced(
        bytes32 indexed orderId,
        address indexed maker,
        bytes32 ictRecipient,
        uint128 maxQty,
        uint128 price,
        uint64 expiry
    );
    event BuyClosed(bytes32 indexed orderId, uint128 refunded);
    event Reserved(
        bytes32 indexed orderId, uint8 indexed fill, address payee, uint128 qty, uint128 amount, uint64 expiry
    );
    event ReservationClaimed(bytes32 indexed orderId, uint8 indexed fill, bytes32 secret, uint128 paid);
    event ReservationExpired(bytes32 indexed orderId, uint8 indexed fill);
    event Locked(
        bytes32 indexed lockId,
        bytes32 indexed incOrderId,
        uint8 fill,
        address taker,
        address payee,
        bytes32 hash,
        uint128 qty,
        uint128 amount,
        uint64 expiry
    );
    event LockClaimed(bytes32 indexed lockId, bytes32 secret, uint128 paid);
    event LockRefunded(bytes32 indexed lockId);
    event DepositHeld(bytes32 indexed orderId, uint8 indexed fill, address depositor, uint128 deposit);
    event DepositReturned(bytes32 indexed orderId, uint8 indexed fill, uint128 amount);
    event DepositHalfBurned(bytes32 indexed orderId, uint8 indexed fill, uint128 returned, uint128 burned);
    event RefundOwed(address indexed to, uint256 amount);
    event OwedPaid(address indexed to, uint256 amount);

    // ------------------------------------------------------------------ errors

    error BadQuantity();
    error BadPrice();
    error BadExpiry();
    error BadRecipient();
    error OverCap();
    error TooSmall();
    error OrderExists();
    error OrderNotOpen();
    error OrderExpired();
    error NotMaker();
    error FillTaken();
    error TooManyOpenReservations();
    error NotActive();
    error WrongSecret();
    error TooLate();
    error TooEarly();
    error LockExists();
    error TransferFailed();
    error Reentrancy();
    error PayeeHasOpenReservation();

    constructor(IERC20 usdc_, uint256 genesisTime_) {
        if (address(usdc_) == address(0)) revert BadRecipient();
        usdc = usdc_;
        genesisTime = genesisTime_;
    }

    modifier nonReentrant() {
        if (_entered != 1) revert Reentrancy();
        _entered = 2;
        _;
        _entered = 1;
    }

    // ------------------------------------------------------------------ views

    /// Largest order (and lock) in ICT base units allowed now: 100 ICT until day 90 after
    /// Incentium genesis, 1,000 ICT until day 120, 5,000 ICT after.
    function maxOrderQty() public view returns (uint256) {
        uint256 day = block.timestamp <= genesisTime ? 0 : (block.timestamp - genesisTime) / 1 days;
        if (day < 90) return 100 * ICT_UNIT;
        if (day < 120) return 1_000 * ICT_UNIT;
        return 5_000 * ICT_UNIT;
    }

    function usdcFor(uint256 qty, uint256 price) public pure returns (uint256) {
        return qty * price / ICT_UNIT;
    }

    /// Deposit for reserving a fill worth `amount` USDC: 2 %, at least 0.5 USDC.
    function depositFor(uint256 amount) public pure returns (uint256) {
        uint256 d = amount * DEPOSIT_BPS / 10_000;
        return d < MIN_DEPOSIT ? MIN_DEPOSIT : d;
    }

    function orderId(address maker, uint256 nonce) public pure returns (bytes32) {
        return keccak256(abi.encode(maker, nonce));
    }

    function lockId(address taker, bytes32 incOrderId, uint8 fill) public pure returns (bytes32) {
        return keccak256(abi.encode(taker, incOrderId, fill));
    }

    function hashOf(bytes32 secret) public pure returns (bytes32) {
        return sha256(abi.encodePacked(secret));
    }

    function hashes(bytes32 id) external view returns (bytes32[MAX_FILLS] memory) {
        return _hashes[id];
    }

    // ------------------------------------------------------------------ buy orders (maker)

    /// Place a buy order and lock maxQty x price of USDC (approve this contract first).
    function placeBuy(
        uint256 nonce,
        uint128 maxQty,
        uint128 price,
        bytes32 ictRecipient,
        uint64 expiry,
        bytes32[MAX_FILLS] calldata fillHashes
    ) external nonReentrant returns (bytes32 id) {
        if (maxQty == 0 || maxQty % QTY_STEP != 0) revert BadQuantity();
        if (maxQty > maxOrderQty()) revert OverCap();
        if (price == 0) revert BadPrice();
        if (ictRecipient == bytes32(0)) revert BadRecipient();
        if (expiry <= block.timestamp || expiry > block.timestamp + ORDER_TTL_MAX) revert BadExpiry();
        uint256 total = usdcFor(maxQty, price);
        if (total < MIN_FILL_USDC) revert TooSmall();
        id = orderId(msg.sender, nonce);
        if (buyOrders[id].maker != address(0)) revert OrderExists();

        buyOrders[id] = BuyOrder({
            maker: msg.sender,
            expiry: expiry,
            open: true,
            openReservations: 0,
            usedFills: 0,
            ictRecipient: ictRecipient,
            maxQty: maxQty,
            price: price,
            remainingQty: maxQty,
            freeUsdc: uint128(total)
        });
        _hashes[id] = fillHashes;
        emit BuyPlaced(id, msg.sender, ictRecipient, maxQty, price, expiry);
        _pull(msg.sender, total);
    }

    /// Cancel: the unreserved remainder goes back to the maker. Open reservations keep their deadlines.
    function cancelBuy(bytes32 id) external nonReentrant {
        BuyOrder storage o = buyOrders[id];
        if (o.maker != msg.sender) revert NotMaker();
        if (!o.open) revert OrderNotOpen();
        _close(id, o);
    }

    /// After expiry anyone can return the unreserved remainder to the maker.
    function refundExpired(bytes32 id) external nonReentrant {
        BuyOrder storage o = buyOrders[id];
        if (!o.open) revert OrderNotOpen();
        if (block.timestamp < o.expiry) revert TooEarly();
        _close(id, o);
    }

    function _close(bytes32 id, BuyOrder storage o) private {
        uint128 refund = o.freeUsdc;
        o.open = false;
        o.freeUsdc = 0;
        o.remainingQty = 0;
        emit BuyClosed(id, refund);
        if (refund > 0) _push(o.maker, refund);
    }

    // ------------------------------------------------------------------ reservations (seller fills a buy order)

    /// Reserve `qty` of buy order `id` as fill `fill`, payable to `payee` (the seller) against hash `fill`.
    /// Anyone may call: the seller's page or a keeper. The caller pays the deposit (approve first).
    function reserve(bytes32 id, uint8 fill, uint128 qty, address payee) external nonReentrant {
        BuyOrder storage o = buyOrders[id];
        if (!o.open) revert OrderNotOpen();
        if (block.timestamp >= o.expiry) revert OrderExpired();
        if (fill >= MAX_FILLS) revert FillTaken();
        if (o.usedFills & (uint16(1) << fill) != 0) revert FillTaken();
        if (o.openReservations >= MAX_OPEN_RESERVATIONS) revert TooManyOpenReservations();
        if (payee == address(0)) revert BadRecipient();
        if (qty == 0 || qty % QTY_STEP != 0 || qty > o.remainingQty) revert BadQuantity();
        uint256 amount = usdcFor(qty, o.price);
        if (amount < MIN_FILL_USDC) revert TooSmall();
        if (openByPayee[id][payee]) revert PayeeHasOpenReservation();
        uint256 deposit = depositFor(amount);

        o.usedFills |= uint16(1) << fill;
        o.openReservations += 1;
        o.remainingQty -= qty;
        o.freeUsdc -= uint128(amount);
        openByPayee[id][payee] = true;
        uint64 expiry = uint64(block.timestamp + RESERVATION_TTL);
        reservations[id][fill] = Reservation({
            state: State.Active,
            expiry: expiry,
            payee: payee,
            qty: qty,
            amount: uint128(amount),
            depositor: msg.sender,
            depositState: DepositState.Held,
            deposit: uint128(deposit)
        });
        emit Reserved(id, fill, payee, qty, uint128(amount), expiry);
        emit DepositHeld(id, fill, msg.sender, uint128(deposit));
        _pull(msg.sender, deposit);
    }

    /// Pay a reservation to its seller with the secret the buyer revealed on Incentium.
    function claimReservation(bytes32 id, uint8 fill, bytes32 secret) external nonReentrant {
        Reservation storage r = reservations[id][fill];
        if (r.state != State.Active) revert NotActive();
        if (block.timestamp >= r.expiry) revert TooLate();
        if (hashOf(secret) != _hashes[id][fill]) revert WrongSecret();
        r.state = State.Claimed;
        buyOrders[id].openReservations -= 1;
        openByPayee[id][r.payee] = false;
        emit ReservationClaimed(id, fill, secret, r.amount);
        _push(r.payee, r.amount);
        if (r.depositState == DepositState.Held) {
            r.depositState = DepositState.Returned;
            emit DepositReturned(id, fill, r.deposit);
            _refund(r.depositor, r.deposit);
        }
    }

    /// After the deadline: quantity back to the order, or USDC back to the maker if the order is closed or
    /// expired. Anyone may call. Half the reservation deposit goes back to its payer, half is burned.
    function expireReservation(bytes32 id, uint8 fill) external nonReentrant {
        Reservation storage r = reservations[id][fill];
        if (r.state != State.Active) revert NotActive();
        if (block.timestamp < r.expiry) revert TooEarly();
        r.state = State.Expired;
        BuyOrder storage o = buyOrders[id];
        o.openReservations -= 1;
        openByPayee[id][r.payee] = false;
        emit ReservationExpired(id, fill);
        if (o.open && block.timestamp < o.expiry) {
            o.remainingQty += r.qty;
            o.freeUsdc += r.amount;
        } else {
            _push(o.maker, r.amount);
        }
        if (r.depositState == DepositState.Held) {
            r.depositState = DepositState.HalfBurned;
            uint128 back = r.deposit / 2;
            uint128 burned = r.deposit - back;
            burnedDeposits += burned;
            emit DepositHalfBurned(id, fill, back, burned);
            _refund(r.depositor, back);
        }
    }

    // ------------------------------------------------------------------ locks (buyer fills a sell order on Incentium)

    /// Lock USDC for fill `fill` of sell order `incOrderId` on Incentium, under that fill's hash,
    /// payable to the sell maker's Base address. Only lock after the reservation exists on Incentium.
    function lock(
        bytes32 incOrderId,
        uint8 fill,
        bytes32 hash,
        address payee,
        uint128 qty,
        uint128 price,
        uint64 expiry
    ) external nonReentrant returns (bytes32 id) {
        if (payee == address(0) || hash == bytes32(0)) revert BadRecipient();
        if (qty == 0 || qty % QTY_STEP != 0) revert BadQuantity();
        if (qty > maxOrderQty()) revert OverCap();
        if (price == 0) revert BadPrice();
        if (expiry <= block.timestamp || expiry > block.timestamp + LOCK_TTL_MAX) revert BadExpiry();
        uint256 amount = usdcFor(qty, price);
        if (amount < MIN_FILL_USDC) revert TooSmall();
        id = lockId(msg.sender, incOrderId, fill);
        if (locks[id].state != State.None) revert LockExists();

        locks[id] = Lock({
            state: State.Active,
            expiry: expiry,
            taker: msg.sender,
            payee: payee,
            qty: qty,
            amount: uint128(amount),
            hash: hash,
            incOrderId: incOrderId,
            fill: fill
        });
        emit Locked(id, incOrderId, fill, msg.sender, payee, hash, qty, uint128(amount), expiry);
        _pull(msg.sender, amount);
    }

    /// The sell maker's confirmation: reveals the secret and pays the lock to the maker's Base address.
    function claimLock(bytes32 id, bytes32 secret) external nonReentrant {
        Lock storage l = locks[id];
        if (l.state != State.Active) revert NotActive();
        if (block.timestamp >= l.expiry) revert TooLate();
        if (hashOf(secret) != l.hash) revert WrongSecret();
        l.state = State.Claimed;
        emit LockClaimed(id, secret, l.amount);
        _push(l.payee, l.amount);
    }

    /// After the deadline the USDC goes back to the buyer.
    function refundLock(bytes32 id) external nonReentrant {
        Lock storage l = locks[id];
        if (l.state != State.Active) revert NotActive();
        if (block.timestamp < l.expiry) revert TooEarly();
        l.state = State.Expired;
        emit LockRefunded(id);
        _push(l.taker, l.amount);
    }

    // ------------------------------------------------------------------ owed refunds

    /// Pay `to` the deposit refunds USDC refused earlier (e.g. while `to` was blacklisted). Anyone may call.
    function claimOwed(address to) external nonReentrant {
        uint256 a = owed[to];
        owed[to] = 0;
        emit OwedPaid(to, a);
        if (a > 0) _push(to, a);
    }

    // ------------------------------------------------------------------ internals

    /// A deposit refund never blocks a claim or an expiry: if USDC refuses it, it is owed instead.
    function _refund(address to, uint256 amount) private {
        if (amount == 0) return;
        (bool ok, bytes memory ret) = address(usdc).call(abi.encodeCall(IERC20.transfer, (to, amount)));
        if (!ok || (ret.length > 0 && !abi.decode(ret, (bool)))) {
            owed[to] += amount;
            emit RefundOwed(to, amount);
        }
    }

    function _pull(address from, uint256 amount) private {
        if (!usdc.transferFrom(from, address(this), amount)) revert TransferFailed();
    }

    function _push(address to, uint256 amount) private {
        if (!usdc.transfer(to, amount)) revert TransferFailed();
    }
}

interface IERC20 {
    function transfer(address to, uint256 amount) external returns (bool);
    function transferFrom(address from, address to, uint256 amount) external returns (bool);
    function balanceOf(address who) external view returns (uint256);
}
