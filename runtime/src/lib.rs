//! ICT runtime (devnet). Aura block production, Grandpa finality, ICT balances and
//! the Incentium pallets: emission, subnet registry, weights, work-verify and consensus.
#![cfg_attr(not(feature = "std"), no_std)]
#![recursion_limit = "256"]

#[cfg(feature = "std")]
include!(concat!(env!("OUT_DIR"), "/wasm_binary.rs"));

#[cfg(feature = "runtime-benchmarks")]
#[macro_use]
mod benchmarks;
pub mod apis;
pub mod block_production;
pub mod configs;
pub mod genesis_config_presets;
pub mod governance;
pub mod subnet_api;
pub mod weights;

#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_npos;

extern crate alloc;
use alloc::vec::Vec;
use polkadot_sdk::*;

use sp_runtime::{
    generic, impl_opaque_keys,
    traits::{BlakeTwo256, IdentifyAccount, Verify},
    MultiAddress, MultiSignature,
};
use sp_version::RuntimeVersion;

pub use frame_system::Call as SystemCall;
pub use pallet_balances::Call as BalancesCall;
pub use pallet_timestamp::Call as TimestampCall;
#[cfg(any(feature = "std", test))]
pub use sp_runtime::BuildStorage;

pub mod opaque {
    use super::*;
    use sp_runtime::traits::Hash as HashT;

    pub use sp_runtime::OpaqueExtrinsic as UncheckedExtrinsic;
    pub type Header = generic::Header<BlockNumber, BlakeTwo256>;
    pub type Block = generic::Block<Header, UncheckedExtrinsic>;
    pub type BlockId = generic::BlockId<Block>;
    pub type Hash = <BlakeTwo256 as HashT>::Output;
}

impl_opaque_keys! {
    pub struct SessionKeys {
        pub aura: Aura,
        pub grandpa: Grandpa,
    }
}

#[sp_version::runtime_version]
pub const VERSION: RuntimeVersion = RuntimeVersion {
    spec_name: alloc::borrow::Cow::Borrowed("incentium"),
    impl_name: alloc::borrow::Cow::Borrowed("incentium-reference"),
    authoring_version: 1,
    // 9: pallet-revive (EVM). 10: subnet registration, issuance sharing, subnet control.
    // 11: Emergency track (authorize a runtime upgrade by hash).
    spec_version: 13,
    impl_version: 1,
    apis: apis::RUNTIME_API_VERSIONS,
    // 2: the transaction extensions changed (EthSetOrigin added).
    // 3: spec 10 changed registry.register_subnet and call index 3 (now try_unlock).
    transaction_version: 3,
    system_version: 1,
};

/// 12-second blocks (spec Section 1). The slot duration cannot change after launch.
pub const MILLI_SECS_PER_BLOCK: u64 = 12_000;
pub const SLOT_DURATION: u64 = MILLI_SECS_PER_BLOCK;

pub const MINUTES: BlockNumber = 60_000 / (MILLI_SECS_PER_BLOCK as BlockNumber);
pub const HOURS: BlockNumber = MINUTES * 60;
pub const DAYS: BlockNumber = HOURS * 24;
/// 360 blocks = 72 minutes. The `fast-epochs` feature (devnet and CI only) uses 10 blocks.
#[cfg(not(feature = "fast-epochs"))]
pub const EPOCH_LENGTH: BlockNumber = 360;
#[cfg(feature = "fast-epochs")]
pub const EPOCH_LENGTH: BlockNumber = 10;

/// 1 ICT = 10^9 base units.
pub const UNIT: Balance = ict_economics::UNIT;
pub const MILLI_UNIT: Balance = UNIT / 1_000;
pub const EXISTENTIAL_DEPOSIT: Balance = MILLI_UNIT;

pub type Signature = MultiSignature;
pub type AccountId = <<Signature as Verify>::Signer as IdentifyAccount>::AccountId;
pub type Balance = u128;
pub type Nonce = u32;
pub type Hash = sp_core::H256;
pub type BlockNumber = u32;
pub type Address = MultiAddress<AccountId, ()>;
pub type Header = generic::Header<BlockNumber, BlakeTwo256>;
pub type Block = generic::Block<Header, UncheckedExtrinsic>;
pub type SignedBlock = generic::SignedBlock<Block>;
pub type BlockId = generic::BlockId<Block>;

