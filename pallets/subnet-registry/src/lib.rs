//! # Subnet registry pallet (spec Section 4)
//!
//! - SN0 exists from genesis and has no owner.
//! - From Phase 2, anyone can open a subnet by locking a deposit whose price doubles
//!   with each registration and decays linearly to the floor over `DecayBlocks`.
//! - Neurons (miners and validators) register with a burned fee.
//! - Each neuron declares a work tier (spec Section 5). Tier weight G, spot-check tolerance,
//!   reference model and model hash per tier are on chain and set by governance; a tier
//!   can be closed (in the launch period only tier 0, the CPU tier, is open).
//! - On deregistration 90 % of the locked ICT goes back to the owner and 10 % is burned.
//!   (Pool seeding and pool withdrawal arrive with the `amm` pallet; until then the
//!   deposit is held in this pallet's account.)
#![cfg_attr(not(feature = "std"), no_std)]

pub use pallet::*;

#[cfg(test)]
mod mock;
#[cfg(test)]
mod tests;

#[polkadot_sdk::frame_support::pallet]
pub mod pallet {
    use ict_economics::registry::{deposit_price, deregistration_split};
    use polkadot_sdk::frame_support::{
        pallet_prelude::*,
        traits::{
            fungible::Mutate,
            tokens::{Fortitude, Precision, Preservation},
        },
        PalletId,
    };
    use polkadot_sdk::frame_system::pallet_prelude::*;
    use polkadot_sdk::sp_runtime::traits::{AccountIdConversion, SaturatedConversion};
    extern crate alloc;
    use alloc::vec::Vec;

    pub type NetUid = u16;
    pub type Uid = u16;
    pub type Tier = u8;
    pub const SN0: NetUid = 0;
    /// Tiers 0..=3: 2B (CPU entry tier), 9B, 27B, 122B.
    pub const MAX_TIER: Tier = 3;

    const STORAGE_VERSION: StorageVersion = StorageVersion::new(1);

