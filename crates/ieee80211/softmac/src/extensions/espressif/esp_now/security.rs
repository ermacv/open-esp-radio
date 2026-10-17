//! Portable ESP-NOW encrypted-peer and anti-replay ownership.
//!
//! This module stops before the first interoperability contract not present
//! in the reviewed repository evidence: the ESP-NOW Action-frame CCMP AAD and
//! the chip key-selector/construct/decrypt callback ABI. It therefore owns
//! secrets, peer generations, packet numbers and replay state, but never
//! claims to produce or authenticate encrypted on-air bytes.

use core::{array, fmt};
use oer_ieee80211_mac::sequence::SequenceNumber;

use oer_ieee80211_mac::{
    channel::WifiChannel,
    extensions::espressif::esp_now::{
        ESP_NOW_CCMP_HEADER_LEN, ESP_NOW_CCMP_MIC_LEN, ESP_NOW_MANAGEMENT_HEADER_LEN,
        ESP_NOW_V1_ACTION_OVERHEAD, EspNowCcmpPacketNumber, EspNowDestination,
        EspNowEncryptedV1Unavailable, EspNowProtectedV1Envelope, EspNowProtectedV1WireError,
        EspNowRandomValue, EspNowUnicastAddress, EspNowV1Payload, EspNowV1WireError,
    },
    station::StaSequenceCounter,
};
use zeroize::{Zeroize, ZeroizeOnDrop};

use crate::{
    extensions::espressif::esp_now::protocol::{
        ESP_NOW_DEFAULT_PEER_CAPACITY, EspNowConfig, EspNowPeerSecurity, EspNowPhyMode,
    },
    interface::{BoundVirtualInterface, ChannelContextId},
};

pub const ESP_NOW_KEY_LEN: usize = 16;
pub const ESP_NOW_RX_REPLAY_WINDOW_BITS: u32 = 64;
/// Portable encrypted-peer storage default. This is not a hardware-key-slot
/// claim; each backend advertises only its proven encrypted-slot capacity.
pub const ESP_NOW_DEFAULT_ENCRYPTED_PEER_CAPACITY: usize = ESP_NOW_DEFAULT_PEER_CAPACITY;
const ESP_NOW_PACKET_NUMBER_MAX: u64 = (1_u64 << 48) - 1;

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

/// One optional PMK and its replacement/removal generation.
pub struct EspNowPmkOwner {
    generation: u32,
    key: Option<EspNowPmk>,
}

impl EspNowPmkOwner {
    pub const fn new() -> Self {
        Self {
            generation: 0,
            key: None,
        }
    }

    pub fn from_key(key: EspNowPmk) -> Self {
        Self {
            generation: 1,
            key: Some(key),
        }
    }

    pub const fn current_id(&self) -> Option<EspNowPmkId> {
        if self.key.is_some() {
            Some(EspNowPmkId {
                generation: self.generation,
            })
        } else {
            None
        }
    }

    pub fn install(&mut self, key: EspNowPmk) -> Result<EspNowPmkId, EspNowPmkMutationFailure> {
        if self.key.is_some() {
            return Err(EspNowPmkMutationFailure::new(
                EspNowPmkError::AlreadyInstalled,
                key,
            ));
        }
        let Some(generation) = self.generation.checked_add(1) else {
            return Err(EspNowPmkMutationFailure::new(
                EspNowPmkError::GenerationExhausted,
                key,
            ));
        };
        self.generation = generation;
        self.key = Some(key);
        Ok(EspNowPmkId { generation })
    }

    /// Atomically replace a PMK. Validation happens before either key moves;
    /// failure returns the proposed key and leaves the installed key intact.
    pub fn replace(
        &mut self,
        current: EspNowPmkId,
        replacement: EspNowPmk,
    ) -> Result<(EspNowPmkId, EspNowPmk), EspNowPmkMutationFailure> {
        if self.current_id() != Some(current) {
            return Err(EspNowPmkMutationFailure::new(
                EspNowPmkError::Stale(current),
                replacement,
            ));
        }
        let Some(generation) = self.generation.checked_add(1) else {
            return Err(EspNowPmkMutationFailure::new(
                EspNowPmkError::GenerationExhausted,
                replacement,
            ));
        };
        let old = self
            .key
            .replace(replacement)
            .expect("validated ESP-NOW PMK owner is occupied");
        self.generation = generation;
        Ok((EspNowPmkId { generation }, old))
    }