pub type TxExtension = (
    frame_system::CheckNonZeroSender<Runtime>,
    frame_system::CheckSpecVersion<Runtime>,
    frame_system::CheckTxVersion<Runtime>,
    frame_system::CheckGenesis<Runtime>,
    frame_system::CheckEra<Runtime>,
    frame_system::CheckNonce<Runtime>,
    frame_system::CheckWeight<Runtime>,
    pallet_transaction_payment::ChargeTransactionPayment<Runtime>,
    frame_metadata_hash_extension::CheckMetadataHash<Runtime>,
    // EVM (pallet-revive): turns the origin of an Ethereum transaction into an EVM origin.
    // Must come after every extension that relies on a signed origin. It encodes to zero
    // bytes, so ordinary Substrate transactions are unaffected apart from the metadata.
    pallet_revive::evm::tx_extension::SetOrigin<Runtime>,
    frame_system::WeightReclaim<Runtime>,
);

/// The extensions the runtime applies to an Ethereum transaction (signed in MetaMask and
/// submitted through the `eth-rpc` server). Must list the same extensions as `TxExtension`.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct EthExtraImpl;

impl pallet_revive::evm::runtime::EthExtra for EthExtraImpl {
    type Config = Runtime;
    type ExtensionV0 = TxExtension;
    type ExtensionOtherVersions = sp_runtime::traits::InvalidVersion;

    fn get_eth_extension(nonce: Nonce, tip: Balance) -> Self::ExtensionV0 {
        (
            frame_system::CheckNonZeroSender::<Runtime>::new(),
            frame_system::CheckSpecVersion::<Runtime>::new(),
            frame_system::CheckTxVersion::<Runtime>::new(),
            frame_system::CheckGenesis::<Runtime>::new(),
            frame_system::CheckMortality::from(generic::Era::Immortal),
            frame_system::CheckNonce::<Runtime>::from(nonce),
            frame_system::CheckWeight::<Runtime>::new(),
            pallet_transaction_payment::ChargeTransactionPayment::<Runtime>::from(tip),
            frame_metadata_hash_extension::CheckMetadataHash::<Runtime>::new(false),
            pallet_revive::evm::tx_extension::SetOrigin::<Runtime>::new_from_eth_transaction(),
            frame_system::WeightReclaim::<Runtime>::new(),
        )
    }
}

/// Accepts both ordinary Substrate extrinsics and Ethereum transactions (`eth_transact`).
pub type UncheckedExtrinsic =
    pallet_revive::evm::runtime::UncheckedExtrinsic<Address, Signature, EthExtraImpl>;
pub type SignedPayload = generic::SignedPayload<RuntimeCall, TxExtension>;

/// One-off for chains already running (testnet, mainnet): pallet-revive needs its own account
/// to exist. On a new chain its genesis build mints the existential deposit (counted in
/// `BOOTSTRAP_PRE_ISSUED`). Here the deposit is moved from the development fund instead of
/// minted, so total issuance, `TotalIssued` and the 21M cap are unaffected.
pub struct CreateReviveAccount;
impl frame_support::traits::OnRuntimeUpgrade for CreateReviveAccount {
    fn on_runtime_upgrade() -> frame_support::weights::Weight {
        use frame_support::traits::{fungible::Mutate, tokens::Preservation};
        let db = <Runtime as frame_system::Config>::DbWeight::get();
        let revive = pallet_revive::Pallet::<Runtime>::account_id();
        if frame_system::Pallet::<Runtime>::account_exists(&revive) {
            return db.reads(1);
        }
        let fund = pallet_treasury::Pallet::<Runtime>::account_id();
        // If the fund cannot pay, the upgrade still succeeds; send 0.001 ICT to the revive
        // account by hand before using the EVM (docs/EVM.md).
        let _ = <Balances as Mutate<AccountId>>::transfer(
            &fund,
            &revive,
            EXISTENTIAL_DEPOSIT,
            Preservation::Preserve,
        );
        db.reads_writes(3, 3)
    }
}

#[allow(unused_parens)]
/// Runtime 12: open block production (docs/OPEN-BLOCK-PRODUCTION.md).
type Migrations = (CreateReviveAccount, block_production::InitBlockProduction);

pub type Executive = frame_executive::Executive<
    Runtime,
    Block,
    frame_system::ChainContext<Runtime>,
    Runtime,
    AllPalletsWithSystem,
    Migrations,