    /// Parameters of one work tier (spec Section 5).
    #[derive(
        Clone, Encode, Decode, DecodeWithMemTracking, PartialEq, Eq, Debug, TypeInfo, MaxEncodedLen,
    )]
    pub struct TierInfo {
        /// Tier weight G in ppm (1.0 = 1,000,000).
        pub weight_ppm: u32,
        /// Maximum mean top-k log-probability distance in a spot check, in ppm.
        pub tolerance_ppm: u32,
        /// Reference model name, e.g. `Qwen3.5-2B`.
        pub model: BoundedVec<u8, ConstU32<64>>,
        /// SHA-256 of the reference weights file; all zero until governance locks it.
        pub model_hash: [u8; 32],
        /// Whether neurons may declare this tier.
        pub open: bool,
    }

    /// Tier table at genesis and at the upgrade that introduces tiers.
    pub fn default_tiers() -> [(Tier, TierInfo); 4] {
        let t = |w: u32, tol: u32, m: &[u8], open: bool| TierInfo {
            weight_ppm: w,
            tolerance_ppm: tol,
            model: BoundedVec::truncate_from(m.to_vec()),
            model_hash: [0; 32],
            open,
        };
        [
            (0, t(150_000, 350_000, b"Qwen3.5-2B", true)),
            (1, t(1_000_000, 350_000, b"Qwen3.5-9B", false)),
            (2, t(3_000_000, 250_000, b"Qwen3.5-27B", false)),
            (3, t(5_000_000, 400_000, b"Qwen3.5-122B", false)),
        ]
    }

    #[derive(
        Clone, Encode, Decode, DecodeWithMemTracking, PartialEq, Eq, Debug, TypeInfo, MaxEncodedLen,
    )]
    pub struct SubnetInfo<AccountId, BlockNumber> {
        /// `None` for SN0.
        pub owner: Option<AccountId>,
        /// ICT locked as deposit, in base units.
        pub deposit: u128,
        pub created: BlockNumber,
    }

    #[pallet::pallet]
    #[pallet::storage_version(STORAGE_VERSION)]
    pub struct Pallet<T>(_);

    #[pallet::config]
    pub trait Config: polkadot_sdk::frame_system::Config {
        #[allow(deprecated)]
        type RuntimeEvent: From<Event<Self>>
            + IsType<<Self as polkadot_sdk::frame_system::Config>::RuntimeEvent>;
        type Currency: Mutate<Self::AccountId, Balance = u128>;
        /// Holds locked subnet deposits.
        #[pallet::constant]
        type PalletId: Get<PalletId>;
        /// Floor of the subnet deposit (1,000 ICT).
        #[pallet::constant]
        type DepositFloor: Get<u128>;
        /// Blocks over which the doubled price decays to the floor (14 days).
        #[pallet::constant]
        type DecayBlocks: Get<u32>;
        /// Share of the deposit returned on deregistration, in ppm (900,000).
        #[pallet::constant]
        type ReturnPpm: Get<u32>;
        /// Burned fee to register a neuron.
        #[pallet::constant]
        type NeuronFee: Get<u128>;
        /// Maximum neurons per subnet.
        #[pallet::constant]
        type MaxNeurons: Get<u16>;
    }

    #[pallet::storage]
    pub type Subnets<T: Config> = StorageMap<
        _,
        Twox64Concat,
        NetUid,
        SubnetInfo<T::AccountId, BlockNumberFor<T>>,
        OptionQuery,
    >;

    /// Subnets excluding SN0.
    #[pallet::storage]
    pub type SubnetCount<T> = StorageValue<_, u16, ValueQuery>;

    #[pallet::type_value]
    pub fn DefaultMaxSubnets() -> u16 {
        32
    }

    #[pallet::storage]
    pub type MaxSubnets<T> = StorageValue<_, u16, ValueQuery, DefaultMaxSubnets>;

    #[pallet::storage]
    pub type LastRegPrice<T> = StorageValue<_, u128, ValueQuery>;

    #[pallet::storage]
    pub type LastRegBlock<T: Config> = StorageValue<_, BlockNumberFor<T>, OptionQuery>;

    /// Set by governance at Gate 1 (later owned by `phase-control`).
    #[pallet::storage]
    pub type Phase2Enabled<T> = StorageValue<_, bool, ValueQuery>;

    #[pallet::storage]
    pub type Neurons<T: Config> =
        StorageDoubleMap<_, Twox64Concat, NetUid, Twox64Concat, Uid, T::AccountId, OptionQuery>;

    #[pallet::storage]
    pub type NeuronUid<T: Config> =
        StorageDoubleMap<_, Twox64Concat, NetUid, Blake2_128Concat, T::AccountId, Uid, OptionQuery>;

    #[pallet::storage]
    pub type NeuronCount<T> = StorageMap<_, Twox64Concat, NetUid, u16, ValueQuery>;

    /// Public endpoint a neuron serves on (e.g. `https://1.2.3.4:8091`), so validators can
    /// find miners without an off-chain directory.
    #[pallet::storage]
    pub type Endpoints<T: Config> = StorageDoubleMap<
        _,
        Twox64Concat,
        NetUid,
        Blake2_128Concat,
        T::AccountId,
        BoundedVec<u8, ConstU32<128>>,
        OptionQuery,
    >;

    /// Work tier parameters, keyed by tier.
    #[pallet::storage]
    pub type Tiers<T> = StorageMap<_, Twox64Concat, Tier, TierInfo, OptionQuery>;

    /// Tier declared by each neuron. Neurons that never declared are tier 0.
    #[pallet::storage]
    pub type NeuronTier<T: Config> =
        StorageDoubleMap<_, Twox64Concat, NetUid, Blake2_128Concat, T::AccountId, Tier, ValueQuery>;

    /// Last block (exclusive) of the bootstrap period; 0 = no bootstrap.
    #[pallet::storage]
    pub type BootstrapUntil<T> = StorageValue<_, u32, ValueQuery>;

    /// During bootstrap only this account can register neurons (fee waived).
    #[pallet::storage]
    pub type BootstrapOperator<T: Config> = StorageValue<_, T::AccountId, OptionQuery>;

    #[pallet::genesis_config]
    #[derive(polkadot_sdk::frame_support::DefaultNoBound)]
    pub struct GenesisConfig<T: Config> {
        pub phase2_enabled: bool,
        /// Length of the disclosed bootstrap period in blocks (90 days = 648,000). 0 = none.
        pub bootstrap_blocks: u32,
        pub bootstrap_operator: Option<T::AccountId>,
        #[serde(skip)]
        pub _phantom: core::marker::PhantomData<T>,
    }

    #[pallet::genesis_build]
    impl<T: Config> BuildGenesisConfig for GenesisConfig<T> {
        fn build(&self) {
            Subnets::<T>::insert(
                SN0,
                SubnetInfo {
                    owner: None,
                    deposit: 0,
                    created: Zero::zero(),
                },
            );
            Phase2Enabled::<T>::put(self.phase2_enabled);
            if self.bootstrap_blocks > 0 {
                assert!(
                    self.bootstrap_operator.is_some(),
                    "bootstrap needs an operator"
                );
                BootstrapUntil::<T>::put(self.bootstrap_blocks);
            }
            if let Some(op) = &self.bootstrap_operator {
                BootstrapOperator::<T>::put(op.clone());
            }
            for (tier, info) in default_tiers() {
                Tiers::<T>::insert(tier, info);
            }
        }
    }

    #[pallet::hooks]
    impl<T: Config> Hooks<BlockNumberFor<T>> for Pallet<T> {
        /// v0 -> v1: write the default tier table on chains started before tiers existed.
        fn on_runtime_upgrade() -> Weight {
            if StorageVersion::get::<Pallet<T>>() >= 1 {
                return T::DbWeight::get().reads(1);
            }
            for (tier, info) in default_tiers() {
                if !Tiers::<T>::contains_key(tier) {
                    Tiers::<T>::insert(tier, info);
                }
            }
            STORAGE_VERSION.put::<Pallet<T>>();
            T::DbWeight::get().reads_writes(5, 5)
        }
    }

    #[pallet::event]
    #[pallet::generate_deposit(pub(super) fn deposit_event)]
    pub enum Event<T: Config> {
        SubnetRegistered {
            netuid: NetUid,
            owner: T::AccountId,
            deposit: u128,
        },
        SubnetDeregistered {
            netuid: NetUid,
            returned: u128,
            burned: u128,
        },
        NeuronRegistered {
            netuid: NetUid,
            uid: Uid,
            hotkey: T::AccountId,
        },
        Phase2Set {
            enabled: bool,
        },
        EndpointSet {
            netuid: NetUid,
            hotkey: T::AccountId,
        },
        /// The bootstrap period ended early; registration is open to everyone.
        BootstrapEnded {
            at: u32,
        },
        /// A neuron declared its work tier.
        TierDeclared {
            netuid: NetUid,
            hotkey: T::AccountId,
            tier: Tier,
        },
        /// Governance changed a tier's parameters.
        TierUpdated {
            tier: Tier,
        },
    }

    #[pallet::error]
    pub enum Error<T> {
        Phase2NotEnabled,
        TooManySubnets,
        DepositAboveLimit,
        NoSuchSubnet,
        CannotDeregisterSn0,
        SubnetFull,
        AlreadyRegistered,
        NotRegistered,
        EndpointTooLong,
        /// During the bootstrap period only the bootstrap operator registers neurons.
        BootstrapRestricted,
        NotInBootstrap,
        /// The tier does not exist.
        UnknownTier,
        /// The tier is closed for new declarations.
        TierClosed,
        /// The tier weight or tolerance is out of range.
        BadTierParams,
        ModelNameTooLong,
    }

    #[pallet::call]
    impl<T: Config> Pallet<T> {
        /// Open a new subnet by locking the current deposit price (Phase 2 onward).
        #[pallet::call_index(0)]
        #[pallet::weight(T::DbWeight::get().reads_writes(6, 5))]
        pub fn register_subnet(origin: OriginFor<T>, max_deposit: u128) -> DispatchResult {
            let who = ensure_signed(origin)?;
            ensure!(Phase2Enabled::<T>::get(), Error::<T>::Phase2NotEnabled);
            ensure!(
                SubnetCount::<T>::get() < MaxSubnets::<T>::get(),
                Error::<T>::TooManySubnets
            );
            let price = Self::current_deposit_price();
            ensure!(price <= max_deposit, Error::<T>::DepositAboveLimit);
            T::Currency::transfer(&who, &Self::account_id(), price, Preservation::Preserve)?;
            let netuid = Self::next_free_netuid().ok_or(Error::<T>::TooManySubnets)?;
            let now = polkadot_sdk::frame_system::Pallet::<T>::block_number();
            Subnets::<T>::insert(
                netuid,
                SubnetInfo {
                    owner: Some(who.clone()),
                    deposit: price,
                    created: now,
                },
            );
            SubnetCount::<T>::mutate(|c| *c += 1);
            LastRegPrice::<T>::put(price);
            LastRegBlock::<T>::put(now);
            Self::deposit_event(Event::SubnetRegistered {
                netuid,
                owner: who,
                deposit: price,
            });
            Ok(())
        }

        /// Close a subnet (protocol/governance). 90 % of the deposit back, 10 % burned.
        #[pallet::call_index(1)]
        #[pallet::weight(T::DbWeight::get().reads_writes(3, 3 + T::MaxNeurons::get() as u64 * 2))]
        pub fn deregister_subnet(origin: OriginFor<T>, netuid: NetUid) -> DispatchResult {
            ensure_root(origin)?;
            ensure!(netuid != SN0, Error::<T>::CannotDeregisterSn0);
            let info = Subnets::<T>::take(netuid).ok_or(Error::<T>::NoSuchSubnet)?;
            let (back, burn) = deregistration_split(info.deposit, T::ReturnPpm::get() as u128);
            let pot = Self::account_id();
            if let Some(owner) = info.owner {
                T::Currency::transfer(&pot, &owner, back, Preservation::Expendable)?;
            }
            T::Currency::burn_from(
                &pot,
                burn,
                Preservation::Expendable,
                Precision::BestEffort,
                Fortitude::Force,
            )?;
            let _ = Neurons::<T>::clear_prefix(netuid, u32::MAX, None);
            let _ = NeuronUid::<T>::clear_prefix(netuid, u32::MAX, None);
            let _ = Endpoints::<T>::clear_prefix(netuid, u32::MAX, None);
            let _ = NeuronTier::<T>::clear_prefix(netuid, u32::MAX, None);
            NeuronCount::<T>::remove(netuid);
            SubnetCount::<T>::mutate(|c| *c = c.saturating_sub(1));
            Self::deposit_event(Event::SubnetDeregistered {
                netuid,
                returned: back,
                burned: burn,
            });
            Ok(())
        }

        /// Register a hotkey as a neuron; the fee is burned.
        #[pallet::call_index(2)]
        #[pallet::weight(T::DbWeight::get().reads_writes(4, 4))]
        pub fn register_neuron(
            origin: OriginFor<T>,
            netuid: NetUid,
            hotkey: T::AccountId,
        ) -> DispatchResult {
            let who = ensure_signed(origin)?;
            ensure!(Subnets::<T>::contains_key(netuid), Error::<T>::NoSuchSubnet);
            ensure!(
                !NeuronUid::<T>::contains_key(netuid, &hotkey),
                Error::<T>::AlreadyRegistered
            );
            let uid = NeuronCount::<T>::get(netuid);
            ensure!(uid < T::MaxNeurons::get(), Error::<T>::SubnetFull);
            if Self::in_bootstrap() {
                // Bootstrap: the operator registers its own hotkeys, without a fee.
                ensure!(
                    BootstrapOperator::<T>::get().as_ref() == Some(&who),
                    Error::<T>::BootstrapRestricted
                );
            } else {
                T::Currency::burn_from(
                    &who,
                    T::NeuronFee::get(),
                    Preservation::Preserve,
                    Precision::Exact,
                    Fortitude::Polite,
                )?;
            }
            Neurons::<T>::insert(netuid, uid, hotkey.clone());
            NeuronUid::<T>::insert(netuid, hotkey.clone(), uid);
            NeuronCount::<T>::insert(netuid, uid + 1);
            Self::deposit_event(Event::NeuronRegistered {
                netuid,
                uid,
                hotkey,
            });
            Ok(())
        }

        /// Publish the endpoint the caller's neuron serves on.
        #[pallet::call_index(4)]
        #[pallet::weight(T::DbWeight::get().reads_writes(1, 1))]
        pub fn set_endpoint(
            origin: OriginFor<T>,
            netuid: NetUid,
            endpoint: Vec<u8>,
        ) -> DispatchResult {
            let who = ensure_signed(origin)?;
            ensure!(
                NeuronUid::<T>::contains_key(netuid, &who),
                Error::<T>::NotRegistered
            );
            let e: BoundedVec<u8, ConstU32<128>> = endpoint
                .try_into()
                .map_err(|_| Error::<T>::EndpointTooLong)?;
            Endpoints::<T>::insert(netuid, &who, e);
            Self::deposit_event(Event::EndpointSet {
                netuid,
                hotkey: who,
            });
            Ok(())
        }

        /// End the bootstrap period early (operator or governance). It can never be
        /// extended or restarted.
        #[pallet::call_index(5)]
        #[pallet::weight(T::DbWeight::get().reads_writes(2, 1))]
        pub fn end_bootstrap(origin: OriginFor<T>) -> DispatchResult {
            let is_root = ensure_root(origin.clone()).is_ok();
            if !is_root {
                let who = ensure_signed(origin)?;
                ensure!(
                    BootstrapOperator::<T>::get().as_ref() == Some(&who),
                    Error::<T>::BootstrapRestricted
                );
            }
            ensure!(Self::in_bootstrap(), Error::<T>::NotInBootstrap);
            let now: u32 = polkadot_sdk::frame_system::Pallet::<T>::block_number().saturated_into();
            BootstrapUntil::<T>::put(now);
            Self::deposit_event(Event::BootstrapEnded { at: now });
            Ok(())
        }

        /// Declare the work tier the caller's neuron serves. The tier must be open. The
        /// validators check the neuron against that tier's reference model.
        #[pallet::call_index(6)]
        #[pallet::weight(T::DbWeight::get().reads_writes(2, 1))]
        pub fn declare_tier(origin: OriginFor<T>, netuid: NetUid, tier: Tier) -> DispatchResult {
            let who = ensure_signed(origin)?;
            ensure!(
                NeuronUid::<T>::contains_key(netuid, &who),
                Error::<T>::NotRegistered
            );
            let info = Tiers::<T>::get(tier).ok_or(Error::<T>::UnknownTier)?;
            ensure!(info.open, Error::<T>::TierClosed);
            NeuronTier::<T>::insert(netuid, &who, tier);
            Self::deposit_event(Event::TierDeclared {
                netuid,
                hotkey: who,
                tier,
            });
            Ok(())
        }

        /// Set a tier's weight, tolerance, reference model, model hash and whether it is
        /// open (governance). Neurons that already declared a tier that gets closed keep it.
        #[pallet::call_index(7)]
        #[pallet::weight(T::DbWeight::get().writes(1))]
        pub fn set_tier(
            origin: OriginFor<T>,
            tier: Tier,
            weight_ppm: u32,
            tolerance_ppm: u32,
            model: Vec<u8>,
            model_hash: [u8; 32],
            open: bool,
        ) -> DispatchResult {
            ensure_root(origin)?;
            ensure!(tier <= MAX_TIER, Error::<T>::UnknownTier);
            // G up to 100, tolerance up to 10 nats.
            ensure!(
                weight_ppm > 0 && weight_ppm <= 100_000_000 && tolerance_ppm <= 10_000_000,
                Error::<T>::BadTierParams
            );
            let model: BoundedVec<u8, ConstU32<64>> =
                model.try_into().map_err(|_| Error::<T>::ModelNameTooLong)?;
            Tiers::<T>::insert(
                tier,
                TierInfo {
                    weight_ppm,
                    tolerance_ppm,
                    model,
                    model_hash,
                    open,
                },
            );
            Self::deposit_event(Event::TierUpdated { tier });
            Ok(())
        }

        /// Turn subnet registration on (Gate 1). Phases only move forward.
        #[pallet::call_index(3)]
        #[pallet::weight(T::DbWeight::get().writes(1))]
        pub fn enable_phase2(origin: OriginFor<T>) -> DispatchResult {
            ensure_root(origin)?;
            Phase2Enabled::<T>::put(true);
            Self::deposit_event(Event::Phase2Set { enabled: true });
            Ok(())
        }
    }

    impl<T: Config> Pallet<T> {
        /// True while the disclosed bootstrap period runs.
        pub fn in_bootstrap() -> bool {
            let now: u32 = polkadot_sdk::frame_system::Pallet::<T>::block_number().saturated_into();
            now < BootstrapUntil::<T>::get()
        }

        pub fn account_id() -> T::AccountId {
            T::PalletId::get().into_account_truncating()
        }

        pub fn current_deposit_price() -> u128 {
            let floor = T::DepositFloor::get();
            match LastRegBlock::<T>::get() {
                None => floor,
                Some(last) => {
                    let now = polkadot_sdk::frame_system::Pallet::<T>::block_number();
                    let since: u64 = (now - last).saturated_into();
                    deposit_price(
                        LastRegPrice::<T>::get(),
                        since,
                        floor,
                        T::DecayBlocks::get() as u64,
                    )
                }
            }
        }

        fn next_free_netuid() -> Option<NetUid> {
            (1..=NetUid::MAX).find(|n| !Subnets::<T>::contains_key(n))
        }
    }
}