    pub fn remove(&mut self, current: EspNowPmkId) -> Result<EspNowPmk, EspNowPmkError> {
        if self.current_id() != Some(current) {
            return Err(EspNowPmkError::Stale(current));
        }
        self.key.take().ok_or(EspNowPmkError::Missing)
    }

    pub fn key(&self, current: EspNowPmkId) -> Result<&EspNowPmk, EspNowPmkError> {
        if self.current_id() != Some(current) {
            return Err(EspNowPmkError::Stale(current));
        }
        self.key.as_ref().ok_or(EspNowPmkError::Missing)
    }
}

impl Default for EspNowPmkOwner {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EspNowPmkError {
    AlreadyInstalled,
    Missing,
    GenerationExhausted,
    Stale(EspNowPmkId),
}

impl fmt::Display for EspNowPmkError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AlreadyInstalled => formatter.write_str("an ESP-NOW PMK is already installed"),
            Self::Missing => formatter.write_str("no ESP-NOW PMK is installed"),
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
    channel: WifiChannel,
    phy_mode: EspNowPhyMode,
    lmk: EspNowLmk,
}

impl EspNowEncryptedPeerConfig {
    pub const fn new(
        destination: EspNowUnicastAddress,
        channel: WifiChannel,
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

    pub const fn channel(&self) -> WifiChannel {
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

    pub fn into_parts(self) -> (EspNowUnicastAddress, WifiChannel, EspNowPhyMode, EspNowLmk) {
        (self.destination, self.channel, self.phy_mode, self.lmk)
    }
}

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
    pub channel: WifiChannel,
    pub phy_mode: EspNowPhyMode,
    pub last_tx_packet_number: u64,
    pub highest_authenticated_rx_packet_number: Option<u64>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct EspNowReplayWindow {
    highest: Option<EspNowCcmpPacketNumber>,
    bitmap: u64,
    revision: u32,
}

struct EspNowEncryptedPeerState {
    config: EspNowEncryptedPeerConfig,
    last_tx_packet_number: u64,
    replay: EspNowReplayWindow,
}

struct EspNowEncryptedPeerSlot {
    generation: u32,
    state: Option<EspNowEncryptedPeerState>,
}

impl EspNowEncryptedPeerSlot {
    const fn empty() -> Self {
        Self {
            generation: 0,
            state: None,
        }
    }
}

/// Diagnostics contain only counters and never key material.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct EspNowEncryptedPeerDiagnostics {
    pub installs: u32,
    pub replacements: u32,
    pub removals: u32,
    pub remove_rollbacks: u32,
    pub cleared_on_shutdown: u32,
    pub mutation_rejections: u32,
    pub tx_packet_numbers_allocated: u32,
    pub tx_packet_number_exhaustions: u32,
    pub replay_candidates: u32,
    pub authenticated_replay_commits: u32,
    pub replay_duplicates: u32,
    pub replay_too_old: u32,
    pub stale_replay_candidates: u32,
}

/// Fixed-capacity owner of encrypted individual peers and their PN spaces.
pub struct EspNowEncryptedPeerTable<const N: usize = ESP_NOW_DEFAULT_ENCRYPTED_PEER_CAPACITY> {
    home_channel: WifiChannel,
    slots: [EspNowEncryptedPeerSlot; N],
    length: usize,
    diagnostics: EspNowEncryptedPeerDiagnostics,
}

impl<const N: usize> EspNowEncryptedPeerTable<N> {
    pub fn new(home_channel: WifiChannel) -> Self {
        Self {
            home_channel,
            slots: array::from_fn(|_| EspNowEncryptedPeerSlot::empty()),
            length: 0,
            diagnostics: EspNowEncryptedPeerDiagnostics::default(),
        }
    }

    pub const fn home_channel(&self) -> WifiChannel {
        self.home_channel
    }

