//! Portable ESP-NOW encrypted-peer and anti-replay ownership.
//!
//! This module stops before the first interoperability contract not present
//! in the reviewed repository evidence: the ESP-NOW Action-frame CCMP AAD and
//! the chip key-selector/construct/decrypt callback ABI. It therefore owns
//! secrets and peer generations, but never claims to produce or authenticate
//! encrypted on-air bytes. Each peer's transmit packet numbers and receive
//! replay frontier are the CCMP owners of `oer-protocols-ieee80211-core-upper-mac`.

use crate::extensions::espressif::phy::EspNowPhyMode;
use core::{array, fmt};
use oer_protocols_ieee80211_wire::sequence::SequenceNumber;

use oer_portable_events::{EventSink, report};
use zeroize::{Zeroize, ZeroizeOnDrop};
use {
    oer_protocols_ieee80211_core_upper_mac::{
        ccmp::{
            CcmpPacketNumberStep, CcmpReplayCandidate, CcmpReplayFrontier, CcmpReplayed,
            InstalledCcmpTxKey, SoftwareCcmpKey,
        },
        sequence::SequenceCounter,
    },
    oer_protocols_ieee80211_wire::{
        ccmp::CcmpPacketNumber,
        channel::Ghz2_4Channel,
        extensions::espressif::esp_now::{
            ESP_NOW_V1_MIN_PROTECTED_MPDU_LEN, EspNowCcmpPacketNumber, EspNowDestination,
            EspNowEncryptedV1Unavailable, EspNowProtectedV1Envelope, EspNowProtectedV1WireError,
            EspNowRandomValue, EspNowUnicastAddress, EspNowV1Payload, EspNowV1WireError,
        },
    },
};

use crate::extensions::espressif::esp_now::protocol::{
    ESP_NOW_DEFAULT_PEER_CAPACITY, EspNowConfig, EspNowPeerSecurity,
};
use oer_protocols_ieee80211_contracts_port::ChannelContextId;
use oer_protocols_ieee80211_core_arbiter::interface::BoundVirtualInterface;

pub const ESP_NOW_KEY_LEN: usize = 16;
/// Portable encrypted-peer storage default. This is not a hardware-key-slot
/// claim; each backend advertises only its proven encrypted-slot capacity.
pub const ESP_NOW_DEFAULT_ENCRYPTED_PEER_CAPACITY: usize = ESP_NOW_DEFAULT_PEER_CAPACITY;

/// ESP-NOW primary master key. Formatting never exposes key material and the
/// owned bytes are zeroized on drop.
#[derive(Zeroize, ZeroizeOnDrop)]
pub struct EspNowPmk([u8; ESP_NOW_KEY_LEN]);

impl EspNowPmk {
    pub const fn new(bytes: [u8; ESP_NOW_KEY_LEN]) -> Self {
        Self(bytes)
    }

    /// Borrow key material for a bounded, reviewed key transaction.
    pub const fn expose_secret(&self) -> &[u8; ESP_NOW_KEY_LEN] {
        &self.0
    }
}

impl fmt::Debug for EspNowPmk {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("EspNowPmk([REDACTED; 16])")
    }
}

/// ESP-NOW local master key for one individual peer. Formatting never
/// exposes key material and the owned bytes are zeroized on drop.
#[derive(Zeroize, ZeroizeOnDrop)]
pub struct EspNowLmk([u8; ESP_NOW_KEY_LEN]);

impl EspNowLmk {
    pub const fn new(bytes: [u8; ESP_NOW_KEY_LEN]) -> Self {
        Self(bytes)
    }

    /// Borrow key material for a bounded, reviewed key transaction.
    pub const fn expose_secret(&self) -> &[u8; ESP_NOW_KEY_LEN] {
        &self.0
    }
}

impl fmt::Debug for EspNowLmk {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("EspNowLmk([REDACTED; 16])")
    }
}

/// Generation-checked identity of the PMK currently owned by one service.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EspNowPmkId {
    generation: u32,
}

impl EspNowPmkId {
    pub const fn generation(self) -> u32 {
        self.generation
    }
}

/// The place of one PMK: empty, or a key. Its generation counts the
/// installs and replacements, so a handle to an earlier key is stale.
pub struct EspNowPmkOwner {
    place: PmkPlace,
}

enum PmkPlace {
    Empty { generation: u32 },
    Installed { generation: u32, key: EspNowPmk },
}

impl EspNowPmkOwner {
    pub const fn new() -> Self {
        Self {
            place: PmkPlace::Empty { generation: 0 },
        }
    }

    pub fn from_key(key: EspNowPmk) -> Self {
        Self {
            place: PmkPlace::Installed { generation: 1, key },
        }
    }

    /// The installed key's handle; `None` while the place is empty.
    pub const fn current_id(&self) -> Option<EspNowPmkId> {
        match self.place {
            PmkPlace::Installed { generation, .. } => Some(EspNowPmkId { generation }),
            PmkPlace::Empty { .. } => None,
        }
    }

