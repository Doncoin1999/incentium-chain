use ipm_runtime::WASM_BINARY;
use polkadot_sdk::*;
use sc_service::ChainType;

/// Specialized `ChainSpec`. This is a specialization of the general Substrate ChainSpec type.
pub type ChainSpec = sc_service::GenericChainSpec;

pub fn development_chain_spec() -> Result<ChainSpec, String> {
    Ok(ChainSpec::builder(
        WASM_BINARY.ok_or_else(|| "Development wasm not available".to_string())?,
        None,
    )
    .with_name("IPM Development")
    .with_id("ipm_dev")
    .with_chain_type(ChainType::Development)
    .with_genesis_config_preset_name(sp_genesis_builder::DEV_RUNTIME_PRESET)
    .with_properties(properties())
    .build())
}

pub fn local_chain_spec() -> Result<ChainSpec, String> {
    Ok(ChainSpec::builder(
        WASM_BINARY.ok_or_else(|| "Development wasm not available".to_string())?,
        None,
    )
    .with_name("IPM Devnet")
    .with_id("ipm_devnet")
    .with_chain_type(ChainType::Local)
    .with_genesis_config_preset_name(sp_genesis_builder::LOCAL_TESTNET_RUNTIME_PRESET)
    .with_properties(properties())
    .build())
}

/// Token properties shown by wallets and explorers.
fn properties() -> sc_service::Properties {
    let mut p = sc_service::Properties::new();
    p.insert("tokenSymbol".into(), "IPM".into());
    p.insert("tokenDecimals".into(), 9.into());
    p.insert("ss58Format".into(), 42.into());
    p
}

/// Mainnet bootstrap template (placeholder keys). Use `scripts/make_bootstrap_spec.py`
/// to put in the operator's real keys before launch.
pub fn bootstrap_chain_spec() -> Result<ChainSpec, String> {
    Ok(ChainSpec::builder(
        WASM_BINARY.ok_or_else(|| "Wasm runtime not available".to_string())?,
        None,
    )
    .with_name("IPM")
    .with_id("ipm")
    .with_chain_type(ChainType::Live)
    .with_genesis_config_preset_name(ipm_runtime::genesis_config_presets::BOOTSTRAP_PRESET)
    .with_properties(properties())
    .build())
}