    pub const fn len(&self) -> usize {
        self.length
    }

    pub const fn is_empty(&self) -> bool {
        self.length == 0
    }

    pub const fn capacity(&self) -> usize {
        N
    }

    pub const fn diagnostics(&self) -> EspNowEncryptedPeerDiagnostics {
        self.diagnostics
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

        let mut reusable = None;
        let mut exhausted_slot = false;
        for (index, slot) in self.slots.iter().enumerate() {
            if slot
                .state
                .as_ref()
                .is_some_and(|peer| peer.config.destination == config.destination)
            {
                return Err(self.mutation_failure(
                    EspNowEncryptedPeerError::Duplicate(config.destination),
                    config,
                ));
            }
            if slot.state.is_none() && reusable.is_none() {
                if slot.generation == u32::MAX {
                    exhausted_slot = true;
                } else {
                    reusable = Some(index);
                }
            }
        }

        let Some(index) = reusable else {
            let error = if exhausted_slot && self.length < N {
                EspNowEncryptedPeerError::GenerationExhausted
            } else {
                EspNowEncryptedPeerError::Full
            };
            return Err(self.mutation_failure(error, config));
        };
        let slot = &mut self.slots[index];
        slot.generation += 1;
        slot.state = Some(EspNowEncryptedPeerState {
            config,
            last_tx_packet_number: 0,
            replay: EspNowReplayWindow::default(),
        });
        self.length += 1;
        self.diagnostics.installs = self.diagnostics.installs.saturating_add(1);
        Ok(EspNowEncryptedPeerId {
            index,
            generation: slot.generation,
        })
    }

    /// Atomically rotate or replace a peer. Failure retains the proposed LMK
    /// and leaves the existing key, PN and replay window unchanged.
    pub fn replace(
        &mut self,
        peer: EspNowEncryptedPeerId,
        replacement: EspNowEncryptedPeerConfig,
    ) -> Result<EspNowEncryptedPeerReplacement, EspNowEncryptedPeerMutationFailure> {
        let Some(slot) = self.slots.get(peer.index) else {
            return Err(self.mutation_failure(EspNowEncryptedPeerError::Stale(peer), replacement));
        };
        if slot.generation != peer.generation || slot.state.is_none() {
            return Err(self.mutation_failure(EspNowEncryptedPeerError::Stale(peer), replacement));
        }
        if replacement.channel != self.home_channel {
            return Err(self.mutation_failure(
                EspNowEncryptedPeerError::ChannelMismatch {
                    peer: replacement.channel,
                    home: self.home_channel,
                },
                replacement,
            ));
        }
        let installed_destination = self.slots[peer.index]
            .state
            .as_ref()
            .expect("validated ESP-NOW encrypted peer slot is occupied")
            .config
            .destination;
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

        let slot = &mut self.slots[peer.index];
        let old = slot
            .state
            .take()
            .expect("validated ESP-NOW encrypted peer slot is occupied");
        // Preserve both packet-number spaces across replacement. This is
        // conservative for a fresh LMK and prevents nonce reuse if a caller
        // supplies the same LMK bytes again.
        slot.state = Some(EspNowEncryptedPeerState {
            config: replacement,
            last_tx_packet_number: old.last_tx_packet_number,
            replay: old.replay,
        });
        slot.generation = generation;
        self.diagnostics.replacements = self.diagnostics.replacements.saturating_add(1);
        Ok(EspNowEncryptedPeerReplacement {
            peer: EspNowEncryptedPeerId {
                index: peer.index,
                generation,
            },
            replaced: old.config,
        })
    }