    /// The empty place, which alone installs a key; or the handle of the
    /// key installed, which a caller replaces or removes first.
    pub fn vacancy(&mut self) -> Result<EspNowPmkVacancy<'_>, EspNowPmkId> {
        match self.place {
            PmkPlace::Empty { generation } => Ok(EspNowPmkVacancy {
                place: &mut self.place,
                generation,
            }),
            PmkPlace::Installed { generation, .. } => Err(EspNowPmkId { generation }),
        }
    }

    /// Atomically replace a PMK. Validation happens before either key moves;
    /// failure returns the proposed key and leaves the installed key intact.
    pub fn replace(
        &mut self,
        current: EspNowPmkId,
        replacement: EspNowPmk,
    ) -> Result<(EspNowPmkId, EspNowPmk), EspNowPmkMutationFailure> {
        let PmkPlace::Installed { generation, key } = &mut self.place else {
            return Err(EspNowPmkMutationFailure::new(
                EspNowPmkError::Stale(current),
                replacement,
            ));
        };
        if *generation != current.generation {
            return Err(EspNowPmkMutationFailure::new(
                EspNowPmkError::Stale(current),
                replacement,
            ));
        }
        let Some(next) = generation.checked_add(1) else {
            return Err(EspNowPmkMutationFailure::new(
                EspNowPmkError::GenerationExhausted,
                replacement,
            ));
        };
        let old = core::mem::replace(key, replacement);
        *generation = next;
        Ok((EspNowPmkId { generation: next }, old))
    }

    /// Remove the key `current` names; the place is empty after it. A
    /// refusal moves nothing.
    pub fn remove(&mut self, current: EspNowPmkId) -> Result<EspNowPmk, EspNowPmkError> {
        if !matches!(
            self.place,
            PmkPlace::Installed { generation, .. } if generation == current.generation
        ) {
            return Err(EspNowPmkError::Stale(current));
        }
        let empty = PmkPlace::Empty {
            generation: current.generation,
        };
        match core::mem::replace(&mut self.place, empty) {
            PmkPlace::Installed { key, .. } => Ok(key),
            // The check above found the key installed; the language cannot
            // carry that into this match, so this arm answers what it would
            // see: no key of `current`.
            vacant @ PmkPlace::Empty { .. } => {
                self.place = vacant;
                Err(EspNowPmkError::Stale(current))
            }
        }
    }

    pub fn key(&self, current: EspNowPmkId) -> Result<&EspNowPmk, EspNowPmkError> {
        match &self.place {
            PmkPlace::Installed { generation, key } if *generation == current.generation => Ok(key),
            PmkPlace::Installed { .. } | PmkPlace::Empty { .. } => {
                Err(EspNowPmkError::Stale(current))
            }
        }
    }
}

impl Default for EspNowPmkOwner {
    fn default() -> Self {
        Self::new()
    }
}

/// The empty PMK place, lent by [`EspNowPmkOwner::vacancy`]: the one that
/// installs a key.
pub struct EspNowPmkVacancy<'owner> {
    place: &'owner mut PmkPlace,
    generation: u32,
}

impl EspNowPmkVacancy<'_> {
    /// Install `key`; a refusal returns it and leaves the place empty.
    pub fn install(self, key: EspNowPmk) -> Result<EspNowPmkId, EspNowPmkMutationFailure> {
        let Some(generation) = self.generation.checked_add(1) else {
            return Err(EspNowPmkMutationFailure::new(
                EspNowPmkError::GenerationExhausted,
                key,
            ));
        };
        *self.place = PmkPlace::Installed { generation, key };
        Ok(EspNowPmkId { generation })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EspNowPmkError {
    GenerationExhausted,
    Stale(EspNowPmkId),
}

impl fmt::Display for EspNowPmkError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::GenerationExhausted => {
                formatter.write_str("the ESP-NOW PMK generation is exhausted")
            }
            Self::Stale(id) => write!(formatter, "ESP-NOW PMK handle {id:?} is stale"),
        }
    }
}

impl core::error::Error for EspNowPmkError {}

/// Failed PMK install/replace retaining the proposed secret for rollback.
pub struct EspNowPmkMutationFailure {
    error: EspNowPmkError,
    key: EspNowPmk,
}

impl EspNowPmkMutationFailure {
    fn new(error: EspNowPmkError, key: EspNowPmk) -> Self {
        Self { error, key }
    }

    pub const fn error(&self) -> EspNowPmkError {
        self.error
    }

    pub fn into_parts(self) -> (EspNowPmkError, EspNowPmk) {
        (self.error, self.key)
    }
}

impl fmt::Debug for EspNowPmkMutationFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("EspNowPmkMutationFailure")
            .field("error", &self.error)
            .field("key", &"[REDACTED]")
            .finish()
    }
}

/// Owned configuration of one encrypted individual peer.
///
/// Broadcast cannot be represented because the destination type is
/// [`EspNowUnicastAddress`]. The LMK is returned to the caller on every failed
/// mutation and zeroized when the configuration is dropped.
pub struct EspNowEncryptedPeerConfig {
    destination: EspNowUnicastAddress,
    channel: Ghz2_4Channel,
    phy_mode: EspNowPhyMode,
    lmk: EspNowLmk,
}

impl EspNowEncryptedPeerConfig {
    pub const fn new(
        destination: EspNowUnicastAddress,
        channel: Ghz2_4Channel,
        lmk: EspNowLmk,
    ) -> Self {
        Self {
            destination,
            channel,
            phy_mode: EspNowPhyMode::LegacyDsss1M,
            lmk,
        }
    }

    pub const fn with_phy_mode(mut self, phy_mode: EspNowPhyMode) -> Self {
        self.phy_mode = phy_mode;
        self
    }

    pub const fn destination(&self) -> EspNowUnicastAddress {
        self.destination
    }

    pub const fn channel(&self) -> Ghz2_4Channel {
        self.channel
    }

    pub const fn phy_mode(&self) -> EspNowPhyMode {
        self.phy_mode
    }

    pub const fn security(&self) -> EspNowPeerSecurity {
        EspNowPeerSecurity::Encrypted
    }

    pub const fn lmk(&self) -> &EspNowLmk {
        &self.lmk
    }

