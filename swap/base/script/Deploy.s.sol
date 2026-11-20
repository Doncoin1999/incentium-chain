// SPDX-License-Identifier: Apache-2.0
pragma solidity 0.8.28;

import {Script, console} from "forge-std/Script.sol";
import {IncentiumSwap, IERC20} from "../src/IncentiumSwap.sol";

/// Deploy IncentiumSwap.
///   USDC            USDC on Base: 0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913 (Base Sepolia: 0x036CbD53842c5426634e7929541eC2318f3dCF7e)
///   GENESIS_TIME    unix time of Incentium mainnet block 1 (explorer: /api/block/1 -> ts)
///   FEE_RECIPIENT   governance multisig on Base that receives the 0.1 % fee
///   PAUSE_GUARDIAN  multisig that may pause new orders, reservations and locks
/// forge script script/Deploy.s.sol --rpc-url $BASE_RPC --broadcast --private-key $DEPLOYER_KEY
contract Deploy is Script {
    function run() external returns (IncentiumSwap swap) {
        address usdc = vm.envAddress("USDC");
        uint256 genesis = vm.envUint("GENESIS_TIME");
        address feeRecipient = vm.envAddress("FEE_RECIPIENT");
        address guardian = vm.envAddress("PAUSE_GUARDIAN");
        vm.startBroadcast();
        swap = new IncentiumSwap(IERC20(usdc), genesis, feeRecipient, guardian);
        vm.stopBroadcast();
        console.log("IncentiumSwap deployed at", address(swap));
    }
}
