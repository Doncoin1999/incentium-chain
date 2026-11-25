//! # Subnet registry pallet (spec Section 4, subnet registration specification of 25 Nov 2026)
//!
//! - SN0 exists from genesis and has no owner.
//! - Subnet registration unlocks by itself, in the first daily check where all four criteria
//!   hold: day 180, at least 300 active SN0 miners as a 30-day average, sudo removed with at
//!   least 3 approved referenda, and the Base swap contract attested by governance as audited
//!   and immutable. Nobody switches it on.
//! - Slots are sold by Dutch auction (see `ict_economics::registry`): SN1 starts at
//!   50,000 ICT, later slots at 4 x the previous sale price, falling 25 % a day to a floor.
//!   The buyer publishes name, description and the hash of the incentive code in the same call.
//! - The payment is not refunded: half to the treasury, half burned.
//! - One slot at a time: the next auction starts `MinSpacing` (60 days) after a sale, and only
//!   while fewer than `MaxSubnets` (8) subnets exist. A new subnet cannot be deregistered for
//!   180 days.
//! - Neurons (miners and validators) register with a burned fee.
//! - Each neuron declares a work tier (spec Section 5). Tier weight G, spot-check tolerance,
//!   reference model and model hash per tier are on chain and set by governance; a tier
//!   can be closed (in the launch period only tier 0, the CPU tier, is open).
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

/// What the runtime tells the registry about governance, for the unlock criteria.
pub trait UnlockSignals {
    /// True once the sudo key is gone for good.
    fn sudo_removed() -> bool;
    /// Number of approved referenda, counted up to `at_least` (counting may stop there).
    fn approved_referenda(at_least: u32) -> u32;
}

impl UnlockSignals for () {
    fn sudo_removed() -> bool {
        false
    }
    fn approved_referenda(_at_least: u32) -> u32 {
        0
    }
}

/// Told when a subnet is closed, so stake on it can be released.
pub trait OnSubnetRemoved {
    fn subnet_removed(netuid: u16);
}

impl OnSubnetRemoved for () {
    fn subnet_removed(_netuid: u16) {}
}

#[polkadot_sdk::frame_support::pallet]
pub mod pallet {
    use crate::{OnSubnetRemoved, UnlockSignals, WeightInfo};
    use ict_economics::registry::{auction_price, floor_price, payment_split, start_price};
    use ict_economics::BLOCKS_PER_DAY;
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

    const DAY: u32 = BLOCKS_PER_DAY as u32;
    /// Unlock criterion 1: day 180 after genesis.
    pub const UNLOCK_MIN_BLOCK: u32 = 180 * DAY;
    /// Unlock criterion 2: at least 300 active SN0 miners as a 30-day average.
    pub const UNLOCK_MIN_MINERS: u32 = 300;
    pub const MINER_WINDOW_DAYS: u32 = 30;
    /// Unlock criterion 3: sudo removed and at least 3 approved referenda.
    pub const UNLOCK_MIN_REFERENDA: u32 = 3;
    /// A new subnet cannot be deregistered for 180 days.
    pub const IMMUNITY_BLOCKS: u32 = 180 * DAY;
    /// Default gap between a sale and the next auction (60 days); governance may lower it
    /// to `MIN_SPACING_FLOOR` (30 days).
    pub const DEFAULT_MIN_SPACING: u32 = 60 * DAY;
    pub const MIN_SPACING_FLOOR: u32 = 30 * DAY;
    /// Subnets (excluding SN0) allowed until governance raises the cap.
    pub const DEFAULT_MAX_SUBNETS: u16 = 8;
    /// Hard upper bound for the cap.
    pub const MAX_SUBNETS_LIMIT: u16 = 256;