    pub fn remove(
        &mut self,
        peer: EspNowEncryptedPeerId,
    ) -> Result<EspNowRemovedEncryptedPeer, EspNowEncryptedPeerError> {
        let slot = self.slot_mut(peer)?;
        let state = slot
            .state
            .take()
            .ok_or(EspNowEncryptedPeerError::Stale(peer))?;
        self.length -= 1;
        self.diagnostics.removals = self.diagnostics.removals.saturating_add(1);
        Ok(EspNowRemovedEncryptedPeer {
            index: peer.index,
            generation: peer.generation,
            config: state.config,
            last_tx_packet_number: state.last_tx_packet_number,
            replay: state.replay,
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
        let Some(slot) = self.slots.get(removed.index) else {
            return Err(EspNowEncryptedPeerRestoreFailure {
                error: EspNowEncryptedPeerError::Stale(EspNowEncryptedPeerId {
                    index: removed.index,
                    generation: removed.generation,
                }),
                removed,
            });
        };
        if slot.generation != removed.generation || slot.state.is_some() {
            return Err(EspNowEncryptedPeerRestoreFailure {
                error: EspNowEncryptedPeerError::Stale(EspNowEncryptedPeerId {
                    index: removed.index,
                    generation: removed.generation,
                }),
                removed,
            });
        }
        let Some(generation) = removed.generation.checked_add(1) else {
            return Err(EspNowEncryptedPeerRestoreFailure {
                error: EspNowEncryptedPeerError::GenerationExhausted,
                removed,
            });
        };
        if removed.config.channel != self.home_channel {
            return Err(EspNowEncryptedPeerRestoreFailure {
                error: EspNowEncryptedPeerError::ChannelMismatch {
                    peer: removed.config.channel,
                    home: self.home_channel,
                },
                removed,
            });
        }
        if self.slots.iter().any(|slot| {
            slot.state
                .as_ref()
                .is_some_and(|state| state.config.destination == removed.config.destination)
        }) {
            return Err(EspNowEncryptedPeerRestoreFailure {
                error: EspNowEncryptedPeerError::Duplicate(removed.config.destination),
                removed,
            });
        }

        let EspNowRemovedEncryptedPeer {
            index,
            generation: _,
            config,
            last_tx_packet_number,
            replay,
        } = removed;
        self.slots[index] = EspNowEncryptedPeerSlot {
            generation,
            state: Some(EspNowEncryptedPeerState {
                config,
                last_tx_packet_number,
                replay,
            }),
        };
        self.length += 1;
        self.diagnostics.remove_rollbacks = self.diagnostics.remove_rollbacks.saturating_add(1);
        Ok(EspNowEncryptedPeerId { index, generation })
    }

    /// Drop and zeroize every LMK. Existing handles become stale immediately;
    /// a later install advances each reused slot generation.
    pub fn clear_on_shutdown(&mut self) -> usize {
        let mut cleared = 0;
        for slot in &mut self.slots {
            if slot.state.take().is_some() {
                cleared += 1;
            }
        }
        self.length = 0;
        self.diagnostics.cleared_on_shutdown = self
            .diagnostics
            .cleared_on_shutdown
            .saturating_add(u32::try_from(cleared).unwrap_or(u32::MAX));
        cleared
    }

    pub fn find(&self, destination: EspNowUnicastAddress) -> Option<EspNowEncryptedPeerId> {
        self.slots
            .iter()
            .enumerate()
            .find(|(_, slot)| {
                slot.state
                    .as_ref()
                    .is_some_and(|state| state.config.destination == destination)
            })
            .map(|(index, slot)| EspNowEncryptedPeerId {
                index,
                generation: slot.generation,
            })
    }

    pub fn get(
        &self,
        peer: EspNowEncryptedPeerId,
    ) -> Result<EspNowEncryptedPeerView, EspNowEncryptedPeerError> {
        let state = self.state(peer)?;
        Ok(EspNowEncryptedPeerView {
            destination: state.config.destination,
            channel: state.config.channel,
            phy_mode: state.config.phy_mode,
            last_tx_packet_number: state.last_tx_packet_number,
            highest_authenticated_rx_packet_number: state.replay.highest.map(|pn| pn.value()),
        })
    }

    pub fn lmk(&self, peer: EspNowEncryptedPeerId) -> Result<&EspNowLmk, EspNowEncryptedPeerError> {
        Ok(&self.state(peer)?.config.lmk)
    }

    /// Burn and return the next per-peer TX PN. A caller must never roll this
    /// value back after a failed publication because reuse under one LMK would
    /// violate CCMP nonce uniqueness.
    pub fn allocate_tx_packet_number(
        &mut self,
        peer: EspNowEncryptedPeerId,
    ) -> Result<EspNowCcmpPacketNumber, EspNowEncryptedPeerError> {
        let state = self.state_mut(peer)?;
        if state.last_tx_packet_number == ESP_NOW_PACKET_NUMBER_MAX {
            self.diagnostics.tx_packet_number_exhaustions = self
                .diagnostics
                .tx_packet_number_exhaustions
                .saturating_add(1);
            return Err(EspNowEncryptedPeerError::TxPacketNumberExhausted(peer));
        }
        state.last_tx_packet_number += 1;
        let packet_number = EspNowCcmpPacketNumber::new(state.last_tx_packet_number)
            .expect("bounded nonzero ESP-NOW TX packet number");
        self.diagnostics.tx_packet_numbers_allocated = self
            .diagnostics
            .tx_packet_numbers_allocated
            .saturating_add(1);
        Ok(packet_number)
    }

    /// Check one unauthenticated on-air PN without mutating replay state.
    /// Only a later successful MIC verification may commit the returned token.
    pub fn prepare_rx_replay(
        &mut self,
        peer: EspNowEncryptedPeerId,
        packet_number: EspNowCcmpPacketNumber,
    ) -> Result<EspNowRxReplayCandidate, EspNowEncryptedPeerError> {
        let state = self.state(peer)?;
        let expected_revision = state.replay.revision;
        let (highest, bitmap) = match state.replay.highest {
            None => (packet_number, 1),
            Some(highest) if packet_number > highest => {
                let shift = packet_number.value() - highest.value();
                let bitmap = if shift >= u64::from(ESP_NOW_RX_REPLAY_WINDOW_BITS) {
                    1
                } else {
                    (state.replay.bitmap << shift) | 1
                };
                (packet_number, bitmap)
            }
            Some(highest) => {
                let distance = highest.value() - packet_number.value();
                if distance >= u64::from(ESP_NOW_RX_REPLAY_WINDOW_BITS) {
                    self.diagnostics.replay_too_old =
                        self.diagnostics.replay_too_old.saturating_add(1);
                    return Err(EspNowEncryptedPeerError::ReplayTooOld {
                        peer,
                        packet_number,
                        highest,
                    });
                }
                let bit = 1_u64 << distance;
                if state.replay.bitmap & bit != 0 {
                    self.diagnostics.replay_duplicates =
                        self.diagnostics.replay_duplicates.saturating_add(1);
                    return Err(EspNowEncryptedPeerError::ReplayDuplicate {
                        peer,
                        packet_number,
                    });
                }
                (highest, state.replay.bitmap | bit)
            }
        };
        self.diagnostics.replay_candidates = self.diagnostics.replay_candidates.saturating_add(1);
        Ok(EspNowRxReplayCandidate {
            peer,
            packet_number,
            expected_revision,
            highest,
            bitmap,
        })
    }

    /// Commit a replay candidate only after a cryptographic owner has
    /// authenticated the MIC for the exact frame which produced the token.
    pub fn commit_authenticated_rx(
        &mut self,
        candidate: EspNowRxReplayCandidate,
    ) -> Result<(), EspNowEncryptedPeerError> {
        let state = self.state_mut(candidate.peer)?;
        if state.replay.revision != candidate.expected_revision {
            self.diagnostics.stale_replay_candidates =
                self.diagnostics.stale_replay_candidates.saturating_add(1);
            return Err(EspNowEncryptedPeerError::StaleReplayCandidate(
                candidate.peer,
            ));
        }
        let Some(revision) = state.replay.revision.checked_add(1) else {
            return Err(EspNowEncryptedPeerError::ReplayRevisionExhausted(
                candidate.peer,
            ));
        };
        state.replay = EspNowReplayWindow {
            highest: Some(candidate.highest),
            bitmap: candidate.bitmap,
            revision,
        };
        self.diagnostics.authenticated_replay_commits = self
            .diagnostics
            .authenticated_replay_commits
            .saturating_add(1);
        Ok(())
    }

    fn state(
        &self,
        peer: EspNowEncryptedPeerId,
    ) -> Result<&EspNowEncryptedPeerState, EspNowEncryptedPeerError> {
        let Some(slot) = self.slots.get(peer.index) else {
            return Err(EspNowEncryptedPeerError::Stale(peer));
        };
        if slot.generation != peer.generation {
            return Err(EspNowEncryptedPeerError::Stale(peer));
        }
        slot.state
            .as_ref()
            .ok_or(EspNowEncryptedPeerError::Stale(peer))
    }

    fn state_mut(
        &mut self,
        peer: EspNowEncryptedPeerId,
    ) -> Result<&mut EspNowEncryptedPeerState, EspNowEncryptedPeerError> {
        let slot = self.slot_mut(peer)?;
        slot.state
            .as_mut()
            .ok_or(EspNowEncryptedPeerError::Stale(peer))
    }

    fn slot_mut(
        &mut self,
        peer: EspNowEncryptedPeerId,
    ) -> Result<&mut EspNowEncryptedPeerSlot, EspNowEncryptedPeerError> {
        let Some(slot) = self.slots.get_mut(peer.index) else {
            return Err(EspNowEncryptedPeerError::Stale(peer));
        };
        if slot.generation != peer.generation {
            return Err(EspNowEncryptedPeerError::Stale(peer));
        }
        Ok(slot)
    }

    fn mutation_failure(
        &mut self,
        error: EspNowEncryptedPeerError,
        config: EspNowEncryptedPeerConfig,
    ) -> EspNowEncryptedPeerMutationFailure {
        self.diagnostics.mutation_rejections =
            self.diagnostics.mutation_rejections.saturating_add(1);
        EspNowEncryptedPeerMutationFailure { error, config }
    }
}

impl<const N: usize> Drop for EspNowEncryptedPeerTable<N> {
    fn drop(&mut self) {
        // `Option::take` drops each LMK through ZeroizeOnDrop before the table
        // storage itself is released.
        for slot in &mut self.slots {
            let _ = slot.state.take();
        }
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
    config: EspNowEncryptedPeerConfig,
    last_tx_packet_number: u64,
    replay: EspNowReplayWindow,
}

impl EspNowRemovedEncryptedPeer {
    pub const fn previous_id(&self) -> EspNowEncryptedPeerId {
        EspNowEncryptedPeerId {
            index: self.index,
            generation: self.generation,
        }
    }

    pub const fn config(&self) -> &EspNowEncryptedPeerConfig {
        &self.config
    }

    /// Finish a successful external removal and return the key-owning config.
    pub fn into_config(self) -> EspNowEncryptedPeerConfig {
        self.config
    }
}

impl fmt::Debug for EspNowRemovedEncryptedPeer {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("EspNowRemovedEncryptedPeer")
            .field("previous_id", &self.previous_id())
            .field("config", &self.config)
            .field("last_tx_packet_number", &self.last_tx_packet_number)
            .field(
                "highest_authenticated_rx_packet_number",
                &self
                    .replay
                    .highest
                    .map(|packet_number| packet_number.value()),
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
        peer: WifiChannel,
        home: WifiChannel,
    },
    GenerationExhausted,
    Stale(EspNowEncryptedPeerId),
    TxPacketNumberExhausted(EspNowEncryptedPeerId),
    ReplayDuplicate {
        peer: EspNowEncryptedPeerId,
        packet_number: EspNowCcmpPacketNumber,
    },
    ReplayTooOld {
        peer: EspNowEncryptedPeerId,
        packet_number: EspNowCcmpPacketNumber,
        highest: EspNowCcmpPacketNumber,
    },
    StaleReplayCandidate(EspNowEncryptedPeerId),
    ReplayRevisionExhausted(EspNowEncryptedPeerId),
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
            Self::ReplayDuplicate {
                peer,
                packet_number,
            } => write!(
                formatter,
                "encrypted ESP-NOW peer {peer:?} repeated PN {}",
                packet_number.value()
            ),
            Self::ReplayTooOld {
                peer,
                packet_number,
                highest,
            } => write!(
                formatter,
                "encrypted ESP-NOW peer {peer:?} PN {} is outside the replay window below {}",
                packet_number.value(),
                highest.value()
            ),
            Self::StaleReplayCandidate(peer) => write!(
                formatter,
                "encrypted ESP-NOW peer {peer:?} replay candidate lost its state race"
            ),
            Self::ReplayRevisionExhausted(peer) => write!(
                formatter,
                "encrypted ESP-NOW peer {peer:?} replay revision is exhausted"
            ),
        }
    }
}

impl core::error::Error for EspNowEncryptedPeerError {}

/// Unauthenticated, non-mutating replay-window proposal.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EspNowRxReplayCandidate {
    peer: EspNowEncryptedPeerId,
    packet_number: EspNowCcmpPacketNumber,
    expected_revision: u32,
    highest: EspNowCcmpPacketNumber,
    bitmap: u64,
}

impl EspNowRxReplayCandidate {
    pub const fn peer(self) -> EspNowEncryptedPeerId {
        self.peer
    }