    pub fn into_parts(
        self,
    ) -> (
        EspNowUnicastAddress,
        Ghz2_4Channel,
        EspNowPhyMode,
        EspNowLmk,
    ) {
        (self.destination, self.channel, self.phy_mode, self.lmk)
    }
}

/// The table owns a peer's LMK by value: installing a configuration pairs it
/// with a new CCMP packet-number allocator.
impl SoftwareCcmpKey for EspNowEncryptedPeerConfig {}

impl fmt::Debug for EspNowEncryptedPeerConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("EspNowEncryptedPeerConfig")
            .field("destination", &self.destination)
            .field("channel", &self.channel)
            .field("phy_mode", &self.phy_mode)
            .field("lmk", &"[REDACTED]")
            .finish()
    }
}

/// Generation-checked identity of one encrypted-peer slot.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EspNowEncryptedPeerId {
    index: usize,
    generation: u32,
}

impl EspNowEncryptedPeerId {
    pub const fn index(self) -> usize {
        self.index
    }

    pub const fn generation(self) -> u32 {
        self.generation
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EspNowEncryptedPeerView {
    pub destination: EspNowUnicastAddress,
    pub channel: Ghz2_4Channel,
    pub phy_mode: EspNowPhyMode,
    pub last_tx_packet_number: u64,
    pub highest_authenticated_rx_packet_number: Option<u64>,
}

/// One peer's key, its transmit packet numbers and its receive replay
/// frontier, all from `oer-protocols-ieee80211-core-upper-mac::ccmp`.
struct EspNowEncryptedPeerState {
    key: InstalledCcmpTxKey<EspNowEncryptedPeerConfig>,
    replay: CcmpReplayFrontier,
}

impl EspNowEncryptedPeerState {
    const fn config(&self) -> &EspNowEncryptedPeerConfig {
        self.key.key()
    }
}

/// The highest authenticated receive packet number of a frontier, `None`
/// before the first: a frontier starts at zero, which no frame carries.
fn highest_authenticated(replay: &CcmpReplayFrontier) -> Option<u64> {
    let highest = replay.highest();
    (highest != CcmpPacketNumber::ZERO).then_some(highest.value())
}

/// One encrypted-peer slot: vacant, or a peer's key and packet-number
/// spaces. Its generation counts the installs, so a handle to an earlier
/// occupant is stale.
enum EspNowEncryptedPeerSlot {
    Vacant {
        generation: u32,
    },
    Occupied {
        generation: u32,
        state: EspNowEncryptedPeerState,
    },
}

impl EspNowEncryptedPeerSlot {
    const fn empty() -> Self {
        Self::Vacant { generation: 0 }
    }

    const fn generation(&self) -> u32 {
        match self {
            Self::Vacant { generation } | Self::Occupied { generation, .. } => *generation,
        }
    }

    const fn occupant(&self) -> Option<&EspNowEncryptedPeerState> {
        match self {
            Self::Occupied { state, .. } => Some(state),
            Self::Vacant { .. } => None,
        }
    }

    /// The state a handle of `generation` names here.
    fn state_mut(&mut self, generation: u32) -> Option<&mut EspNowEncryptedPeerState> {
        match self {
            Self::Occupied {
                generation: occupant,
                state,
            } if *occupant == generation => Some(state),
            Self::Occupied { .. } | Self::Vacant { .. } => None,
        }
    }

    /// Vacate the slot, keeping its generation: every LMK it held is
    /// dropped and zeroized; `None` when it was vacant.
    fn vacate(&mut self) -> Option<EspNowEncryptedPeerState> {
        let generation = self.generation();
        match core::mem::replace(self, Self::Vacant { generation }) {
            Self::Occupied { state, .. } => Some(state),
            Self::Vacant { .. } => None,
        }
    }
}

/// Fixed-capacity owner of encrypted individual peers and their PN spaces.
///
/// Every change, packet-number allocation and replay step is its call's
/// result; the table keeps no count of them.
pub struct EspNowEncryptedPeerTable<const N: usize = ESP_NOW_DEFAULT_ENCRYPTED_PEER_CAPACITY> {
    home_channel: Ghz2_4Channel,
    slots: [EspNowEncryptedPeerSlot; N],
}

/// Shutdown cleared the keys of `count` peers, saturated at `u32::MAX`:
/// [`EspNowEncryptedProtocol::shutdown`] reports it, because it returns only
/// the configuration.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EspNowPeersCleared {
    pub count: u32,
}

impl<const N: usize> EspNowEncryptedPeerTable<N> {
    pub fn new(home_channel: Ghz2_4Channel) -> Self {
        Self {
            home_channel,
            slots: array::from_fn(|_| EspNowEncryptedPeerSlot::empty()),
        }
    }

    pub const fn home_channel(&self) -> Ghz2_4Channel {
        self.home_channel
    }

    /// The number of peers: the occupied slots.
    pub fn len(&self) -> usize {
        self.slots
            .iter()
            .filter_map(EspNowEncryptedPeerSlot::occupant)
            .count()
    }

    pub fn is_empty(&self) -> bool {
        self.slots.iter().all(|slot| slot.occupant().is_none())
    }

    pub const fn capacity(&self) -> usize {
        N
    }

