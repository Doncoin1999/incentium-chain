//! # ICT emission pallet (spec Section 3)
//!
//! Every block: issue the scheduled reward (halving triggered by total issued),
//! mint `FundShare` of it to the development fund (an on-chain treasury account)
//! and the rest to the subnet pot, which `utility-metrics` will later allocate.
//! In Phase 1 the pot belongs to SN0.
#![cfg_attr(not(feature = "std"), no_std)]

pub use pallet::*;
pub mod weights;
pub use weights::WeightInfo;
#[cfg(feature = "runtime-benchmarks")]
mod benchmarking;

#[cfg(test)]
mod mock;
#[cfg(test)]
mod tests;

#[polkadot_sdk::frame_support::pallet]
pub mod pallet {
    use crate::WeightInfo;
    use ict_economics::emission::{self, EmissionState};
    use polkadot_sdk::frame_support::{pallet_prelude::*, traits::fungible::Mutate};
    use polkadot_sdk::frame_system::pallet_prelude::*;

    /// Upper bound for the fund share, in ppm (20 %).
    pub const MAX_FUND_SHARE_PPM: u32 = 200_000;

    #[pallet::pallet]
    pub struct Pallet<T>(_);

    #[pallet::config]
    pub trait Config: polkadot_sdk::frame_system::Config {
        #[allow(deprecated)]
        type RuntimeEvent: From<Event<Self>>
            + IsType<<Self as polkadot_sdk::frame_system::Config>::RuntimeEvent>;
        /// Weights of this pallet's calls (benchmarked in the runtime).
        type WeightInfo: WeightInfo;
        /// ICT balances; the protocol mints through this.
        type Currency: Mutate<Self::AccountId, Balance = u128>;
        /// On-chain treasury that receives the development-fund share.
        #[pallet::constant]
        type FundAccount: Get<Self::AccountId>;
        /// Account holding issuance for subnets until it is allocated.
        #[pallet::constant]
        type SubnetPot: Get<Self::AccountId>;
    }

    /// ICT issued since genesis, in base units (1 ICT = 10^9).
    #[pallet::storage]
    pub type TotalIssued<T> = StorageValue<_, u128, ValueQuery>;

    /// Number of halvings so far.
    #[pallet::storage]
    pub type Halvings<T> = StorageValue<_, u8, ValueQuery>;

    #[pallet::type_value]
    pub fn DefaultFundShare() -> u32 {
        100_000
    }

    /// Development fund share of issuance, in ppm (proposed 10 %).
    #[pallet::storage]
    pub type FundShare<T> = StorageValue<_, u32, ValueQuery, DefaultFundShare>;

    #[pallet::genesis_config]
    #[derive(polkadot_sdk::frame_support::DefaultNoBound)]
    pub struct GenesisConfig<T: Config> {
        /// ICT endowed in genesis (bootstrap allocation), counted against the 21M cap.
        pub pre_issued: u128,
        #[serde(skip)]
        pub _phantom: core::marker::PhantomData<T>,
    }

    #[pallet::genesis_build]
    impl<T: Config> BuildGenesisConfig for GenesisConfig<T> {
        fn build(&self) {
            assert!(
                self.pre_issued < emission::next_threshold(0),
                "genesis allocation too large"
            );
            TotalIssued::<T>::put(self.pre_issued);
        }
    }

    #[pallet::event]
    #[pallet::generate_deposit(pub(super) fn deposit_event)]
    pub enum Event<T: Config> {
        /// A halving happened; `halvings` is the new count.
        Halved { halvings: u8, total_issued: u128 },
        /// The fund share was changed by governance.
        FundShareSet { ppm: u32 },
    }

    #[pallet::error]
    pub enum Error<T> {
        /// The fund share is above the allowed maximum.
        FundShareTooHigh,
    }

    #[pallet::hooks]
    impl<T: Config> Hooks<BlockNumberFor<T>> for Pallet<T> {
        fn on_initialize(_n: BlockNumberFor<T>) -> Weight {
            Self::issue_block();
            <T as Config>::WeightInfo::on_initialize()
        }
    }

    #[pallet::call]
    impl<T: Config> Pallet<T> {
        /// Change the development-fund share (governance only).
        #[pallet::call_index(0)]
        #[pallet::weight(<T as Config>::WeightInfo::set_fund_share())]
        pub fn set_fund_share(origin: OriginFor<T>, ppm: u32) -> DispatchResult {
            ensure_root(origin)?;
            ensure!(ppm <= MAX_FUND_SHARE_PPM, Error::<T>::FundShareTooHigh);
            FundShare::<T>::put(ppm);
            Self::deposit_event(Event::FundShareSet { ppm });
            Ok(())
        }
    }

    impl<T: Config> Pallet<T> {
        /// Issue one block's reward and mint it. Returns the amount issued.
        pub fn issue_block() -> u128 {
            let mut s = EmissionState {
                total_issued: TotalIssued::<T>::get(),
                halvings: Halvings::<T>::get(),
            };
            let before = s.halvings;
            let issued = emission::issue_block(&mut s);
            if issued == 0 {
                return 0;
            }
            let split = emission::split(issued, FundShare::<T>::get() as u128);
            // Minting can only fail on overflow of total issuance, which the cap rules out.
            let _ = T::Currency::mint_into(&T::FundAccount::get(), split.fund);
            let _ = T::Currency::mint_into(&T::SubnetPot::get(), split.subnets);
            TotalIssued::<T>::put(s.total_issued);
            if s.halvings != before {
                Halvings::<T>::put(s.halvings);
                Self::deposit_event(Event::Halved {
                    halvings: s.halvings,
                    total_issued: s.total_issued,
                });
            }
            issued
        }
    }
}
