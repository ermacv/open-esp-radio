//! Portable ESP-NOW encrypted-peer and anti-replay ownership.
//!
//! This module stops before the first interoperability contract not present
//! in the reviewed repository evidence: the ESP-NOW Action-frame CCMP AAD and
//! the chip key-selector/construct/decrypt callback ABI. It therefore owns
//! secrets and peer generations, but never claims to produce or authenticate
//! encrypted on-air bytes. Each peer's transmit packet numbers and receive
//! replay frontier are the CCMP owners of `oer-ieee80211-upper-mac`.

use crate::extensions::espressif::phy::EspNowPhyMode;
use core::{array, fmt};
use oer_ieee80211_wire::sequence::SequenceNumber;

use zeroize::{Zeroize, ZeroizeOnDrop};
use {
    oer_ieee80211_upper_mac::{
        ccmp::{
            CcmpPacketNumberStep, CcmpReplayCandidate, CcmpReplayFrontier, CcmpReplayed,
            InstalledCcmpTxKey, SoftwareCcmpKey,
        },
        sequence::SequenceCounter,
    },
    oer_ieee80211_wire::{
        ccmp::CcmpPacketNumber,
        channel::WifiChannel,
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
use crate::interface::BoundVirtualInterface;
use oer_ieee80211_port::ChannelContextId;

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
        if self.generation != current.generation {
            return Err(EspNowPmkMutationFailure::new(
                EspNowPmkError::Stale(current),
                replacement,
            ));
        }
        let Some(installed) = self.key.as_mut() else {
            return Err(EspNowPmkMutationFailure::new(
                EspNowPmkError::Stale(current),
                replacement,
            ));
        };
        let Some(generation) = self.generation.checked_add(1) else {
            return Err(EspNowPmkMutationFailure::new(
                EspNowPmkError::GenerationExhausted,
                replacement,
            ));
        };
        let old = core::mem::replace(installed, replacement);
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
    pub channel: WifiChannel,
    pub phy_mode: EspNowPhyMode,
    pub last_tx_packet_number: u64,
    pub highest_authenticated_rx_packet_number: Option<u64>,
}

/// One peer's key, its transmit packet numbers and its receive replay
/// frontier, all from `oer-ieee80211-upper-mac::ccmp`.
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
    pub replays: u32,
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

        if self.slots.iter().any(|slot| {
            slot.state
                .as_ref()
                .is_some_and(|peer| peer.config().destination == config.destination)
        }) {
            return Err(self.mutation_failure(
                EspNowEncryptedPeerError::Duplicate(config.destination),
                config,
            ));
        }
        let Some((index, slot)) = self
            .slots
            .iter_mut()
            .enumerate()
            .find(|(_, slot)| slot.state.is_none() && slot.generation < u32::MAX)
        else {
            let error = if self.slots.iter().any(|slot| slot.state.is_none()) {
                EspNowEncryptedPeerError::GenerationExhausted
            } else {
                EspNowEncryptedPeerError::Full
            };
            return Err(self.mutation_failure(error, config));
        };
        let Some(generation) = slot.generation.checked_add(1) else {
            return Err(
                self.mutation_failure(EspNowEncryptedPeerError::GenerationExhausted, config)
            );
        };
        let Some(length) = self.length.checked_add(1).filter(|length| *length <= N) else {
            return Err(self.mutation_failure(
                EspNowEncryptedPeerError::InvalidCount {
                    count: self.length,
                    capacity: N,
                },
                config,
            ));
        };
        slot.generation = generation;
        slot.state = Some(EspNowEncryptedPeerState {
            key: InstalledCcmpTxKey::install(config, CcmpPacketNumberStep::ONE),
            replay: CcmpReplayFrontier::default(),
        });
        self.length = length;
        self.diagnostics.installs = self.diagnostics.installs.saturating_add(1);
        Ok(EspNowEncryptedPeerId { index, generation })
    }

    /// Atomically rotate or replace a peer. Failure retains the proposed LMK
    /// and leaves the existing key, PN and replay window unchanged.
    pub fn replace(
        &mut self,
        peer: EspNowEncryptedPeerId,
        replacement: EspNowEncryptedPeerConfig,
    ) -> Result<EspNowEncryptedPeerReplacement, EspNowEncryptedPeerMutationFailure> {
        let Some(slot) = self.slots.get_mut(peer.index) else {
            return Err(self.mutation_failure(EspNowEncryptedPeerError::Stale(peer), replacement));
        };
        if slot.generation != peer.generation {
            return Err(self.mutation_failure(EspNowEncryptedPeerError::Stale(peer), replacement));
        }
        let Some(installed) = slot.state.as_mut() else {
            return Err(self.mutation_failure(EspNowEncryptedPeerError::Stale(peer), replacement));
        };
        if replacement.channel != self.home_channel {
            return Err(self.mutation_failure(
                EspNowEncryptedPeerError::ChannelMismatch {
                    peer: replacement.channel,
                    home: self.home_channel,
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
        slot.generation = generation;
        self.diagnostics.replacements = self.diagnostics.replacements.saturating_add(1);
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
        let length = self
            .length
            .checked_sub(1)
            .filter(|length| *length < N)
            .ok_or(EspNowEncryptedPeerError::InvalidCount {
                count: self.length,
                capacity: N,
            })?;
        let slot = self.slot_mut(peer)?;
        let state = slot
            .state
            .take()
            .ok_or(EspNowEncryptedPeerError::Stale(peer))?;
        self.length = length;
        self.diagnostics.removals = self.diagnostics.removals.saturating_add(1);
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
        if removed.config().channel != self.home_channel {
            return Err(EspNowEncryptedPeerRestoreFailure {
                error: EspNowEncryptedPeerError::ChannelMismatch {
                    peer: removed.config().channel,
                    home: self.home_channel,
                },
                removed,
            });
        }
        if self.slots.iter().any(|slot| {
            slot.state
                .as_ref()
                .is_some_and(|state| state.config().destination == removed.config().destination)
        }) {
            return Err(EspNowEncryptedPeerRestoreFailure {
                error: EspNowEncryptedPeerError::Duplicate(removed.config().destination),
                removed,
            });
        }

        let Some(length) = self.length.checked_add(1).filter(|length| *length <= N) else {
            return Err(EspNowEncryptedPeerRestoreFailure {
                error: EspNowEncryptedPeerError::InvalidCount {
                    count: self.length,
                    capacity: N,
                },
                removed,
            });
        };
        let Some(slot) = self.slots.get_mut(removed.index) else {
            return Err(EspNowEncryptedPeerRestoreFailure {
                error: EspNowEncryptedPeerError::Stale(removed.previous_id()),
                removed,
            });
        };
        let EspNowRemovedEncryptedPeer {
            index,
            generation: _,
            state,
        } = removed;
        *slot = EspNowEncryptedPeerSlot {
            generation,
            state: Some(state),
        };
        self.length = length;
        self.diagnostics.remove_rollbacks = self.diagnostics.remove_rollbacks.saturating_add(1);
        Ok(EspNowEncryptedPeerId { index, generation })
    }

    /// Drop and zeroize every LMK. Existing handles become stale immediately;
    /// a later install advances each reused slot generation.
    pub fn clear_on_shutdown(&mut self) -> usize {
        let cleared = self
            .slots
            .iter_mut()
            .filter_map(|slot| slot.state.take())
            .count();
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
                    .is_some_and(|state| state.config().destination == destination)
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
            self.diagnostics.tx_packet_number_exhaustions = self
                .diagnostics
                .tx_packet_number_exhaustions
                .saturating_add(1);
            return Err(EspNowEncryptedPeerError::TxPacketNumberExhausted(peer));
        };
        self.diagnostics.tx_packet_numbers_allocated = self
            .diagnostics
            .tx_packet_numbers_allocated
            .saturating_add(1);
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
        let Self {
            slots, diagnostics, ..
        } = self;
        let state = slots
            .get_mut(peer.index)
            .filter(|slot| slot.generation == peer.generation)
            .and_then(|slot| slot.state.as_mut())
            .ok_or(EspNowEncryptedPeerError::Stale(peer))?;
        match state.replay.prepare(CcmpPacketNumber::from(packet_number)) {
            Ok(replay) => {
                diagnostics.replay_candidates = diagnostics.replay_candidates.saturating_add(1);
                Ok(EspNowRxReplayCandidate {
                    peer,
                    packet_number,
                    replay,
                    commits: &mut diagnostics.authenticated_replay_commits,
                })
            }
            Err(CcmpReplayed { highest, .. }) => {
                diagnostics.replays = diagnostics.replays.saturating_add(1);
                Err(EspNowEncryptedPeerError::Replay {
                    peer,
                    packet_number,
                    highest,
                })
            }
        }
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
    InvalidCount {
        count: usize,
        capacity: usize,
    },
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
            Self::InvalidCount { count, capacity } => write!(
                formatter,
                "encrypted ESP-NOW peer count {count} is inconsistent with capacity {capacity}"
            ),
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
#[derive(Debug)]
#[must_use = "commit after authentication, or drop to cancel admission"]
pub struct EspNowRxReplayCandidate<'a> {
    peer: EspNowEncryptedPeerId,
    packet_number: EspNowCcmpPacketNumber,
    replay: CcmpReplayCandidate<'a>,
    commits: &'a mut u32,
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
        *self.commits = self.commits.saturating_add(1);
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
        active_channel: WifiChannel,
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
    pub fn shutdown(mut self) -> EspNowConfig {
        self.peers.clear_on_shutdown();
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
    home_channel: WifiChannel,
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
        self.encoded_len
    }

    pub fn encode(self, _output: &mut [u8]) -> Result<usize, EspNowEncryptedV1Unavailable> {
        Err(EspNowEncryptedV1Unavailable::ActionAadContractUnproven)
    }
}

/// Structurally admitted protected frame and its uncommitted replay token.
#[derive(Debug)]
pub struct EspNowEncryptedRxCandidate<'a, 'frame> {
    peer: EspNowEncryptedPeerId,
    envelope: EspNowProtectedV1Envelope<'frame>,
    replay: EspNowRxReplayCandidate<'a>,
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

#[cfg(test)]
#[allow(clippy::disallowed_macros)]
mod tests;