    pub fn install(
        &mut self,
        config: EspNowEncryptedPeerConfig,
    ) -> Result<EspNowEncryptedPeerId, EspNowEncryptedPeerMutationFailure> {
        if config.channel != self.home_channel {
            return Err(self.mutation_failure(
                EspNowEncryptedPeerError::ChannelMismatch {
                    peer: config.channel,
                    home: self.home_channel,
                },
                config,
            ));
        }

        if self.find(config.destination).is_some() {
            return Err(self.mutation_failure(
                EspNowEncryptedPeerError::Duplicate(config.destination),
                config,
            ));
        }
        let Some((index, slot, generation)) =
            self.slots
                .iter_mut()
                .enumerate()
                .find_map(|(index, slot)| match slot {
                    EspNowEncryptedPeerSlot::Vacant { generation } => generation
                        .checked_add(1)
                        .map(|generation| (index, slot, generation)),
                    EspNowEncryptedPeerSlot::Occupied { .. } => None,
                })
        else {
            let error = if self.slots.iter().any(|slot| slot.occupant().is_none()) {
                EspNowEncryptedPeerError::GenerationExhausted
            } else {
                EspNowEncryptedPeerError::Full
            };
            return Err(self.mutation_failure(error, config));
        };
        *slot = EspNowEncryptedPeerSlot::Occupied {
            generation,
            state: EspNowEncryptedPeerState {
                key: InstalledCcmpTxKey::install(config, CcmpPacketNumberStep::ONE),
                replay: CcmpReplayFrontier::default(),
            },
        };
        Ok(EspNowEncryptedPeerId { index, generation })
    }

    /// Atomically rotate or replace a peer. Failure retains the proposed LMK
    /// and leaves the existing key, PN and replay window unchanged.
    pub fn replace(
        &mut self,
        peer: EspNowEncryptedPeerId,
        replacement: EspNowEncryptedPeerConfig,
    ) -> Result<EspNowEncryptedPeerReplacement, EspNowEncryptedPeerMutationFailure> {
        let home_channel = self.home_channel;
        let Some(EspNowEncryptedPeerSlot::Occupied {
            generation: occupant,
            state: installed,
        }) = self.slots.get_mut(peer.index)
        else {
            return Err(self.mutation_failure(EspNowEncryptedPeerError::Stale(peer), replacement));
        };
        if *occupant != peer.generation {
            return Err(self.mutation_failure(EspNowEncryptedPeerError::Stale(peer), replacement));
        }
        if replacement.channel != home_channel {
            return Err(self.mutation_failure(
                EspNowEncryptedPeerError::ChannelMismatch {
                    peer: replacement.channel,
                    home: home_channel,
                },
                replacement,
            ));
        }
        let installed_destination = installed.config().destination;
        if replacement.destination != installed_destination {
            return Err(self.mutation_failure(
                EspNowEncryptedPeerError::ReplacementDestinationMismatch {
                    installed: installed_destination,
                    proposed: replacement.destination,
                },
                replacement,
            ));
        }
        let Some(generation) = peer.generation.checked_add(1) else {
            return Err(
                self.mutation_failure(EspNowEncryptedPeerError::GenerationExhausted, replacement)
            );
        };

        // Preserve both packet-number spaces across replacement. This is
        // conservative for a fresh LMK and prevents nonce reuse if a caller
        // supplies the same LMK bytes again.
        let replaced = installed.key.replace_key(replacement);
        *occupant = generation;
        Ok(EspNowEncryptedPeerReplacement {
            peer: EspNowEncryptedPeerId {
                index: peer.index,
                generation,
            },
            replaced,
        })
    }

    pub fn remove(
        &mut self,
        peer: EspNowEncryptedPeerId,
    ) -> Result<EspNowRemovedEncryptedPeer, EspNowEncryptedPeerError> {
        self.state(peer)?;
        let Some(state) = self
            .slots
            .get_mut(peer.index)
            .and_then(EspNowEncryptedPeerSlot::vacate)
        else {
            return Err(EspNowEncryptedPeerError::Stale(peer));
        };
        Ok(EspNowRemovedEncryptedPeer {
            index: peer.index,
            generation: peer.generation,
            state,
        })
    }

    /// Roll back a failed external key-clear transaction without resetting PN
    /// or replay state. The original slot must still be empty and unchanged;
    /// restoration advances its generation so the pre-remove handle remains
    /// stale.
    pub fn restore_removed(
        &mut self,
        removed: EspNowRemovedEncryptedPeer,
    ) -> Result<EspNowEncryptedPeerId, EspNowEncryptedPeerRestoreFailure> {
        // A slot occupied since the removal has a later generation: install
        // and restore both advance it, so the generation alone refuses it.
        let duplicate = self.find(removed.config().destination).is_some();
        let home = self.home_channel;
        let Some(slot) = self
            .slots
            .get_mut(removed.index)
            .filter(|slot| slot.generation() == removed.generation)
        else {
            return Err(EspNowEncryptedPeerRestoreFailure {
                error: EspNowEncryptedPeerError::Stale(removed.previous_id()),
                removed,
            });
        };
        let Some(generation) = removed.generation.checked_add(1) else {
            return Err(EspNowEncryptedPeerRestoreFailure {
                error: EspNowEncryptedPeerError::GenerationExhausted,
                removed,
            });
        };
        if removed.config().channel != home {
            return Err(EspNowEncryptedPeerRestoreFailure {
                error: EspNowEncryptedPeerError::ChannelMismatch {
                    peer: removed.config().channel,
                    home,
                },
                removed,
            });
        }
        if duplicate {
            return Err(EspNowEncryptedPeerRestoreFailure {
                error: EspNowEncryptedPeerError::Duplicate(removed.config().destination),
                removed,
            });
        }
        let EspNowRemovedEncryptedPeer {
            index,
            generation: _,
            state,
        } = removed;
        *slot = EspNowEncryptedPeerSlot::Occupied { generation, state };
        Ok(EspNowEncryptedPeerId { index, generation })
    }