    pub const fn packet_number(self) -> EspNowCcmpPacketNumber {
        self.packet_number
    }
}

/// Portable owner combining one station binding, PMK and encrypted peers.
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
        sequence: &mut StaSequenceCounter,
        random_value: EspNowRandomValue,
        payload: &'payload [u8],
    ) -> Result<EspNowPreparedEncryptedV1Tx<'payload>, EspNowEncryptedSendError> {
        let payload = EspNowV1Payload::new(payload).map_err(EspNowEncryptedSendError::Wire)?;
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
        })
    }

    /// Parse and address-check protected metadata, then prepare a replay
    /// proposal without mutating the authenticated window.
    pub fn prepare_receive_v1<'frame>(
        &mut self,
        active_station: BoundVirtualInterface,
        active_channel: WifiChannel,
        bytes: &'frame [u8],
    ) -> Result<EspNowEncryptedRxCandidate<'frame>, EspNowEncryptedReceiveError> {
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
    pub fn shutdown(mut self) -> EspNowConfig {
        self.peers.clear_on_shutdown();
        self.config
    }
}

/// Portable encrypted TX metadata. No method exposes the LMK and `encode`
/// always fails at the first unproven interoperable stage.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EspNowPreparedEncryptedV1Tx<'payload> {
    peer: EspNowEncryptedPeerId,
    home_channel: WifiChannel,
    station: BoundVirtualInterface,
    source: EspNowUnicastAddress,
    destination: EspNowUnicastAddress,
    phy_mode: EspNowPhyMode,
    sequence_number: SequenceNumber,
    packet_number: EspNowCcmpPacketNumber,
    random_value: EspNowRandomValue,
    payload: EspNowV1Payload<'payload>,
}

