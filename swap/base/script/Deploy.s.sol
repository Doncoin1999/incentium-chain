// SPDX-License-Identifier: Apache-2.0
pragma solidity 0.8.28;

import {Script, console} from "forge-std/Script.sol";
import {IncentiumSwap, IERC20} from "../src/IncentiumSwap.sol";

/// Deploy IncentiumSwap.
///   USDC            USDC on Base: 0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913 (Base Sepolia: 0x036CbD53842c5426634e7929541eC2318f3dCF7e)
///   GENESIS_TIME    unix time of Incentium mainnet block 1 (explorer: /api/block/1 -> ts)
/// The contract has no fee, no pause and no privileged address; the deployer gets no rights.
/// forge script script/Deploy.s.sol --rpc-url $BASE_RPC --broadcast --private-key $DEPLOYER_KEY
contract Deploy is Script {
    function run() external returns (IncentiumSwap swap) {
        address usdc = vm.envAddress("USDC");
        uint256 genesis = vm.envUint("GENESIS_TIME");
        vm.startBroadcast();
        swap = new IncentiumSwap(IERC20(usdc), genesis);
        vm.stopBroadcast();
        console.log("IncentiumSwap deployed at", address(swap));
    }
}