>;

#[frame_support::runtime]
mod runtime {
    #[runtime::runtime]
    #[runtime::derive(
        RuntimeCall,
        RuntimeEvent,
        RuntimeError,
        RuntimeOrigin,
        RuntimeFreezeReason,
        RuntimeHoldReason,
        RuntimeSlashReason,
        RuntimeLockId,
        RuntimeTask,
        RuntimeViewFunction
    )]
    pub struct Runtime;

    #[runtime::pallet_index(0)]
    pub type System = frame_system;
    #[runtime::pallet_index(1)]
    pub type Timestamp = pallet_timestamp;
    #[runtime::pallet_index(2)]
    pub type Aura = pallet_aura;
    #[runtime::pallet_index(3)]
    pub type Grandpa = pallet_grandpa;
    #[runtime::pallet_index(4)]
    pub type Balances = pallet_balances;
    #[runtime::pallet_index(5)]
    pub type TransactionPayment = pallet_transaction_payment;
    /// Launch period only: the runtime removes the key when the period ends (governance.rs).
    #[runtime::pallet_index(6)]
    pub type Sudo = pallet_sudo;

    #[runtime::pallet_index(10)]
    pub type Emission = pallet_ict_emission;
    #[runtime::pallet_index(11)]
    pub type Registry = pallet_subnet_registry;
    #[runtime::pallet_index(12)]
    pub type Weights = pallet_weights;
    #[runtime::pallet_index(13)]
    pub type WorkVerify = pallet_work_verify;
    #[runtime::pallet_index(14)]
    pub type Consensus = pallet_ict_consensus;
    /// ICT <-> USDC atomic swaps with Base (Incentium Swap spec v0.1).
    #[runtime::pallet_index(15)]
    pub type Swap = pallet_ict_swap;
    #[runtime::pallet_index(16)]
    pub type Endpoints = pallet_endpoints;

    // Governance and the development fund (governance.rs).
    #[runtime::pallet_index(20)]
    pub type Scheduler = pallet_scheduler;
    #[runtime::pallet_index(21)]
    pub type Preimage = pallet_preimage;
    #[runtime::pallet_index(22)]
    pub type Treasury = pallet_treasury;
    #[runtime::pallet_index(23)]
    pub type Bounties = pallet_bounties;
    #[runtime::pallet_index(24)]
    pub type ChildBounties = pallet_child_bounties;
    #[runtime::pallet_index(25)]
    pub type Multisig = pallet_multisig;
    #[runtime::pallet_index(26)]
    pub type ConvictionVoting = pallet_conviction_voting;
    #[runtime::pallet_index(27)]
    pub type Referenda = pallet_referenda;
    #[runtime::pallet_index(28)]
    pub type Origins = governance::pallet_custom_origins;
    #[runtime::pallet_index(29)]
    pub type Launch = governance::pallet_launch;

    /// EVM and smart contracts (Solidity on the REVM backend), used by the Hyperlane bridge.
    #[runtime::pallet_index(30)]
    pub type Revive = pallet_revive;

    #[runtime::pallet_index(31)]
    pub type Emergency = governance::pallet_emergency;
    /// Parameters track: protocol parameters from a fixed list (runtime 12).
    #[runtime::pallet_index(34)]
    pub type Parameters = governance::pallet_parameters;

    // Open block production (block_production.rs). BlockProducers first, so its genesis is
    // built before Session asks it for the first set.
    #[runtime::pallet_index(40)]
    pub type BlockProducers = pallet_block_producers;
    #[runtime::pallet_index(41)]
    pub type Authorship = pallet_authorship;
    #[runtime::pallet_index(42)]
    pub type Staking = pallet_staking;
    #[runtime::pallet_index(43)]
    pub type Offences = pallet_offences;
    #[runtime::pallet_index(44)]
    pub type Historical = pallet_session::historical;
    #[runtime::pallet_index(45)]
    pub type Session = pallet_session;
    #[runtime::pallet_index(46)]
    pub type VoterList = pallet_bags_list<Instance1>;

    /// Alpha tokens and subnet pools (spec Section 7, decided 25-26 Nov 2026), wired to the
    /// registry and consensus. A pool exists only for a subnet sold after this upgrade.
    #[runtime::pallet_index(32)]
    pub type SubnetPools = pallet_subnet_pools;
}