impl<'payload> EspNowPreparedEncryptedV1Tx<'payload> {
    pub const fn peer(self) -> EspNowEncryptedPeerId {
        self.peer
    }

    pub const fn home_channel(self) -> WifiChannel {
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
        ESP_NOW_MANAGEMENT_HEADER_LEN
            + ESP_NOW_CCMP_HEADER_LEN
            + ESP_NOW_V1_ACTION_OVERHEAD
            + self.payload.len()
            + ESP_NOW_CCMP_MIC_LEN
    }

    pub fn encode(self, _output: &mut [u8]) -> Result<usize, EspNowEncryptedV1Unavailable> {
        Err(EspNowEncryptedV1Unavailable::ActionAadContractUnproven)
    }
}

/// Structurally admitted protected frame and its uncommitted replay token.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EspNowEncryptedRxCandidate<'frame> {
    peer: EspNowEncryptedPeerId,
    envelope: EspNowProtectedV1Envelope<'frame>,
    replay: EspNowRxReplayCandidate,
}

impl<'frame> EspNowEncryptedRxCandidate<'frame> {
    pub const fn peer(self) -> EspNowEncryptedPeerId {
        self.peer
    }

    pub const fn envelope(self) -> EspNowProtectedV1Envelope<'frame> {
        self.envelope
    }

    /// Consume the structural candidate into the token which a future exact
    /// decrypt/MIC backend must commit only after successful authentication.
    pub const fn into_replay_candidate(self) -> EspNowRxReplayCandidate {
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
        configured: WifiChannel,
        active: WifiChannel,
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
