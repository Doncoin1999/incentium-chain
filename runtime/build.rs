fn main() {
    // Set SKIP_WASM_BUILD=1 to build only the native runtime (for tests without the wasm target).
    // With `metadata-hash` (on by default via the node), the wasm carries the metadata hash that
    // CheckMetadataHash needs, so Ledger and Talisman can verify what they sign.
    #[cfg(all(feature = "std", feature = "metadata-hash"))]
    substrate_wasm_builder::WasmBuilder::init_with_defaults()
        .enable_metadata_hash("ICT", 9)
        .build();
    #[cfg(all(feature = "std", not(feature = "metadata-hash")))]
    substrate_wasm_builder::WasmBuilder::build_using_defaults();
}
