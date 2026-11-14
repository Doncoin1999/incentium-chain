//! # Weights pallet (spec Section 6, devnet version)
//!
//! Validators commit `blake2_256(SCALE(netuid, uids, weights, salt))` during epoch E and
//! reveal during epoch E+1. Revealed vectors are stored under (netuid, E) for the
//! consensus pallet. The drand time-lock (automatic reveal) replaces the manual
//! reveal before testnet; the storage layout stays the same.
#![cfg_attr(not(feature = "std"), no_std)]

pub use pallet::*;

#[cfg(test)]
mod mock;
#[cfg(test)]
mod tests;

#[polkadot_sdk::frame_support::pallet]
pub mod pallet {
    use codec::Encode;
    use pallet_subnet_registry::{NetUid, NeuronUid, Uid};
    use polkadot_sdk::frame_support::pallet_prelude::*;
    use polkadot_sdk::frame_system::pallet_prelude::*;
    use polkadot_sdk::sp_core::H256;
    use polkadot_sdk::sp_runtime::traits::SaturatedConversion;
    extern crate alloc;
    use alloc::vec::Vec;

    pub type Epoch = u64;
    /// One validator's revealed row: (validator, [(uid, weight)]).
    pub type Row<AccountId> = (AccountId, Vec<(Uid, u16)>);

    #[pallet::pallet]
    pub struct Pallet<T>(_);

    #[pallet::config]
    pub trait Config: polkadot_sdk::frame_system::Config + pallet_subnet_registry::Config {
        #[allow(deprecated)]
        type RuntimeEvent: From<Event<Self>>
            + IsType<<Self as polkadot_sdk::frame_system::Config>::RuntimeEvent>;
        /// Blocks per epoch (360 = 72 minutes).
        #[pallet::constant]
        type EpochLength: Get<u32>;
        /// Maximum entries in one weight vector.
        #[pallet::constant]
        type MaxWeights: Get<u32>;
    }

    #[pallet::storage]
    pub type Commits<T: Config> = StorageDoubleMap<
        _,
        Twox64Concat,
        NetUid,
        Blake2_128Concat,
        T::AccountId,
        (H256, Epoch),
        OptionQuery,
    >;

    /// Revealed weights: (netuid, epoch of commit) -> validator hotkey -> [(uid, weight)].
    #[pallet::storage]
    pub type Revealed<T: Config> = StorageDoubleMap<
        _,
        Twox64Concat,
        (NetUid, Epoch),
        Blake2_128Concat,
        T::AccountId,
        BoundedVec<(Uid, u16), T::MaxWeights>,
        OptionQuery,
    >;

    #[pallet::event]
    #[pallet::generate_deposit(pub(super) fn deposit_event)]
    pub enum Event<T: Config> {
        WeightsCommitted {
            netuid: NetUid,
            validator: T::AccountId,
            epoch: Epoch,
        },
        WeightsRevealed {
            netuid: NetUid,
            validator: T::AccountId,
            epoch: Epoch,
        },
    }

    #[pallet::error]
    pub enum Error<T> {
        NotRegistered,
        NoCommit,
        WrongRevealEpoch,
        HashMismatch,
        LengthMismatch,
        TooManyWeights,
        UidOutOfRange,
    }

    #[pallet::call]
    impl<T: Config> Pallet<T> {
        /// Commit a hash of this epoch's weights. A later commit in the same epoch replaces it.
        #[pallet::call_index(0)]
        #[pallet::weight(T::DbWeight::get().reads_writes(2, 1))]
        pub fn commit_weights(origin: OriginFor<T>, netuid: NetUid, hash: H256) -> DispatchResult {
            let who = ensure_signed(origin)?;
            ensure!(
                NeuronUid::<T>::contains_key(netuid, &who),
                Error::<T>::NotRegistered
            );
            let epoch = Self::current_epoch();
            Commits::<T>::insert(netuid, &who, (hash, epoch));
            Self::deposit_event(Event::WeightsCommitted {
                netuid,
                validator: who,
                epoch,
            });
            Ok(())
        }

        /// Reveal in the epoch after the commit.
        #[pallet::call_index(1)]
        #[pallet::weight(T::DbWeight::get().reads_writes(2, 2))]
        pub fn reveal_weights(
            origin: OriginFor<T>,
            netuid: NetUid,
            uids: Vec<Uid>,
            weights: Vec<u16>,
            salt: [u8; 32],
        ) -> DispatchResult {
            let who = ensure_signed(origin)?;
            let (hash, epoch) = Commits::<T>::get(netuid, &who).ok_or(Error::<T>::NoCommit)?;
            ensure!(
                Self::current_epoch() == epoch + 1,
                Error::<T>::WrongRevealEpoch
            );
            ensure!(uids.len() == weights.len(), Error::<T>::LengthMismatch);
            ensure!(
                Self::commit_hash(netuid, &uids, &weights, &salt) == hash,
                Error::<T>::HashMismatch
            );
            let n = pallet_subnet_registry::NeuronCount::<T>::get(netuid);
            ensure!(uids.iter().all(|u| *u < n), Error::<T>::UidOutOfRange);
            let pairs: Vec<(Uid, u16)> = uids.into_iter().zip(weights).collect();
            let bounded: BoundedVec<_, T::MaxWeights> =
                pairs.try_into().map_err(|_| Error::<T>::TooManyWeights)?;
            Revealed::<T>::insert((netuid, epoch), &who, bounded);
            Commits::<T>::remove(netuid, &who);
            Self::deposit_event(Event::WeightsRevealed {
                netuid,
                validator: who,
                epoch,
            });
            Ok(())
        }
    }

    impl<T: Config> Pallet<T> {
        pub fn current_epoch() -> Epoch {
            let n: u64 = polkadot_sdk::frame_system::Pallet::<T>::block_number().saturated_into();
            n / T::EpochLength::get() as u64
        }

        pub fn commit_hash(netuid: NetUid, uids: &[Uid], weights: &[u16], salt: &[u8; 32]) -> H256 {
            H256(polkadot_sdk::sp_io::hashing::blake2_256(
                &(netuid, uids, weights, salt).encode(),
            ))
        }

        /// All revealed rows for (netuid, epoch).
        pub fn revealed(netuid: NetUid, epoch: Epoch) -> Vec<Row<T::AccountId>> {
            Revealed::<T>::iter_prefix((netuid, epoch))
                .map(|(k, v)| (k, v.into_inner()))
                .collect()
        }

        /// Drop data for an epoch once consensus has used it.
        pub fn prune(netuid: NetUid, epoch: Epoch) {
            let _ = Revealed::<T>::clear_prefix((netuid, epoch), u32::MAX, None);
        }
    }
}