    /// Drop and zeroize every LMK. Existing handles become stale immediately;
    /// a later install advances each reused slot generation.
    pub fn clear_on_shutdown(&mut self) -> usize {
        self.slots
            .iter_mut()
            .filter_map(EspNowEncryptedPeerSlot::vacate)
            .count()
    }

    pub fn find(&self, destination: EspNowUnicastAddress) -> Option<EspNowEncryptedPeerId> {
        self.slots
            .iter()
            .enumerate()
            .find(|(_, slot)| {
                slot.occupant()
                    .is_some_and(|state| state.config().destination == destination)
            })
            .map(|(index, slot)| EspNowEncryptedPeerId {
                index,
                generation: slot.generation(),
            })
    }

    pub fn get(
        &self,
        peer: EspNowEncryptedPeerId,
    ) -> Result<EspNowEncryptedPeerView, EspNowEncryptedPeerError> {
        let state = self.state(peer)?;
        let config = state.config();
        Ok(EspNowEncryptedPeerView {
            destination: config.destination,
            channel: config.channel,
            phy_mode: config.phy_mode,
            last_tx_packet_number: state.key.packet_numbers().last().value(),
            highest_authenticated_rx_packet_number: highest_authenticated(&state.replay),
        })
    }

    pub fn lmk(&self, peer: EspNowEncryptedPeerId) -> Result<&EspNowLmk, EspNowEncryptedPeerError> {
        Ok(&self.state(peer)?.config().lmk)
    }

    /// Burn and return the next per-peer TX PN. A caller must never roll this
    /// value back after a failed publication because reuse under one LMK would
    /// violate CCMP nonce uniqueness.
    pub fn allocate_tx_packet_number(
        &mut self,
        peer: EspNowEncryptedPeerId,
    ) -> Result<EspNowCcmpPacketNumber, EspNowEncryptedPeerError> {
        // The allocator starts after zero, so every number it hands out is
        // an ESP-NOW packet number.
        let packet_number = self
            .state(peer)?
            .key
            .packet_numbers()
            .allocate()
            .ok()
            .and_then(|next| EspNowCcmpPacketNumber::new(next.value()).ok());
        let Some(packet_number) = packet_number else {
            return Err(EspNowEncryptedPeerError::TxPacketNumberExhausted(peer));
        };
        Ok(packet_number)
    }

    /// Check one unauthenticated on-air PN against the peer's replay
    /// frontier and lend that frontier to the returned candidate. Only a
    /// later successful MIC verification may commit it; dropping it cancels
    /// the check. The borrow keeps the peer from being replaced or removed
    /// in between.
    pub fn prepare_rx_replay(
        &mut self,
        peer: EspNowEncryptedPeerId,
        packet_number: EspNowCcmpPacketNumber,
    ) -> Result<EspNowRxReplayCandidate<'_>, EspNowEncryptedPeerError> {
        let state = self
            .slots
            .get_mut(peer.index)
            .and_then(|slot| slot.state_mut(peer.generation))
            .ok_or(EspNowEncryptedPeerError::Stale(peer))?;
        match state.replay.prepare(CcmpPacketNumber::from(packet_number)) {
            Ok(replay) => Ok(EspNowRxReplayCandidate {
                peer,
                packet_number,
                replay,
            }),
            Err(CcmpReplayed { highest, .. }) => Err(EspNowEncryptedPeerError::Replay {
                peer,
                packet_number,
                highest,
            }),
        }
    }

    fn state(
        &self,
        peer: EspNowEncryptedPeerId,
    ) -> Result<&EspNowEncryptedPeerState, EspNowEncryptedPeerError> {
        match self.slots.get(peer.index) {
            Some(EspNowEncryptedPeerSlot::Occupied { generation, state })
                if *generation == peer.generation =>
            {
                Ok(state)
            }
            Some(_) | None => Err(EspNowEncryptedPeerError::Stale(peer)),
        }
    }

    fn mutation_failure(
        &mut self,
        error: EspNowEncryptedPeerError,
        config: EspNowEncryptedPeerConfig,
    ) -> EspNowEncryptedPeerMutationFailure {
        EspNowEncryptedPeerMutationFailure { error, config }
    }
}

/// Successful peer replacement and the old key-owning configuration.
pub struct EspNowEncryptedPeerReplacement {
    pub peer: EspNowEncryptedPeerId,
    pub replaced: EspNowEncryptedPeerConfig,
}

impl fmt::Debug for EspNowEncryptedPeerReplacement {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("EspNowEncryptedPeerReplacement")
            .field("peer", &self.peer)
            .field("replaced", &self.replaced)
            .finish()
    }
}

/// Removed peer state retained until an external hardware clear succeeds.
/// Dropping this token zeroizes the LMK. Restoring it preserves both PN spaces
/// and advances the slot generation.
pub struct EspNowRemovedEncryptedPeer {
    index: usize,
    generation: u32,
    state: EspNowEncryptedPeerState,
}

impl EspNowRemovedEncryptedPeer {
    pub const fn previous_id(&self) -> EspNowEncryptedPeerId {
        EspNowEncryptedPeerId {
            index: self.index,
            generation: self.generation,
        }
    }

    pub const fn config(&self) -> &EspNowEncryptedPeerConfig {
        self.state.config()
    }

    /// Finish a successful external removal and return the key-owning config.
    pub fn into_config(self) -> EspNowEncryptedPeerConfig {
        self.state.key.into_key()
    }
}