    const STORAGE_VERSION: StorageVersion = StorageVersion::new(2);

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
        /// ICT paid for the slot, in base units (not refundable).
        pub deposit: u128,
        pub created: BlockNumber,
    }

    /// What the buyer of a slot publishes on chain. No approval, only openness.
    #[derive(
        Clone, Encode, Decode, DecodeWithMemTracking, PartialEq, Eq, Debug, TypeInfo, MaxEncodedLen,
    )]
    pub struct SubnetMetadata {
        pub name: BoundedVec<u8, ConstU32<64>>,
        pub description: BoundedVec<u8, ConstU32<512>>,
        /// Hash of the incentive-mechanism code (e.g. SHA-256 of a release archive or a git
        /// commit id padded to 32 bytes).
        pub code_hash: [u8; 32],
    }

    /// Governance's attestation that the Base swap contract is audited and immutable.
    #[derive(
        Clone, Encode, Decode, DecodeWithMemTracking, PartialEq, Eq, Debug, TypeInfo, MaxEncodedLen,
    )]
    pub struct SwapAttestation {
        /// Contract address on Base.
        pub contract: [u8; 20],
        /// Hash of the audit report.
        pub audit_hash: [u8; 32],
    }

    /// Where each unlock criterion stands (read by incentium.org's progress tab).
    #[derive(Clone, Encode, Decode, PartialEq, Eq, Debug, TypeInfo)]
    pub struct UnlockStatus {
        pub unlocked: bool,
        pub block: u32,
        pub min_block: u32,
        /// 30-day average of active SN0 miners; `None` until 30 full days are recorded.
        pub miners_avg: Option<u32>,
        pub miner_days: u32,
        pub min_miners: u32,
        pub sudo_removed: bool,
        pub approved_referenda: u32,
        pub min_referenda: u32,
        pub swap_attested: bool,
    }

    #[pallet::pallet]
    #[pallet::storage_version(STORAGE_VERSION)]
    pub struct Pallet<T>(_);

    #[pallet::config]
    pub trait Config: polkadot_sdk::frame_system::Config {
        #[allow(deprecated)]
        type RuntimeEvent: From<Event<Self>>
            + IsType<<Self as polkadot_sdk::frame_system::Config>::RuntimeEvent>;
        /// Weights of this pallet's calls (benchmarked in the runtime).
        type WeightInfo: WeightInfo;
        type Currency: Mutate<Self::AccountId, Balance = u128>;
        /// The registry's own account (kept for neuron bookkeeping and benchmarks).
        #[pallet::constant]
        type PalletId: Get<PalletId>;
        /// On-chain treasury that receives half of every slot payment.
        #[pallet::constant]
        type TreasuryAccount: Get<Self::AccountId>;
        /// Governance facts for the unlock criteria.
        type Unlock: UnlockSignals;
        /// Called when a subnet is closed (consensus releases its stake).
        type OnRemoved: OnSubnetRemoved;
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

    #[pallet::storage]
    pub type Metadata<T> = StorageMap<_, Twox64Concat, NetUid, SubnetMetadata, OptionQuery>;

    /// Subnets excluding SN0.
    #[pallet::storage]
    pub type SubnetCount<T> = StorageValue<_, u16, ValueQuery>;

    #[pallet::type_value]
    pub fn DefaultMaxSubnets() -> u16 {
        DEFAULT_MAX_SUBNETS
    }

    /// Cap on subnets (excluding SN0); only governance can raise it.
    #[pallet::storage]
    pub type MaxSubnets<T> = StorageValue<_, u16, ValueQuery, DefaultMaxSubnets>;

    #[pallet::type_value]
    pub fn DefaultMinSpacing() -> u32 {
        DEFAULT_MIN_SPACING
    }

    /// Blocks from a sale to the start of the next auction.
    #[pallet::storage]
    pub type MinSpacing<T> = StorageValue<_, u32, ValueQuery, DefaultMinSpacing>;

    /// True once subnet registration has unlocked. It never locks again.
    #[pallet::storage]
    pub type Phase2Enabled<T> = StorageValue<_, bool, ValueQuery>;

    /// Block where subnet registration unlocked (start of the 180-day emission ramp).
    #[pallet::storage]
    pub type UnlockedAt<T> = StorageValue<_, u32, OptionQuery>;

    /// Block where the current (or next) auction starts.
    #[pallet::storage]
    pub type AuctionStart<T> = StorageValue<_, u32, OptionQuery>;

    /// Slots sold since genesis (deregistrations do not lower it).
    #[pallet::storage]
    pub type Sales<T> = StorageValue<_, u32, ValueQuery>;

    /// Price the last slot sold for.
    #[pallet::storage]
    pub type LastSalePrice<T> = StorageValue<_, u128, ValueQuery>;

    #[pallet::storage]
    pub type SwapAttested<T> = StorageValue<_, SwapAttestation, OptionQuery>;

    /// Running tally for the current day: (day, sum of active miners over epochs, epochs).
    #[pallet::storage]
    pub type MinerDay<T> = StorageValue<_, (u32, u64, u32), ValueQuery>;

    /// Daily averages of active SN0 miners, oldest first, at most 30.
    #[pallet::storage]
    pub type MinerHistory<T> =
        StorageValue<_, BoundedVec<u32, ConstU32<MINER_WINDOW_DAYS>>, ValueQuery>;

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
        /// Devnets only: start unlocked, with the SN1 auction running from block 0.
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
            if self.phase2_enabled {
                Phase2Enabled::<T>::put(true);
                UnlockedAt::<T>::put(0);
                AuctionStart::<T>::put(0);
            }
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
        /// Once a day, until it has happened: unlock subnet registration if all criteria hold.
        fn on_initialize(n: BlockNumberFor<T>) -> Weight {
            let n: u32 = n.saturated_into();
            if n % DAY != 0 || Phase2Enabled::<T>::get() {
                return T::DbWeight::get().reads(1);
            }
            let _ = Self::try_unlock_now();
            // Status reads plus a bounded referendum count.
            T::DbWeight::get().reads_writes(16, 3)
        }

        /// v0 -> v1: write the default tier table on chains started before tiers existed.
        /// v1 -> v2: auction storage replaces the doubling-deposit storage (no subnet was
        /// ever registered on mainnet, so there is nothing to move).
        fn on_runtime_upgrade() -> Weight {
            let on_chain = StorageVersion::get::<Pallet<T>>();
            if on_chain >= 2 {
                return T::DbWeight::get().reads(1);
            }
            let mut w = T::DbWeight::get().reads(1);
            if on_chain < 1 {
                for (tier, info) in default_tiers() {
                    if !Tiers::<T>::contains_key(tier) {
                        Tiers::<T>::insert(tier, info);
                    }
                }
                w = w.saturating_add(T::DbWeight::get().reads_writes(4, 4));
            }
            let prefix =
                <Pallet<T> as polkadot_sdk::frame_support::traits::PalletInfoAccess>::name()
                    .as_bytes();
            let _ = polkadot_sdk::frame_support::storage::migration::clear_storage_prefix(
                prefix,
                b"LastRegPrice",
                b"",
                None,
                None,
            );
            let _ = polkadot_sdk::frame_support::storage::migration::clear_storage_prefix(
                prefix,
                b"LastRegBlock",
                b"",
                None,
                None,
            );
            STORAGE_VERSION.put::<Pallet<T>>();
            w.saturating_add(T::DbWeight::get().writes(3))
        }
    }

    #[pallet::event]
    #[pallet::generate_deposit(pub(super) fn deposit_event)]
    pub enum Event<T: Config> {
        /// A slot was bought. `to_treasury` went to the treasury, `burned` was burned.
        SubnetRegistered {
            netuid: NetUid,
            owner: T::AccountId,
            price: u128,
            to_treasury: u128,
            burned: u128,
        },
        /// A subnet was closed. Its payment is not refunded.
        SubnetDeregistered {
            netuid: NetUid,
        },
        NeuronRegistered {
            netuid: NetUid,
            uid: Uid,
            hotkey: T::AccountId,
        },
        /// All unlock criteria held; the SN1 auction starts at `auction_start`.
        RegistrationUnlocked {
            at: u32,
            auction_start: u32,
        },
        /// The next auction starts at block `at`.
        AuctionScheduled {
            at: u32,
            start_price: u128,
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
        /// Governance attested the audited, immutable Base swap contract.
        SwapAttested {
            contract: [u8; 20],
            audit_hash: [u8; 32],
        },
        MinSpacingSet {
            blocks: u32,
        },
        MaxSubnetsSet {
            max: u16,
        },
        /// A day's average of active SN0 miners was recorded.
        MinerDayRecorded {
            day: u32,
            average: u32,
        },
    }

    #[pallet::error]
    pub enum Error<T> {
        /// Subnet registration has not unlocked yet.
        Phase2NotEnabled,
        TooManySubnets,
        /// The current price is above the buyer's limit.
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
        /// No auction is running (waiting for the spacing period or a free slot).
        NoAuction,
        NameTooLong,
        DescriptionTooLong,
        EmptyName,
        /// The subnet is within its 180-day immunity.
        StillImmune,
        /// Not all unlock criteria hold yet.
        CriteriaNotMet,
        AlreadyUnlocked,
        AlreadyAttested,
        SpacingTooShort,
        /// The cap must be at least the current number of subnets and at most 256.
        BadMaxSubnets,
    }

    #[pallet::call]
    impl<T: Config> Pallet<T> {
        /// Buy the slot on auction at the current price, publishing the subnet's name,
        /// description and incentive-code hash. Half the price goes to the treasury and half
        /// is burned; none of it is refunded.
        #[pallet::call_index(0)]
        #[pallet::weight(<T as Config>::WeightInfo::register_subnet())]
        pub fn register_subnet(
            origin: OriginFor<T>,
            max_price: u128,
            name: Vec<u8>,
            description: Vec<u8>,
            code_hash: [u8; 32],
        ) -> DispatchResult {
            let who = ensure_signed(origin)?;
            ensure!(Phase2Enabled::<T>::get(), Error::<T>::Phase2NotEnabled);
            ensure!(
                SubnetCount::<T>::get() < MaxSubnets::<T>::get(),
                Error::<T>::TooManySubnets
            );
            let price = Self::current_price().ok_or(Error::<T>::NoAuction)?;
            ensure!(price <= max_price, Error::<T>::DepositAboveLimit);
            ensure!(!name.is_empty(), Error::<T>::EmptyName);
            let meta = SubnetMetadata {
                name: name.try_into().map_err(|_| Error::<T>::NameTooLong)?,
                description: description
                    .try_into()
                    .map_err(|_| Error::<T>::DescriptionTooLong)?,
                code_hash,
            };
            let netuid = Self::next_free_netuid().ok_or(Error::<T>::TooManySubnets)?;

            let (to_treasury, burned) = payment_split(price);
            T::Currency::transfer(
                &who,
                &T::TreasuryAccount::get(),
                to_treasury,
                Preservation::Preserve,
            )?;
            T::Currency::burn_from(
                &who,
                burned,
                Preservation::Preserve,
                Precision::Exact,
                Fortitude::Polite,
            )?;

            let now = polkadot_sdk::frame_system::Pallet::<T>::block_number();
            Subnets::<T>::insert(
                netuid,
                SubnetInfo {
                    owner: Some(who.clone()),
                    deposit: price,
                    created: now,
                },
            );
            Metadata::<T>::insert(netuid, meta);
            SubnetCount::<T>::mutate(|c| *c += 1);
            Sales::<T>::mutate(|s| *s += 1);
            LastSalePrice::<T>::put(price);
            let now: u32 = now.saturated_into();
            let next = now.saturating_add(MinSpacing::<T>::get());
            AuctionStart::<T>::put(next);
            Self::deposit_event(Event::SubnetRegistered {
                netuid,
                owner: who,
                price,
                to_treasury,
                burned,
            });
            Self::deposit_event(Event::AuctionScheduled {
                at: next,
                start_price: Self::next_start_price(),
            });
            Ok(())
        }

        /// Close a subnet (governance), after its 180-day immunity. Nothing is refunded.
        #[pallet::call_index(1)]
        #[pallet::weight(<T as Config>::WeightInfo::deregister_subnet(T::MaxNeurons::get() as u32))]
        pub fn deregister_subnet(origin: OriginFor<T>, netuid: NetUid) -> DispatchResult {
            ensure_root(origin)?;
            ensure!(netuid != SN0, Error::<T>::CannotDeregisterSn0);
            let info = Subnets::<T>::get(netuid).ok_or(Error::<T>::NoSuchSubnet)?;
            let now: u32 = polkadot_sdk::frame_system::Pallet::<T>::block_number().saturated_into();
            let created: u32 = info.created.saturated_into();
            ensure!(
                now >= created.saturating_add(IMMUNITY_BLOCKS),
                Error::<T>::StillImmune
            );
            let was_full = SubnetCount::<T>::get() >= MaxSubnets::<T>::get();
            Subnets::<T>::remove(netuid);
            Metadata::<T>::remove(netuid);
            let _ = Neurons::<T>::clear_prefix(netuid, u32::MAX, None);
            let _ = NeuronUid::<T>::clear_prefix(netuid, u32::MAX, None);
            let _ = Endpoints::<T>::clear_prefix(netuid, u32::MAX, None);
            let _ = NeuronTier::<T>::clear_prefix(netuid, u32::MAX, None);
            NeuronCount::<T>::remove(netuid);
            SubnetCount::<T>::mutate(|c| *c = c.saturating_sub(1));
            T::OnRemoved::subnet_removed(netuid);
            if was_full {
                Self::slot_freed(now);
            }
            Self::deposit_event(Event::SubnetDeregistered { netuid });
            Ok(())
        }

        /// Register a hotkey as a neuron; the fee is burned.
        #[pallet::call_index(2)]
        #[pallet::weight(<T as Config>::WeightInfo::register_neuron())]
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

        /// Anyone may ask the chain to check the unlock criteria now instead of waiting for
        /// the daily check. Fails if a criterion does not hold.
        #[pallet::call_index(3)]
        #[pallet::weight(T::DbWeight::get().reads_writes(1_040, 3))]
        pub fn try_unlock(origin: OriginFor<T>) -> DispatchResult {
            ensure_signed(origin)?;
            ensure!(!Phase2Enabled::<T>::get(), Error::<T>::AlreadyUnlocked);
            ensure!(Self::try_unlock_now(), Error::<T>::CriteriaNotMet);
            Ok(())
        }

        /// Publish the endpoint the caller's neuron serves on.
        #[pallet::call_index(4)]
        #[pallet::weight(<T as Config>::WeightInfo::set_endpoint())]
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
        #[pallet::weight(<T as Config>::WeightInfo::end_bootstrap())]
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
        #[pallet::weight(<T as Config>::WeightInfo::declare_tier())]
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
        #[pallet::weight(<T as Config>::WeightInfo::set_tier())]
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

        /// Attest, once, that the Base swap contract is audited and immutable (governance).
        #[pallet::call_index(8)]
        #[pallet::weight(T::DbWeight::get().reads_writes(1, 1))]
        pub fn attest_swap(
            origin: OriginFor<T>,
            contract: [u8; 20],
            audit_hash: [u8; 32],
        ) -> DispatchResult {
            ensure_root(origin)?;
            ensure!(!SwapAttested::<T>::exists(), Error::<T>::AlreadyAttested);
            SwapAttested::<T>::put(SwapAttestation {
                contract,
                audit_hash,
            });
            Self::deposit_event(Event::SwapAttested {
                contract,
                audit_hash,
            });
            Ok(())
        }

        /// Change the gap between a sale and the next auction (governance), at least 30 days.
        #[pallet::call_index(9)]
        #[pallet::weight(T::DbWeight::get().writes(1))]
        pub fn set_min_spacing(origin: OriginFor<T>, blocks: u32) -> DispatchResult {
            ensure_root(origin)?;
            ensure!(blocks >= MIN_SPACING_FLOOR, Error::<T>::SpacingTooShort);
            MinSpacing::<T>::put(blocks);
            Self::deposit_event(Event::MinSpacingSet { blocks });
            Ok(())
        }

        /// Change the cap on subnets (governance).
        #[pallet::call_index(10)]
        #[pallet::weight(T::DbWeight::get().reads_writes(3, 2))]
        pub fn set_max_subnets(origin: OriginFor<T>, max: u16) -> DispatchResult {
            ensure_root(origin)?;
            let count = SubnetCount::<T>::get();
            ensure!(
                max >= count && max <= MAX_SUBNETS_LIMIT,
                Error::<T>::BadMaxSubnets
            );
            let was_full = count >= MaxSubnets::<T>::get();
            MaxSubnets::<T>::put(max);
            if was_full && count < max {
                let now: u32 =
                    polkadot_sdk::frame_system::Pallet::<T>::block_number().saturated_into();
                Self::slot_freed(now);
            }
            Self::deposit_event(Event::MaxSubnetsSet { max });
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

        /// Start price of the next slot to be sold.
        pub fn next_start_price() -> u128 {
            start_price(Sales::<T>::get() + 1, LastSalePrice::<T>::get())
        }

        /// Price of the slot on auction now; `None` when no auction runs.
        pub fn current_price() -> Option<u128> {
            if !Phase2Enabled::<T>::get() || SubnetCount::<T>::get() >= MaxSubnets::<T>::get() {
                return None;
            }
            let start = AuctionStart::<T>::get()?;
            let now: u32 = polkadot_sdk::frame_system::Pallet::<T>::block_number().saturated_into();
            if now < start {
                return None;
            }
            let sale = Sales::<T>::get() + 1;
            Some(auction_price(
                Self::next_start_price(),
                (now - start) as u64,
                floor_price(sale),
            ))
        }

        /// A slot became free while the cap was reached: an auction waiting for it starts now.
        fn slot_freed(now: u32) {
            if Phase2Enabled::<T>::get() && AuctionStart::<T>::get().map_or(false, |s| s < now) {
                AuctionStart::<T>::put(now);
                Self::deposit_event(Event::AuctionScheduled {
                    at: now,
                    start_price: Self::next_start_price(),
                });
            }
        }

        /// 30-day average of active SN0 miners, once 30 days are recorded.
        pub fn miners_average() -> Option<u32> {
            let h = MinerHistory::<T>::get();
            if (h.len() as u32) < MINER_WINDOW_DAYS {
                return None;
            }
            let sum: u64 = h.iter().map(|x| *x as u64).sum();
            Some((sum / h.len() as u64) as u32)
        }

        /// Called by consensus after each SN0 epoch with the number of miners paid in it
        /// (neither penalised by a failed spot check nor banned). Rolls the daily average.
        pub fn note_sn0_epoch(active_miners: u32) {
            let now: u32 = polkadot_sdk::frame_system::Pallet::<T>::block_number().saturated_into();
            let today = now / DAY;
            let (day, sum, epochs) = MinerDay::<T>::get();
            if day != today && epochs > 0 {
                let avg = (sum / epochs as u64) as u32;
                MinerHistory::<T>::mutate(|h| {
                    if h.is_full() {
                        h.remove(0);
                    }
                    let _ = h.try_push(avg);
                });
                Self::deposit_event(Event::MinerDayRecorded { day, average: avg });
            }
            if day != today {
                MinerDay::<T>::put((today, active_miners as u64, 1));
            } else {
                MinerDay::<T>::put((today, sum + active_miners as u64, epochs + 1));
            }
        }

        /// Where the unlock criteria stand.
        pub fn unlock_status() -> UnlockStatus {
            let block: u32 =
                polkadot_sdk::frame_system::Pallet::<T>::block_number().saturated_into();
            UnlockStatus {
                unlocked: Phase2Enabled::<T>::get(),
                block,
                min_block: UNLOCK_MIN_BLOCK,
                miners_avg: Self::miners_average(),
                miner_days: MinerHistory::<T>::decode_len().unwrap_or(0) as u32,
                min_miners: UNLOCK_MIN_MINERS,
                sudo_removed: T::Unlock::sudo_removed(),
                approved_referenda: T::Unlock::approved_referenda(UNLOCK_MIN_REFERENDA),
                min_referenda: UNLOCK_MIN_REFERENDA,
                swap_attested: SwapAttested::<T>::exists(),
            }
        }

        /// True if every criterion holds.
        pub fn criteria_met(s: &UnlockStatus) -> bool {
            s.block >= s.min_block
                && s.miners_avg.map_or(false, |m| m >= s.min_miners)
                && s.sudo_removed
                && s.approved_referenda >= s.min_referenda
                && s.swap_attested
        }

        /// Unlock if all criteria hold. The SN1 auction starts in the same block.
        fn try_unlock_now() -> bool {
            if Phase2Enabled::<T>::get() {
                return false;
            }
            let s = Self::unlock_status();
            if !Self::criteria_met(&s) {
                return false;
            }
            Phase2Enabled::<T>::put(true);
            UnlockedAt::<T>::put(s.block);
            AuctionStart::<T>::put(s.block);
            Self::deposit_event(Event::RegistrationUnlocked {
                at: s.block,
                auction_start: s.block,
            });
            Self::deposit_event(Event::AuctionScheduled {
                at: s.block,
                start_price: Self::next_start_price(),
            });
            true
        }

        fn next_free_netuid() -> Option<NetUid> {
            (1..=NetUid::MAX).find(|n| !Subnets::<T>::contains_key(n))
        }
    }
}
