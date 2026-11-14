fn main() {
    // Set SKIP_WASM_BUILD=1 to build only the native runtime (for tests without the wasm target).
    #[cfg(feature = "std")]
    substrate_wasm_builder::WasmBuilder::build_using_defaults();
}