impl fmt::Debug for EspNowRemovedEncryptedPeer {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("EspNowRemovedEncryptedPeer")
            .field("previous_id", &self.previous_id())
            .field("config", self.config())
            .field(
                "last_tx_packet_number",
                &self.state.key.packet_numbers().last().value(),
            )
            .field(
                "highest_authenticated_rx_packet_number",
                &highest_authenticated(&self.state.replay),
            )
            .finish()
    }
}

/// Failed remove rollback retaining the complete removed state.
pub struct EspNowEncryptedPeerRestoreFailure {
    error: EspNowEncryptedPeerError,
    removed: EspNowRemovedEncryptedPeer,
}

impl EspNowEncryptedPeerRestoreFailure {
    pub const fn error(&self) -> EspNowEncryptedPeerError {
        self.error
    }

    pub fn into_parts(self) -> (EspNowEncryptedPeerError, EspNowRemovedEncryptedPeer) {
        (self.error, self.removed)
    }
}

impl fmt::Debug for EspNowEncryptedPeerRestoreFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("EspNowEncryptedPeerRestoreFailure")
            .field("error", &self.error)
            .field("removed", &self.removed)
            .finish()
    }
}

/// Failed peer install/replace retaining the complete proposed config.
pub struct EspNowEncryptedPeerMutationFailure {
    error: EspNowEncryptedPeerError,
    config: EspNowEncryptedPeerConfig,
}

impl EspNowEncryptedPeerMutationFailure {
    pub const fn error(&self) -> EspNowEncryptedPeerError {
        self.error
    }

    pub fn into_parts(self) -> (EspNowEncryptedPeerError, EspNowEncryptedPeerConfig) {
        (self.error, self.config)
    }
}

impl fmt::Debug for EspNowEncryptedPeerMutationFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("EspNowEncryptedPeerMutationFailure")
            .field("error", &self.error)
            .field("config", &self.config)
            .finish()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EspNowEncryptedPeerError {
    Full,
    Duplicate(EspNowUnicastAddress),
    ReplacementDestinationMismatch {
        installed: EspNowUnicastAddress,
        proposed: EspNowUnicastAddress,
    },
    ChannelMismatch {
        peer: Ghz2_4Channel,
        home: Ghz2_4Channel,
    },
    GenerationExhausted,
    Stale(EspNowEncryptedPeerId),
    TxPacketNumberExhausted(EspNowEncryptedPeerId),
    /// The PN does not exceed the highest authenticated one of the peer.
    Replay {
        peer: EspNowEncryptedPeerId,
        packet_number: EspNowCcmpPacketNumber,
        highest: CcmpPacketNumber,
    },
}

impl fmt::Display for EspNowEncryptedPeerError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Full => formatter.write_str("the encrypted ESP-NOW peer table is full"),
            Self::Duplicate(destination) => {
                write!(
                    formatter,
                    "encrypted ESP-NOW peer {destination:?} already exists"
                )
            }
            Self::ReplacementDestinationMismatch {
                installed,
                proposed,
            } => write!(
                formatter,
                "encrypted ESP-NOW peer replacement changes identity from {installed:?} to {proposed:?}"
            ),
            Self::ChannelMismatch { peer, home } => write!(
                formatter,
                "encrypted ESP-NOW peer channel {peer:?} differs from home channel {home:?}"
            ),
            Self::GenerationExhausted => {
                formatter.write_str("an encrypted ESP-NOW peer generation counter is exhausted")
            }
            Self::Stale(peer) => write!(formatter, "encrypted ESP-NOW peer {peer:?} is stale"),
            Self::TxPacketNumberExhausted(peer) => {
                write!(
                    formatter,
                    "encrypted ESP-NOW peer {peer:?} exhausted its TX PN"
                )
            }
            Self::Replay {
                peer,
                packet_number,
                highest,
            } => write!(
                formatter,
                "encrypted ESP-NOW peer {peer:?} PN {} does not exceed {}",
                packet_number.value(),
                highest.value()
            ),
        }
    }
}

impl core::error::Error for EspNowEncryptedPeerError {}

/// An unauthenticated PN admitted by its peer's replay frontier, holding
/// that frontier exclusively. Committing advances it; dropping cancels.
#[must_use = "commit after authentication, or drop to cancel admission"]
pub struct EspNowRxReplayCandidate<'a> {
    peer: EspNowEncryptedPeerId,
    packet_number: EspNowCcmpPacketNumber,
    replay: CcmpReplayCandidate<'a>,
}

impl fmt::Debug for EspNowRxReplayCandidate<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("EspNowRxReplayCandidate")
            .field("peer", &self.peer)
            .field("packet_number", &self.packet_number)
            .field("replay", &self.replay)
            .finish_non_exhaustive()
    }
}

impl EspNowRxReplayCandidate<'_> {
    pub const fn peer(&self) -> EspNowEncryptedPeerId {
        self.peer
    }

    pub const fn packet_number(&self) -> EspNowCcmpPacketNumber {
        self.packet_number
    }

    /// Commit only after a cryptographic owner has authenticated the MIC of
    /// the exact frame which produced this candidate.
    pub fn commit_authenticated(self) {
        self.replay.commit();
    }
}

/// Portable owner combining one station binding, PMK and encrypted peers.
///
pub struct EspNowEncryptedProtocol<const N: usize = ESP_NOW_DEFAULT_ENCRYPTED_PEER_CAPACITY> {
    config: EspNowConfig,
    pmk: EspNowPmkOwner,
    peers: EspNowEncryptedPeerTable<N>,
}

impl<const N: usize> EspNowEncryptedProtocol<N> {
    pub fn new(config: EspNowConfig, pmk: EspNowPmk) -> Self {
        Self {
            config,
            pmk: EspNowPmkOwner::from_key(pmk),
            peers: EspNowEncryptedPeerTable::new(config.home_channel()),
        }
    }

