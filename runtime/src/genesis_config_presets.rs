//! Genesis presets for the devnet.
//! - `development`: one authority (Alice), sudo Alice.
//! - `local_testnet`: three authorities (Alice, Bob, Charlie) for the closed devnet.
//! - `bootstrap`: mainnet template with a 90-day disclosed bootstrap period run by one
//!   operator; keys are replaced before launch.
//!
//! SN0 is created by the registry genesis; Phase 2 starts disabled.

use crate::{
    AccountId, BalancesConfig, EmissionConfig, RegistryConfig, RuntimeGenesisConfig, SudoConfig,
    DAYS, UNIT,
};
use alloc::{vec, vec::Vec};
use frame_support::build_struct_json_patch;
use polkadot_sdk::*;
use serde_json::Value;
use sp_consensus_aura::sr25519::AuthorityId as AuraId;
use sp_consensus_grandpa::AuthorityId as GrandpaId;
use sp_genesis_builder::{self, PresetId};
use sp_keyring::{Ed25519Keyring, Sr25519Keyring};

/// Devnet accounts start with 100,000 ICT each (devnet only; test coins).
const DEVNET_ENDOWMENT: u128 = 100_000 * UNIT;

fn devnet_genesis(
    authorities: Vec<(AuraId, GrandpaId)>,
    endowed: Vec<AccountId>,
    root: AccountId,
) -> Value {
    build_struct_json_patch!(RuntimeGenesisConfig {
        balances: BalancesConfig {
            balances: endowed
                .iter()
                .cloned()
                .map(|k| (k, DEVNET_ENDOWMENT))
                .collect::<Vec<_>>(),
        },
        aura: pallet_aura::GenesisConfig {
            authorities: authorities.iter().map(|x| x.0.clone()).collect::<Vec<_>>()
        },
        grandpa: pallet_grandpa::GenesisConfig {
            authorities: authorities
                .iter()
                .map(|x| (x.1.clone(), 1))
                .collect::<Vec<_>>(),
        },
        sudo: SudoConfig { key: Some(root) },
        registry: RegistryConfig {
            phase2_enabled: false
        },
    })
}

/// Preset id of the mainnet bootstrap template.
pub const BOOTSTRAP_PRESET: &str = "bootstrap";
/// Disclosed bootstrap period: 90 days.
pub const BOOTSTRAP_BLOCKS: u32 = 90 * DAYS;
/// Genesis allocation to the operator for transaction fees and initial validator stake,
/// counted against the 21M cap. This and the bootstrap issuance are disclosed in the whitepaper.
pub const BOOTSTRAP_ALLOCATION: u128 = 100 * UNIT;

/// Mainnet bootstrap template. The dev keys are placeholders: `scripts/make_bootstrap_spec.py`
/// replaces them with the operator's own keys before launch.
fn bootstrap_genesis(authorities: Vec<(AuraId, GrandpaId)>, operator: AccountId) -> Value {
    build_struct_json_patch!(RuntimeGenesisConfig {
        balances: BalancesConfig {
            balances: vec![(operator.clone(), BOOTSTRAP_ALLOCATION)]
        },
        aura: pallet_aura::GenesisConfig {
            authorities: authorities.iter().map(|x| x.0.clone()).collect::<Vec<_>>()
        },
        grandpa: pallet_grandpa::GenesisConfig {
            authorities: authorities
                .iter()
                .map(|x| (x.1.clone(), 1))
                .collect::<Vec<_>>(),
        },
        sudo: SudoConfig {
            key: Some(operator.clone())
        },
        registry: RegistryConfig {
            phase2_enabled: false,
            bootstrap_blocks: BOOTSTRAP_BLOCKS,
            bootstrap_operator: Some(operator),
        },
        emission: EmissionConfig {
            pre_issued: BOOTSTRAP_ALLOCATION
        },
    })
}

pub fn bootstrap_template_genesis() -> Value {
    bootstrap_genesis(
        vec![
            keys(Sr25519Keyring::Alice, Ed25519Keyring::Alice),
            keys(Sr25519Keyring::Bob, Ed25519Keyring::Bob),
            keys(Sr25519Keyring::Charlie, Ed25519Keyring::Charlie),
        ],
        Sr25519Keyring::Alice.to_account_id(),
    )
}

fn keys(k: Sr25519Keyring, e: Ed25519Keyring) -> (AuraId, GrandpaId) {
    (k.public().into(), e.public().into())
}

fn dev_accounts() -> Vec<AccountId> {
    Sr25519Keyring::iter()
        .filter(|v| v != &Sr25519Keyring::One && v != &Sr25519Keyring::Two)
        .map(|v| v.to_account_id())
        .collect()
}

pub fn development_config_genesis() -> Value {
    devnet_genesis(
        vec![keys(Sr25519Keyring::Alice, Ed25519Keyring::Alice)],
        dev_accounts(),
        Sr25519Keyring::Alice.to_account_id(),
    )
}

pub fn local_config_genesis() -> Value {
    devnet_genesis(
        vec![
            keys(Sr25519Keyring::Alice, Ed25519Keyring::Alice),
            keys(Sr25519Keyring::Bob, Ed25519Keyring::Bob),
            keys(Sr25519Keyring::Charlie, Ed25519Keyring::Charlie),
        ],
        dev_accounts(),
        Sr25519Keyring::Alice.to_account_id(),
    )
}

pub fn get_preset(id: &PresetId) -> Option<Vec<u8>> {
    let patch = match id.as_ref() {
        sp_genesis_builder::DEV_RUNTIME_PRESET => development_config_genesis(),
        sp_genesis_builder::LOCAL_TESTNET_RUNTIME_PRESET => local_config_genesis(),
        BOOTSTRAP_PRESET => bootstrap_template_genesis(),
        _ => return None,
    };
    Some(
        serde_json::to_string(&patch)
            .expect("serialization to json works")
            .into_bytes(),
    )
}

pub fn preset_names() -> Vec<PresetId> {
    vec![
        PresetId::from(sp_genesis_builder::DEV_RUNTIME_PRESET),
        PresetId::from(sp_genesis_builder::LOCAL_TESTNET_RUNTIME_PRESET),
        PresetId::from(BOOTSTRAP_PRESET),
    ]
}
