//! Data for the `SubnetRegistryApi` runtime API (incentium.org's subnet page).

use alloc::vec::Vec;
use pallet_subnet_registry::SubnetSummary;

use crate::{AccountId, Runtime};

/// Every subnet with the share of all issuance it gets now, in ppm: the pot's allocation
/// (consensus) times the part of issuance that reaches the pot (1 - fund share).
pub fn subnets_with_emission() -> Vec<SubnetSummary<AccountId>> {
    const PPM: u128 = 1_000_000;
    let fund = pallet_ict_emission::FundShare::<Runtime>::get() as u128;
    let to_pot = PPM.saturating_sub(fund);
    let plan = pallet_ict_consensus::Pallet::<Runtime>::allocation(PPM);
    let mut out = pallet_subnet_registry::Pallet::<Runtime>::subnet_summaries();
    for s in out.iter_mut() {
        let of_pot = plan
            .iter()
            .find(|(n, _)| *n == s.netuid)
            .map(|(_, x)| *x)
            .unwrap_or(0);
        s.emission_ppm = (of_pot * to_pot / PPM) as u32;
    }
    out
}
