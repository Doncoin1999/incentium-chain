//! The pallets benchmarked by scripts/run-benchmarks.sh (frame-omni-bencher).
polkadot_sdk::frame_benchmarking::define_benchmarks!(
    [pallet_ict_emission, Emission]
    [pallet_subnet_registry, Registry]
    [pallet_weights, Weights]
    [pallet_work_verify, WorkVerify]
    [pallet_ict_consensus, Consensus]
);