    pub const fn config(&self) -> EspNowConfig {
        self.config
    }

    pub const fn pmk(&self) -> &EspNowPmkOwner {
        &self.pmk
    }

    pub fn pmk_mut(&mut self) -> &mut EspNowPmkOwner {
        &mut self.pmk
    }

    pub const fn peers(&self) -> &EspNowEncryptedPeerTable<N> {
        &self.peers
    }

    pub fn peers_mut(&mut self) -> &mut EspNowEncryptedPeerTable<N> {
        &mut self.peers
    }

    pub fn add_peer(
        &mut self,
        peer: EspNowEncryptedPeerConfig,
    ) -> Result<EspNowEncryptedPeerId, EspNowEncryptedPeerMutationFailure> {
        self.peers.install(peer)
    }

    pub fn replace_peer(
        &mut self,
        peer: EspNowEncryptedPeerId,
        replacement: EspNowEncryptedPeerConfig,
    ) -> Result<EspNowEncryptedPeerReplacement, EspNowEncryptedPeerMutationFailure> {
        self.peers.replace(peer, replacement)
    }

    pub fn remove_peer(
        &mut self,
        peer: EspNowEncryptedPeerId,
    ) -> Result<EspNowRemovedEncryptedPeer, EspNowEncryptedPeerError> {
        self.peers.remove(peer)
    }

    pub fn restore_removed_peer(
        &mut self,
        removed: EspNowRemovedEncryptedPeer,
    ) -> Result<EspNowEncryptedPeerId, EspNowEncryptedPeerRestoreFailure> {
        self.peers.restore_removed(removed)
    }

    /// Build all portable TX metadata and burn one peer-local PN. Final frame
    /// construction remains fail-closed at [`EspNowPreparedEncryptedV1Tx::encode`].
    pub fn prepare_v1_tx<'payload>(
        &mut self,
        peer: EspNowEncryptedPeerId,
        sequence: &mut SequenceCounter,
        random_value: EspNowRandomValue,
        payload: &'payload [u8],
    ) -> Result<EspNowPreparedEncryptedV1Tx<'payload>, EspNowEncryptedSendError> {
        let payload = EspNowV1Payload::new(payload).map_err(EspNowEncryptedSendError::Wire)?;
        let encoded_len =
            encrypted_v1_encoded_len(payload.len()).map_err(EspNowEncryptedSendError::Wire)?;
        let peer_view = self
            .peers
            .get(peer)
            .map_err(EspNowEncryptedSendError::Peer)?;
        let packet_number = self
            .peers
            .allocate_tx_packet_number(peer)
            .map_err(EspNowEncryptedSendError::Peer)?;
        Ok(EspNowPreparedEncryptedV1Tx {
            peer,
            home_channel: self.config.home_channel(),
            station: self.config.station(),
            source: self.config.local_address(),
            destination: peer_view.destination,
            phy_mode: peer_view.phy_mode,
            sequence_number: sequence.take(),
            packet_number,
            random_value,
            payload,
            encoded_len,
        })
    }

    /// Parse and address-check protected metadata, then prepare a replay
    /// proposal without mutating the authenticated window.
    pub fn prepare_receive_v1<'frame>(
        &mut self,
        active_station: BoundVirtualInterface,
        active_channel: Ghz2_4Channel,
        bytes: &'frame [u8],
    ) -> Result<EspNowEncryptedRxCandidate<'_, 'frame>, EspNowEncryptedReceiveError> {
        if active_station != self.config.station() {
            return Err(EspNowEncryptedReceiveError::StationBindingMismatch {
                configured: self.config.station(),
                active: active_station,
            });
        }
        if active_channel != self.config.home_channel() {
            return Err(EspNowEncryptedReceiveError::ChannelMismatch {
                configured: self.config.home_channel(),
                active: active_channel,
            });
        }
        let envelope =
            EspNowProtectedV1Envelope::parse(bytes).map_err(EspNowEncryptedReceiveError::Wire)?;
        if envelope.destination() != self.config.local_address() {
            return Err(EspNowEncryptedReceiveError::ForeignDestination(
                envelope.destination(),
            ));
        }
        let source = envelope.source();
        let peer = self
            .peers
            .find(source)
            .ok_or(EspNowEncryptedReceiveError::UnknownPeer(source))?;
        let replay = self
            .peers
            .prepare_rx_replay(peer, envelope.packet_number())
            .map_err(EspNowEncryptedReceiveError::Peer)?;
        Ok(EspNowEncryptedRxCandidate {
            peer,
            envelope,
            replay,
        })
    }

    /// End the security epoch, zeroizing all LMKs and the PMK on drop. The
    /// returned config contains no secret and can seed a later fresh epoch.
    pub fn shutdown(mut self, events: &mut impl EventSink<EspNowPeersCleared>) -> EspNowConfig {
        let cleared = self.peers.clear_on_shutdown();
        report(events, || EspNowPeersCleared {
            count: u32::try_from(cleared).unwrap_or(u32::MAX),
        });
        self.config
    }
}

fn encrypted_v1_encoded_len(payload_length: usize) -> Result<usize, EspNowV1WireError> {
    ESP_NOW_V1_MIN_PROTECTED_MPDU_LEN
        .checked_add(payload_length)
        .ok_or(EspNowV1WireError::PayloadTooLong {
            length: payload_length,
        })
}

