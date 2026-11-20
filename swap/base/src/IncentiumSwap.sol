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
/// No owner, no upgrade. A pause guardian can stop new orders, reservations and locks only.
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
    uint256 public constant FEE_BPS = 10; // 0.1 %

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

    struct Reservation {
        State state;
        uint64 expiry;
        address payee; // the seller's Base address
        uint128 qty;
        uint128 amount; // USDC
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
    address public immutable feeRecipient;
    address public immutable pauseGuardian;
    bool public paused;

    mapping(bytes32 => BuyOrder) public buyOrders;
    mapping(bytes32 => bytes32[MAX_FILLS]) internal _hashes;
    mapping(bytes32 => mapping(uint8 => Reservation)) public reservations;
    mapping(bytes32 => Lock) public locks;

    uint256 private _entered = 1;

    // ------------------------------------------------------------------ events

    event BuyPlaced(
        bytes32 indexed orderId, address indexed maker, bytes32 ictRecipient, uint128 maxQty, uint128 price, uint64 expiry
    );
    event BuyClosed(bytes32 indexed orderId, uint128 refunded);
    event Reserved(bytes32 indexed orderId, uint8 indexed fill, address payee, uint128 qty, uint128 amount, uint64 expiry);
    event ReservationClaimed(bytes32 indexed orderId, uint8 indexed fill, bytes32 secret, uint128 paid, uint128 fee);
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
    event LockClaimed(bytes32 indexed lockId, bytes32 secret, uint128 paid, uint128 fee);
    event LockRefunded(bytes32 indexed lockId);
    event Paused(bool paused);

    // ------------------------------------------------------------------ errors

    error IsPaused();
    error NotGuardian();
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

    constructor(IERC20 usdc_, uint256 genesisTime_, address feeRecipient_, address pauseGuardian_) {
        if (address(usdc_) == address(0) || feeRecipient_ == address(0)) revert BadRecipient();
        usdc = usdc_;
        genesisTime = genesisTime_;
        feeRecipient = feeRecipient_;
        pauseGuardian = pauseGuardian_;
    }

    modifier nonReentrant() {
        if (_entered != 1) revert Reentrancy();
        _entered = 2;
        _;
        _entered = 1;
    }

    modifier whenNotPaused() {
        if (paused) revert IsPaused();
        _;
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
    ) external nonReentrant whenNotPaused returns (bytes32 id) {
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
    /// Anyone may call: the seller's page or a keeper.
    function reserve(bytes32 id, uint8 fill, uint128 qty, address payee) external nonReentrant whenNotPaused {
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

        o.usedFills |= uint16(1) << fill;
        o.openReservations += 1;
        o.remainingQty -= qty;
        o.freeUsdc -= uint128(amount);
        uint64 expiry = uint64(block.timestamp + RESERVATION_TTL);
        reservations[id][fill] =
            Reservation({state: State.Active, expiry: expiry, payee: payee, qty: qty, amount: uint128(amount)});
        emit Reserved(id, fill, payee, qty, uint128(amount), expiry);
    }

    /// Pay a reservation to its seller with the secret the buyer revealed on Incentium.
    function claimReservation(bytes32 id, uint8 fill, bytes32 secret) external nonReentrant {
        Reservation storage r = reservations[id][fill];
        if (r.state != State.Active) revert NotActive();
        if (block.timestamp >= r.expiry) revert TooLate();
        if (hashOf(secret) != _hashes[id][fill]) revert WrongSecret();
        r.state = State.Claimed;
        buyOrders[id].openReservations -= 1;
        (uint128 paid, uint128 fee) = _split(r.amount);
        emit ReservationClaimed(id, fill, secret, paid, fee);
        _push(r.payee, paid);
        if (fee > 0) _push(feeRecipient, fee);
    }

    /// After the deadline: quantity back to the order, or USDC back to the maker if the order is closed or expired.
    function expireReservation(bytes32 id, uint8 fill) external nonReentrant {
        Reservation storage r = reservations[id][fill];
        if (r.state != State.Active) revert NotActive();
        if (block.timestamp < r.expiry) revert TooEarly();
        r.state = State.Expired;
        BuyOrder storage o = buyOrders[id];
        o.openReservations -= 1;
        emit ReservationExpired(id, fill);
        if (o.open && block.timestamp < o.expiry) {
            o.remainingQty += r.qty;
            o.freeUsdc += r.amount;
        } else {
            _push(o.maker, r.amount);
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
    ) external nonReentrant whenNotPaused returns (bytes32 id) {
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
        (uint128 paid, uint128 fee) = _split(l.amount);
        emit LockClaimed(id, secret, paid, fee);
        _push(l.payee, paid);
        if (fee > 0) _push(feeRecipient, fee);
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

    // ------------------------------------------------------------------ pause

    /// Stops new orders, reservations and locks. Claims, refunds and cancellations always work.
    function setPaused(bool p) external {
        if (msg.sender != pauseGuardian) revert NotGuardian();
        paused = p;
        emit Paused(p);
    }

    // ------------------------------------------------------------------ internals

    function _split(uint128 amount) private pure returns (uint128 paid, uint128 fee) {
        fee = uint128(uint256(amount) * FEE_BPS / 10_000);
        paid = amount - fee;
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