/// Portable encrypted TX metadata. No method exposes the LMK and `encode`
/// always fails at the first unproven interoperable stage.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EspNowPreparedEncryptedV1Tx<'payload> {
    peer: EspNowEncryptedPeerId,
    home_channel: Ghz2_4Channel,
    station: BoundVirtualInterface,
    source: EspNowUnicastAddress,
    destination: EspNowUnicastAddress,
    phy_mode: EspNowPhyMode,
    sequence_number: SequenceNumber,
    packet_number: EspNowCcmpPacketNumber,
    random_value: EspNowRandomValue,
    payload: EspNowV1Payload<'payload>,
    encoded_len: usize,
}

impl<'payload> EspNowPreparedEncryptedV1Tx<'payload> {
    pub const fn peer(self) -> EspNowEncryptedPeerId {
        self.peer
    }

    pub const fn home_channel(self) -> Ghz2_4Channel {
        self.home_channel
    }

    pub const fn station(self) -> BoundVirtualInterface {
        self.station
    }

    pub const fn channel_context(self) -> ChannelContextId {
        self.station.channel_context
    }

    pub const fn source(self) -> EspNowUnicastAddress {
        self.source
    }

    pub const fn destination(self) -> EspNowUnicastAddress {
        self.destination
    }

    pub const fn phy_mode(self) -> EspNowPhyMode {
        self.phy_mode
    }

    pub const fn sequence_number(self) -> SequenceNumber {
        self.sequence_number
    }

    pub const fn packet_number(self) -> EspNowCcmpPacketNumber {
        self.packet_number
    }

    pub const fn random_value(self) -> EspNowRandomValue {
        self.random_value
    }

    pub const fn payload(self) -> EspNowV1Payload<'payload> {
        self.payload
    }

    pub const fn security(self) -> EspNowPeerSecurity {
        EspNowPeerSecurity::Encrypted
    }

    pub const fn encoded_len(self) -> usize {
        self.encoded_len
    }

    pub fn encode(self, _output: &mut [u8]) -> Result<usize, EspNowEncryptedV1Unavailable> {
        Err(EspNowEncryptedV1Unavailable::ActionAadContractUnproven)
    }
}

/// Structurally admitted protected frame and its uncommitted replay token.
pub struct EspNowEncryptedRxCandidate<'a, 'frame> {
    peer: EspNowEncryptedPeerId,
    envelope: EspNowProtectedV1Envelope<'frame>,
    replay: EspNowRxReplayCandidate<'a>,
}

impl fmt::Debug for EspNowEncryptedRxCandidate<'_, '_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("EspNowEncryptedRxCandidate")
            .field("peer", &self.peer)
            .field("envelope", &self.envelope)
            .field("replay", &self.replay)
            .finish()
    }
}

impl<'a, 'frame> EspNowEncryptedRxCandidate<'a, 'frame> {
    pub const fn peer(&self) -> EspNowEncryptedPeerId {
        self.peer
    }

    pub const fn envelope(&self) -> EspNowProtectedV1Envelope<'frame> {
        self.envelope
    }

    /// Consume the structural candidate into the token which a future exact
    /// decrypt/MIC backend must commit only after successful authentication.
    pub fn into_replay_candidate(self) -> EspNowRxReplayCandidate<'a> {
        self.replay
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EspNowEncryptedSendError {
    Peer(EspNowEncryptedPeerError),
    Wire(EspNowV1WireError),
}

impl fmt::Display for EspNowEncryptedSendError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Peer(error) => write!(formatter, "encrypted ESP-NOW peer error: {error}"),
            Self::Wire(error) => write!(formatter, "encrypted ESP-NOW frame error: {error}"),
        }
    }
}

impl core::error::Error for EspNowEncryptedSendError {}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EspNowEncryptedReceiveError {
    StationBindingMismatch {
        configured: BoundVirtualInterface,
        active: BoundVirtualInterface,
    },
    ChannelMismatch {
        configured: Ghz2_4Channel,
        active: Ghz2_4Channel,
    },
    Wire(EspNowProtectedV1WireError),
    ForeignDestination(EspNowUnicastAddress),
    UnknownPeer(EspNowUnicastAddress),
    Peer(EspNowEncryptedPeerError),
}

impl fmt::Display for EspNowEncryptedReceiveError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::StationBindingMismatch { configured, active } => write!(
                formatter,
                "encrypted ESP-NOW station binding {configured:?} differs from active binding {active:?}"
            ),
            Self::ChannelMismatch { configured, active } => write!(
                formatter,
                "encrypted ESP-NOW channel {configured:?} differs from active channel {active:?}"
            ),
            Self::Wire(error) => write!(formatter, "protected ESP-NOW frame error: {error}"),
            Self::ForeignDestination(destination) => write!(
                formatter,
                "protected ESP-NOW frame is addressed to {destination:?}"
            ),
            Self::UnknownPeer(source) => {
                write!(
                    formatter,
                    "protected ESP-NOW source {source:?} is not configured"
                )
            }
            Self::Peer(error) => write!(formatter, "encrypted ESP-NOW peer error: {error}"),
        }
    }
}

impl core::error::Error for EspNowEncryptedReceiveError {}

/// Explicit protocol-level fail-closed status for callers probing encrypted
/// v1 interoperability before allocating a packet number.
pub const fn esp_now_encrypted_v1_codec_status() -> Result<(), EspNowEncryptedV1Unavailable> {
    Err(EspNowEncryptedV1Unavailable::ActionAadContractUnproven)
}

/// Convert an encrypted peer address to the exact destination domain used by
/// common plaintext APIs without weakening encrypted-unicast-only admission.
pub const fn encrypted_peer_destination(address: EspNowUnicastAddress) -> EspNowDestination {
    EspNowDestination::Unicast(address)
}

#[cfg(test)]
mod tests;
