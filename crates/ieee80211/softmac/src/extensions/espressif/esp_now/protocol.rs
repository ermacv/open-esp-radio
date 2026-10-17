//! Portable ESP-NOW protocol ownership for plaintext v1 and v2 profiles.
//!
//! This module binds ESP-NOW to an existing station VIF and its one home
//! channel. It owns peer admission and frame construction, but deliberately
//! does not claim receive-filter, key-slot, PHY-low-rate or DMA authority.

use core::fmt;
use oer_ieee80211_mac::sequence::SequenceNumber;

use oer_ieee80211_mac::{
    channel::WifiChannel,
    extensions::espressif::esp_now::{
        ESP_NOW_V1_MAX_PAYLOAD_LEN, ESP_NOW_V2_MAX_PAYLOAD_LEN, EspNowDestination,
        EspNowRandomValue, EspNowUnicastAddress, EspNowV1Frame, EspNowV1Payload, EspNowV1WireError,
        EspNowV2Frame, EspNowV2Payload, EspNowV2WireError,
    },
    station::StaSequenceCounter,
};

use crate::interface::{BoundVirtualInterface, ChannelContextId, VifRole};

/// Default source-owned peer storage. The const-generic table remains usable
/// with a smaller application-selected capacity.
pub const ESP_NOW_DEFAULT_PEER_CAPACITY: usize = 20;
/// Recent exact fingerprints retained independently for each RX peer.
pub const ESP_NOW_RX_DUPLICATE_HISTORY_CAPACITY: usize = 4;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct EspNowRxFingerprint {
    random_value: EspNowRandomValue,
    sequence_number: SequenceNumber,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct EspNowRxPeerSlot {
    peer: Option<(
        EspNowPeerId,
        EspNowUnicastAddress,
        EspNowPeerCapability,
        EspNowPhyMode,
    )>,
    history: [Option<EspNowRxFingerprint>; ESP_NOW_RX_DUPLICATE_HISTORY_CAPACITY],
    next_history: usize,
}

impl EspNowRxPeerSlot {
    const EMPTY: Self = Self {
        peer: None,
        history: [None; ESP_NOW_RX_DUPLICATE_HISTORY_CAPACITY],
        next_history: 0,
    };
}

/// One standard OFDM rate explicitly selected for an ESP-NOW peer.
///
/// The values are kept portable instead of exposing a chip MAC rate byte.
/// A backend may support only a subset, but must reject an unsupported value
/// before publishing a frame.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EspNowOfdmRate {
    Mbps6,
    Mbps9,
    Mbps12,
    Mbps18,
    Mbps24,
    Mbps36,
    Mbps48,
    Mbps54,
}

/// One-spatial-stream MCS for the standard ESP-NOW HT20 profile.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EspNowHtMcs {
    Mcs0,
    Mcs1,
    Mcs2,
    Mcs3,
    Mcs4,
    Mcs5,
    Mcs6,
    Mcs7,
}

impl EspNowHtMcs {
    pub const fn index(self) -> u8 {
        match self {
            Self::Mcs0 => 0,
            Self::Mcs1 => 1,
            Self::Mcs2 => 2,
            Self::Mcs3 => 3,
            Self::Mcs4 => 4,
            Self::Mcs5 => 5,
            Self::Mcs6 => 6,
            Self::Mcs7 => 7,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EspNowHtGuardInterval {
    Long800Ns,
    Short400Ns,
}

/// A complete standard HT20 rate requested for one ESP-NOW peer.
///
/// MCS32 is deliberately absent: duplicate mode has a distinct hardware
/// certification boundary and cannot be represented as a one-stream MCS.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EspNowHt20Rate {
    mcs: EspNowHtMcs,
    guard_interval: EspNowHtGuardInterval,
}

impl EspNowHt20Rate {
    pub const fn new(mcs: EspNowHtMcs, guard_interval: EspNowHtGuardInterval) -> Self {
        Self {
            mcs,
            guard_interval,
        }
    }

    pub const fn mcs(self) -> EspNowHtMcs {
        self.mcs
    }

    pub const fn guard_interval(self) -> EspNowHtGuardInterval {
        self.guard_interval
    }
}

/// PHY policy requested for one peer.
///
/// The two `StandardP2p*` variants use the dedicated P2P retry arenas on
/// backends which own them. A backend must reject `LongRange` unless it owns
/// the complete LR enable, PLCP, rate and receive-status contract. Merely
/// possessing recovered LR rate schedule bytes is not sufficient.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum EspNowPhyMode {
    #[default]
    LegacyDsss1M,
    StandardP2pOfdm(EspNowOfdmRate),
    StandardP2pHt20(EspNowHt20Rate),
    LongRange(crate::EspressifLongRangeRate),
}

/// Security profile of one typed ESP-NOW handoff.
///
/// Encrypted peers are owned by the separate zeroizing security protocol;
/// this enum carries no key bytes or hardware key selector.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EspNowPeerSecurity {
    Plaintext,
    Encrypted,
}

/// Wire-version capability explicitly asserted for one plaintext peer.
///
/// A v2-capable device can also receive v1. Keeping the default at
/// [`Self::V1Only`] makes existing unicast and broadcast configurations safe
/// for mixed-version networks. Marking a broadcast destination v2-capable is
/// the caller's assertion that every intended receiver supports v2.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum EspNowPeerCapability {
    #[default]
    V1Only,
    V2Capable,
}

impl EspNowPeerCapability {
    pub const fn supports_v2(self) -> bool {
        matches!(self, Self::V2Capable)
    }
}

/// One ESP-NOW service bound to a station interface and one tuned channel.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EspNowConfig {
    station: BoundVirtualInterface,
    local_address: EspNowUnicastAddress,
    home_channel: WifiChannel,
}

impl EspNowConfig {
    pub fn new(
        station: BoundVirtualInterface,
        home_channel: WifiChannel,
    ) -> Result<Self, EspNowConfigError> {
        if station.interface.role != VifRole::Station {
            return Err(EspNowConfigError::RequiresStationInterface);
        }
        let local_address = EspNowUnicastAddress::new(station.interface.address)
            .map_err(|_| EspNowConfigError::InvalidStationAddress)?;
        Ok(Self {
            station,
            local_address,
            home_channel,
        })
    }

    pub const fn station(self) -> BoundVirtualInterface {
        self.station
    }

    pub const fn local_address(self) -> EspNowUnicastAddress {
        self.local_address
    }

    pub const fn home_channel(self) -> WifiChannel {
        self.home_channel
    }

    pub const fn channel_context(self) -> ChannelContextId {
        self.station.channel_context
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EspNowConfigError {
    RequiresStationInterface,
    InvalidStationAddress,
}

impl fmt::Display for EspNowConfigError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::RequiresStationInterface => {
                "ESP-NOW currently requires an existing station virtual interface"
            }
            Self::InvalidStationAddress => {
                "ESP-NOW requires a valid individual station MAC address"
            }
        })
    }
}

impl core::error::Error for EspNowConfigError {}

/// Plaintext configuration for one broadcast or individual peer.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EspNowPeerConfig {
    destination: EspNowDestination,
    channel: EspNowPeerChannelPolicy,
    phy_mode: EspNowPhyMode,
    capability: EspNowPeerCapability,
}

/// Explicit channel authority for one plaintext peer.
///
/// `HomeChannel` is accepted by every ESP-NOW composition and is checked
/// against the protocol owner's home channel when the peer is installed.
/// `StandaloneFixed` is the only portable opt-in to an off-channel send. It
/// names one exact peer channel; it never scans or falls back to the home
/// channel, and connected runtimes must reject it before consuming a sequence
/// number.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EspNowPeerChannelPolicy {
    HomeChannel(WifiChannel),
    StandaloneFixed(WifiChannel),
}

impl EspNowPeerChannelPolicy {
    pub const fn channel(self) -> WifiChannel {
        match self {
            Self::HomeChannel(channel) | Self::StandaloneFixed(channel) => channel,
        }
    }

    pub const fn is_home_channel(self) -> bool {
        matches!(self, Self::HomeChannel(_))
    }
}

impl EspNowPeerConfig {
    /// Configure a peer on the service's home channel using the supported
    /// standard one-megabit Action-frame profile.
    pub const fn plaintext(destination: EspNowDestination, channel: WifiChannel) -> Self {
        Self {
            destination,
            channel: EspNowPeerChannelPolicy::HomeChannel(channel),
            phy_mode: EspNowPhyMode::LegacyDsss1M,
            capability: EspNowPeerCapability::V1Only,
        }
    }

    /// Configure one exact peer channel for a standalone runtime. This is a
    /// deliberate off-channel policy, not an automatic fallback. The peer
    /// table rejects it when `channel` is the protocol home channel.
    pub const fn plaintext_off_channel(
        destination: EspNowDestination,
        channel: WifiChannel,
    ) -> Self {
        Self {
            destination,
            channel: EspNowPeerChannelPolicy::StandaloneFixed(channel),
            phy_mode: EspNowPhyMode::LegacyDsss1M,
            capability: EspNowPeerCapability::V1Only,
        }
    }

    /// Configure an explicitly v2-capable peer. The same peer remains valid
    /// for v1 sends because v2 receivers are backwards-compatible.
    pub const fn plaintext_v2(destination: EspNowDestination, channel: WifiChannel) -> Self {
        Self::plaintext(destination, channel).with_capability(EspNowPeerCapability::V2Capable)
    }

    /// Configure one explicitly v2-capable peer on one exact standalone-only
    /// off-channel.
    pub const fn plaintext_v2_off_channel(
        destination: EspNowDestination,
        channel: WifiChannel,
    ) -> Self {
        Self::plaintext_off_channel(destination, channel)
            .with_capability(EspNowPeerCapability::V2Capable)
    }

    /// Request a typed PHY mode. Unsupported chip backends must fail before
    /// publishing the frame rather than lowering LR to an ordinary rate code.
    pub const fn with_phy_mode(mut self, phy_mode: EspNowPhyMode) -> Self {
        self.phy_mode = phy_mode;
        self
    }

    /// Set the peer's asserted wire-version capability.
    pub const fn with_capability(mut self, capability: EspNowPeerCapability) -> Self {
        self.capability = capability;
        self
    }

    pub const fn destination(self) -> EspNowDestination {
        self.destination
    }

    pub const fn channel(self) -> WifiChannel {
        self.channel.channel()
    }

    pub const fn channel_policy(self) -> EspNowPeerChannelPolicy {
        self.channel
    }

    pub const fn phy_mode(self) -> EspNowPhyMode {
        self.phy_mode
    }

    pub const fn capability(self) -> EspNowPeerCapability {
        self.capability
    }

    pub const fn security(self) -> EspNowPeerSecurity {
        EspNowPeerSecurity::Plaintext
    }
}

/// Generation-checked identity of one peer-table slot.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EspNowPeerId {
    index: usize,
    generation: u32,
}

impl EspNowPeerId {
    pub const fn index(self) -> usize {
        self.index
    }

    pub const fn generation(self) -> u32 {
        self.generation
    }
}

#[derive(Clone, Copy)]
struct EspNowPeerSlot {
    generation: u32,
    config: Option<EspNowPeerConfig>,
}

impl EspNowPeerSlot {
    const EMPTY: Self = Self {
        generation: 0,
        config: None,
    };
}

/// Fixed-capacity plaintext peer owner with explicit per-peer channel policy.
pub struct EspNowPeerTable<const N: usize = ESP_NOW_DEFAULT_PEER_CAPACITY> {
    home_channel: WifiChannel,
    slots: [EspNowPeerSlot; N],
    length: usize,
}

impl<const N: usize> EspNowPeerTable<N> {
    pub const fn new(home_channel: WifiChannel) -> Self {
        Self {
            home_channel,
            slots: [EspNowPeerSlot::EMPTY; N],
            length: 0,
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

    pub fn add(&mut self, config: EspNowPeerConfig) -> Result<EspNowPeerId, EspNowPeerTableError> {
        match config.channel {
            EspNowPeerChannelPolicy::HomeChannel(peer) if peer != self.home_channel => {
                return Err(EspNowPeerTableError::ChannelMismatch {
                    peer,
                    home: self.home_channel,
                });
            }
            EspNowPeerChannelPolicy::StandaloneFixed(peer) if peer == self.home_channel => {
                return Err(EspNowPeerTableError::OffChannelMatchesHome(peer));
            }
            EspNowPeerChannelPolicy::HomeChannel(_)
            | EspNowPeerChannelPolicy::StandaloneFixed(_) => {}
        }

        let mut reusable = None;
        let mut exhausted_slot = false;
        for (index, slot) in self.slots.iter().enumerate() {
            if slot
                .config
                .is_some_and(|peer| peer.destination == config.destination)
            {
                return Err(EspNowPeerTableError::Duplicate(config.destination));
            }
            if slot.config.is_none() && reusable.is_none() {
                if slot.generation == u32::MAX {
                    exhausted_slot = true;
                } else {
                    reusable = Some(index);
                }
            }
        }

        let Some(index) = reusable else {
            return Err(if exhausted_slot && self.length < N {
                EspNowPeerTableError::GenerationExhausted
            } else {
                EspNowPeerTableError::Full
            });
        };
        let slot = &mut self.slots[index];
        slot.generation += 1;
        slot.config = Some(config);
        self.length += 1;
        Ok(EspNowPeerId {
            index,
            generation: slot.generation,
        })
    }

    pub fn get(&self, peer: EspNowPeerId) -> Result<EspNowPeerConfig, EspNowPeerTableError> {
        let Some(slot) = self.slots.get(peer.index) else {
            return Err(EspNowPeerTableError::Stale(peer));
        };
        if slot.generation != peer.generation {
            return Err(EspNowPeerTableError::Stale(peer));
        }
        slot.config.ok_or(EspNowPeerTableError::Stale(peer))
    }

    pub fn find(&self, destination: EspNowDestination) -> Option<EspNowPeerId> {
        self.slots
            .iter()
            .enumerate()
            .find(|(_, slot)| {
                slot.config
                    .is_some_and(|peer| peer.destination == destination)
            })
            .map(|(index, slot)| EspNowPeerId {
                index,
                generation: slot.generation,
            })
    }

    pub fn remove(&mut self, peer: EspNowPeerId) -> Result<EspNowPeerConfig, EspNowPeerTableError> {
        let Some(slot) = self.slots.get_mut(peer.index) else {
            return Err(EspNowPeerTableError::Stale(peer));
        };
        if slot.generation != peer.generation {
            return Err(EspNowPeerTableError::Stale(peer));
        }
        let Some(config) = slot.config.take() else {
            return Err(EspNowPeerTableError::Stale(peer));
        };
        self.length -= 1;
        Ok(config)
    }

    pub const fn iter(&self) -> EspNowPeers<'_, N> {
        EspNowPeers {
            table: self,
            next: 0,
        }
    }
}

/// Borrowed iterator over occupied peer slots.
pub struct EspNowPeers<'table, const N: usize> {
    table: &'table EspNowPeerTable<N>,
    next: usize,
}

impl<const N: usize> Iterator for EspNowPeers<'_, N> {
    type Item = (EspNowPeerId, EspNowPeerConfig);

    fn next(&mut self) -> Option<Self::Item> {
        while self.next < N {
            let index = self.next;
            self.next += 1;
            let slot = self.table.slots[index];
            if let Some(config) = slot.config {
                return Some((
                    EspNowPeerId {
                        index,
                        generation: slot.generation,
                    },
                    config,
                ));
            }
        }
        None
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        (0, Some(N.saturating_sub(self.next)))
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EspNowPeerTableError {
    Full,
    Duplicate(EspNowDestination),
    ChannelMismatch {
        peer: WifiChannel,
        home: WifiChannel,
    },
    OffChannelMatchesHome(WifiChannel),
    GenerationExhausted,
    Stale(EspNowPeerId),
}

impl fmt::Display for EspNowPeerTableError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Full => formatter.write_str("the ESP-NOW peer table is full"),
            Self::Duplicate(destination) => {
                write!(
                    formatter,
                    "ESP-NOW peer {destination:?} is already configured"
                )
            }
            Self::ChannelMismatch { peer, home } => write!(
                formatter,
                "ESP-NOW peer channel {peer:?} differs from home channel {home:?}"
            ),
            Self::OffChannelMatchesHome(channel) => write!(
                formatter,
                "ESP-NOW standalone off-channel policy names home channel {channel:?}"
            ),
            Self::GenerationExhausted => {
                formatter.write_str("an ESP-NOW peer generation counter is exhausted")
            }
            Self::Stale(peer) => write!(formatter, "ESP-NOW peer handle {peer:?} is stale"),
        }
    }
}

impl core::error::Error for EspNowPeerTableError {}

/// Portable ESP-NOW protocol owner. Sequence ownership remains explicit so a
/// station service can share its existing non-QoS sequence space correctly.
pub struct EspNowProtocol<const N: usize = ESP_NOW_DEFAULT_PEER_CAPACITY> {
    config: EspNowConfig,
    peers: EspNowPeerTable<N>,
}

impl<const N: usize> EspNowProtocol<N> {
    pub const fn new(config: EspNowConfig) -> Self {
        Self {
            config,
            peers: EspNowPeerTable::new(config.home_channel),
        }
    }

    pub const fn config(&self) -> EspNowConfig {
        self.config
    }

    pub const fn peers(&self) -> &EspNowPeerTable<N> {
        &self.peers
    }

    pub fn add_peer(
        &mut self,
        peer: EspNowPeerConfig,
    ) -> Result<EspNowPeerId, EspNowPeerTableError> {
        self.peers.add(peer)
    }

    pub fn remove_peer(
        &mut self,
        peer: EspNowPeerId,
    ) -> Result<EspNowPeerConfig, EspNowPeerTableError> {
        self.peers.remove(peer)
    }

    /// Snapshot the configured individual peers into one exclusive normal-RX
    /// epoch.
    ///
    /// The caller must supply the station/channel owners which are live at
    /// this transition. This keeps a monitor observation or a stale peer
    /// table from manufacturing receive authority. Duplicate history belongs
    /// to the returned value and therefore cannot leak across stop/restart.
    pub fn begin_rx_epoch(
        &self,
        active_station: BoundVirtualInterface,
        active_channel: WifiChannel,
    ) -> Result<EspNowRxEpoch<N>, EspNowReceiveError> {
        validate_active_binding(self.config, active_station, active_channel)?;

        let mut slots = [EspNowRxPeerSlot::EMPTY; N];
        let mut peer_count = 0;
        for (peer, config) in self.peers.iter() {
            if !config.channel_policy().is_home_channel() {
                // Standalone fixed-channel peers grant TX authority only. The
                // normal receive epoch is deliberately rebuilt on home.
                continue;
            }
            let EspNowDestination::Unicast(address) = config.destination else {
                // A broadcast entry grants only broadcast TX authority.
                continue;
            };
            slots[peer.index] = EspNowRxPeerSlot {
                peer: Some((peer, address, config.capability, config.phy_mode)),
                history: [None; ESP_NOW_RX_DUPLICATE_HISTORY_CAPACITY],
                next_history: 0,
            };
            peer_count += 1;
        }
        Ok(EspNowRxEpoch {
            config: self.config,
            slots,
            peer_count,
        })
    }

    /// Prove that an independently moved receive epoch is the current peer
    /// snapshot produced by this protocol owner.
    ///
    /// Runtime compositions use this before touching RX policy so an epoch
    /// from another request cannot admit a peer which is not registered in
    /// the active standalone service.
    pub fn owns_rx_epoch(&self, epoch: &EspNowRxEpoch<N>) -> bool {
        if epoch.config != self.config {
            return false;
        }
        let mut peer_count = 0;
        for (index, peer_slot) in self.peers.slots.iter().enumerate() {
            let expected = match peer_slot.config {
                Some(EspNowPeerConfig {
                    destination: EspNowDestination::Unicast(address),
                    channel: EspNowPeerChannelPolicy::HomeChannel(_),
                    capability,
                    phy_mode,
                    ..
                }) => {
                    peer_count += 1;
                    Some((
                        EspNowPeerId {
                            index,
                            generation: peer_slot.generation,
                        },
                        address,
                        capability,
                        phy_mode,
                    ))
                }
                Some(_) | None => None,
            };
            if epoch.slots[index].peer != expected {
                return false;
            }
        }
        epoch.peer_count == peer_count
    }

    /// Resolve a peer, validate the payload, then consume one shared station
    /// management/non-QoS sequence number and return a backend handoff.
    pub fn prepare_v1_tx<'payload>(
        &self,
        peer: EspNowPeerId,
        sequence: &mut StaSequenceCounter,
        random_value: EspNowRandomValue,
        payload: &'payload [u8],
    ) -> Result<EspNowPreparedV1Tx<'payload>, EspNowSendError> {
        let peer_config = self.peers.get(peer).map_err(EspNowSendError::Peer)?;
        // Validate before advancing the shared sequence space.
        EspNowV1Payload::new(payload).map_err(EspNowSendError::Wire)?;
        let frame = EspNowV1Frame::new(
            peer_config.destination,
            self.config.local_address,
            sequence.take(),
            random_value,
            payload,
        )
        .map_err(EspNowSendError::Wire)?;
        Ok(EspNowPreparedV1Tx {
            peer,
            home_channel: self.config.home_channel,
            channel_policy: peer_config.channel,
            station: self.config.station,
            phy_mode: peer_config.phy_mode,
            frame,
        })
    }

    /// Resolve an explicitly v2-capable peer and construct a portable v2
    /// handoff. No chip/runtime transmit support is implied by this value.
    ///
    /// Capability and payload validation happen before the shared station
    /// sequence counter advances.
    pub fn prepare_v2_tx<'payload>(
        &self,
        peer: EspNowPeerId,
        sequence: &mut StaSequenceCounter,
        random_value: EspNowRandomValue,
        payload: &'payload [u8],
    ) -> Result<EspNowPreparedV2Tx<'payload>, EspNowV2SendError> {
        let peer_config = self.peers.get(peer).map_err(EspNowV2SendError::Peer)?;
        if !peer_config.capability.supports_v2() {
            return Err(EspNowV2SendError::PeerNotV2Capable {
                peer,
                capability: peer_config.capability,
            });
        }
        EspNowV2Payload::new(payload).map_err(EspNowV2SendError::Wire)?;
        let frame = EspNowV2Frame::new(
            peer_config.destination,
            self.config.local_address,
            sequence.take(),
            random_value,
            payload,
        )
        .map_err(EspNowV2SendError::Wire)?;
        Ok(EspNowPreparedV2Tx {
            peer,
            home_channel: self.config.home_channel,
            channel_policy: peer_config.channel,
            station: self.config.station,
            phy_mode: peer_config.phy_mode,
            frame,
        })
    }

    /// Parse and admit a plaintext frame addressed to this service from one
    /// configured individual peer. A configured broadcast destination grants
    /// TX authority only; it never acts as a wildcard RX peer. The active
    /// binding and channel must come from the exclusive normal RX owner; a
    /// passive monitor observation is not sufficient authority.
    pub fn receive_v1<'frame>(
        &self,
        active_station: BoundVirtualInterface,
        active_channel: WifiChannel,
        bytes: &'frame [u8],
    ) -> Result<EspNowReceivedV1<'frame>, EspNowReceiveError> {
        validate_active_binding(self.config, active_station, active_channel)?;
        let frame = EspNowV1Frame::parse(bytes).map_err(EspNowReceiveError::Wire)?;
        if !matches!(frame.destination(), EspNowDestination::Broadcast)
            && frame.destination() != EspNowDestination::Unicast(self.config.local_address)
        {
            return Err(EspNowReceiveError::ForeignDestination(frame.destination()));
        }
        let source = frame.source();
        let Some(peer) = self.peers.find(EspNowDestination::Unicast(source)) else {
            return Err(EspNowReceiveError::UnknownPeer(source));
        };
        if !self
            .peers
            .get(peer)
            .is_ok_and(|config| config.channel_policy().is_home_channel())
        {
            return Err(EspNowReceiveError::UnknownPeer(source));
        }
        Ok(EspNowReceivedV1 { peer, frame })
    }

    /// Structurally parse and admit one portable plaintext v2 frame from an
    /// explicitly v2-capable individual peer. This direct API does not perform
    /// the duplicate suppression owned by a live runtime RX epoch.
    pub fn receive_v2<'frame>(
        &self,
        active_station: BoundVirtualInterface,
        active_channel: WifiChannel,
        bytes: &'frame [u8],
    ) -> Result<EspNowReceivedV2<'frame>, EspNowV2ReceiveError> {
        if active_station != self.config.station {
            return Err(EspNowV2ReceiveError::StationBindingMismatch {
                configured: self.config.station,
                active: active_station,
            });
        }
        if active_channel != self.config.home_channel {
            return Err(EspNowV2ReceiveError::ChannelMismatch {
                configured: self.config.home_channel,
                active: active_channel,
            });
        }
        let frame = EspNowV2Frame::parse(bytes).map_err(EspNowV2ReceiveError::Wire)?;
        if !matches!(frame.destination(), EspNowDestination::Broadcast)
            && frame.destination() != EspNowDestination::Unicast(self.config.local_address)
        {
            return Err(EspNowV2ReceiveError::ForeignDestination(
                frame.destination(),
            ));
        }
        let source = frame.source();
        let Some(peer) = self.peers.find(EspNowDestination::Unicast(source)) else {
            return Err(EspNowV2ReceiveError::UnknownPeer(source));
        };
        let peer_config = self.peers.get(peer).map_err(EspNowV2ReceiveError::Peer)?;
        if !peer_config.channel_policy().is_home_channel() {
            return Err(EspNowV2ReceiveError::UnknownPeer(source));
        }
        if !peer_config.capability.supports_v2() {
            return Err(EspNowV2ReceiveError::PeerNotV2Capable {
                peer,
                capability: peer_config.capability,
            });
        }
        Ok(EspNowReceivedV2 { peer, frame })
    }
}

fn validate_active_binding(
    config: EspNowConfig,
    active_station: BoundVirtualInterface,
    active_channel: WifiChannel,
) -> Result<(), EspNowReceiveError> {
    if active_station != config.station {
        return Err(EspNowReceiveError::StationBindingMismatch {
            configured: config.station,
            active: active_station,
        });
    }
    if active_channel != config.home_channel {
        return Err(EspNowReceiveError::ChannelMismatch {
            configured: config.home_channel,
            active: active_channel,
        });
    }
    Ok(())
}

/// Peer snapshot and duplicate history owned by one normal receive epoch.
///
/// Dropping this value ends all receive authority. A later epoch must be
/// created again from [`EspNowProtocol::begin_rx_epoch`], which starts with
/// empty duplicate history and the then-current peer generation values.
#[derive(Debug, Eq, PartialEq)]
pub struct EspNowRxEpoch<const N: usize = ESP_NOW_DEFAULT_PEER_CAPACITY> {
    config: EspNowConfig,
    slots: [EspNowRxPeerSlot; N],
    peer_count: usize,
}

impl<const N: usize> EspNowRxEpoch<N> {
    pub const fn config(&self) -> EspNowConfig {
        self.config
    }

    pub const fn peer_count(&self) -> usize {
        self.peer_count
    }

    /// Return the configured PHY identity retained beside one admitted peer.
    ///
    /// Backends use this only to normalize receive evidence. It does not prove
    /// that the observed PPDU used the peer's preferred transmit rate.
    pub fn peer_phy_mode(&self, peer: EspNowPeerId) -> Option<EspNowPhyMode> {
        self.slots.get(peer.index).and_then(|slot| {
            slot.peer
                .and_then(|(configured, _, _, phy_mode)| (configured == peer).then_some(phy_mode))
        })
    }

    /// Parse, address-check and admit one complete plaintext v1 MPDU.
    ///
    /// Duplicate identity intentionally combines the configured source with
    /// both on-air collision domains: the opaque random value and the
    /// management sequence number. Suppression happens before any borrowed
    /// payload is published to an integration callback.
    pub fn receive_v1<'frame>(
        &mut self,
        bytes: &'frame [u8],
    ) -> Result<EspNowRxOutcome<'frame>, EspNowReceiveError> {
        let frame = EspNowV1Frame::parse(bytes).map_err(EspNowReceiveError::Wire)?;
        if !matches!(frame.destination(), EspNowDestination::Broadcast)
            && frame.destination() != EspNowDestination::Unicast(self.config.local_address)
        {
            return Err(EspNowReceiveError::ForeignDestination(frame.destination()));
        }

        let source = frame.source();
        let Some(slot) = self.slots.iter_mut().find(|slot| {
            slot.peer
                .is_some_and(|(_, configured_source, _, _)| configured_source == source)
        }) else {
            return Err(EspNowReceiveError::UnknownPeer(source));
        };
        let (peer, _, _, _) = slot
            .peer
            .expect("an ESP-NOW RX slot selected by source is occupied");
        let fingerprint = EspNowRxFingerprint {
            random_value: frame.action().random_value(),
            sequence_number: frame.sequence_number(),
        };
        if slot.history.contains(&Some(fingerprint)) {
            return Ok(EspNowRxOutcome::Duplicate { peer });
        }
        slot.history[slot.next_history] = Some(fingerprint);
        slot.next_history = (slot.next_history + 1) % ESP_NOW_RX_DUPLICATE_HISTORY_CAPACITY;
        Ok(EspNowRxOutcome::Received(EspNowReceivedV1 { peer, frame }))
    }

    /// Parse, capability-check and admit one complete plaintext v2 MPDU.
    ///
    /// The same per-peer random-value/sequence fingerprint history is shared
    /// with v1, so a live epoch cannot publish a duplicate merely because the
    /// sender changed its wire version. Capability is checked before the
    /// fingerprint is committed.
    pub fn receive_v2<'frame>(
        &mut self,
        bytes: &'frame [u8],
    ) -> Result<EspNowV2RxOutcome<'frame>, EspNowV2ReceiveError> {
        let frame = EspNowV2Frame::parse(bytes).map_err(EspNowV2ReceiveError::Wire)?;
        if !matches!(frame.destination(), EspNowDestination::Broadcast)
            && frame.destination() != EspNowDestination::Unicast(self.config.local_address)
        {
            return Err(EspNowV2ReceiveError::ForeignDestination(
                frame.destination(),
            ));
        }

        let source = frame.source();
        let Some(slot) = self.slots.iter_mut().find(|slot| {
            slot.peer
                .is_some_and(|(_, configured_source, _, _)| configured_source == source)
        }) else {
            return Err(EspNowV2ReceiveError::UnknownPeer(source));
        };
        let (peer, _, capability, _) = slot
            .peer
            .expect("an ESP-NOW RX slot selected by source is occupied");
        if !capability.supports_v2() {
            return Err(EspNowV2ReceiveError::PeerNotV2Capable { peer, capability });
        }
        let fingerprint = EspNowRxFingerprint {
            random_value: frame.action().random_value(),
            sequence_number: frame.sequence_number(),
        };
        if slot.history.contains(&Some(fingerprint)) {
            return Ok(EspNowV2RxOutcome::Duplicate { peer });
        }
        slot.history[slot.next_history] = Some(fingerprint);
        slot.next_history = (slot.next_history + 1) % ESP_NOW_RX_DUPLICATE_HISTORY_CAPACITY;
        Ok(EspNowV2RxOutcome::Received(EspNowReceivedV2 {
            peer,
            frame,
        }))
    }

    /// Clear duplicate state before the surrounding normal-RX owner is
    /// returned to a lifecycle composition root.
    pub fn reset_duplicate_history(&mut self) -> usize {
        let mut cleared = 0;
        for slot in &mut self.slots {
            for fingerprint in &mut slot.history {
                if fingerprint.take().is_some() {
                    cleared += 1;
                }
            }
            slot.next_history = 0;
        }
        cleared
    }
}

/// Result of ESP-NOW admission before integration publication.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EspNowRxOutcome<'frame> {
    Received(EspNowReceivedV1<'frame>),
    Duplicate { peer: EspNowPeerId },
}

/// Result of live v2 admission before an integration copies the payload.
#[derive(Clone, Copy, Debug)]
pub enum EspNowV2RxOutcome<'frame> {
    Received(EspNowReceivedV2<'frame>),
    Duplicate { peer: EspNowPeerId },
}

/// Fully validated plaintext MPDU handoff to one channel-bound backend.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EspNowPreparedV1Tx<'payload> {
    peer: EspNowPeerId,
    home_channel: WifiChannel,
    channel_policy: EspNowPeerChannelPolicy,
    station: BoundVirtualInterface,
    phy_mode: EspNowPhyMode,
    frame: EspNowV1Frame<'payload>,
}

impl EspNowPreparedV1Tx<'_> {
    pub const fn peer(self) -> EspNowPeerId {
        self.peer
    }

    pub const fn destination(self) -> EspNowDestination {
        self.frame.destination()
    }

    pub const fn home_channel(self) -> WifiChannel {
        self.home_channel
    }

    /// Exact channel on which the backend may publish this frame.
    pub const fn transmit_channel(self) -> WifiChannel {
        self.channel_policy.channel()
    }

    pub const fn channel_policy(self) -> EspNowPeerChannelPolicy {
        self.channel_policy
    }

    pub const fn station(self) -> BoundVirtualInterface {
        self.station
    }

    pub const fn channel_context(self) -> ChannelContextId {
        self.station.channel_context
    }

    pub const fn phy_mode(self) -> EspNowPhyMode {
        self.phy_mode
    }

    pub const fn security(self) -> EspNowPeerSecurity {
        EspNowPeerSecurity::Plaintext
    }

    pub const fn encoded_len(self) -> usize {
        self.frame.encoded_len()
    }

    pub fn encode(self, output: &mut [u8]) -> Result<usize, EspNowV1WireError> {
        self.frame.encode(output)
    }
}

/// Portable, fully validated v2 plaintext handoff.
///
/// A chip runtime may encode this value only while it owns a sufficiently
/// large ordinary Action-frame TX buffer and the active station/channel.
#[derive(Clone, Copy, Debug)]
pub struct EspNowPreparedV2Tx<'payload> {
    peer: EspNowPeerId,
    home_channel: WifiChannel,
    channel_policy: EspNowPeerChannelPolicy,
    station: BoundVirtualInterface,
    phy_mode: EspNowPhyMode,
    frame: EspNowV2Frame<'payload>,
}

impl EspNowPreparedV2Tx<'_> {
    pub const fn peer(self) -> EspNowPeerId {
        self.peer
    }

    pub const fn destination(self) -> EspNowDestination {
        self.frame.destination()
    }

    pub const fn home_channel(self) -> WifiChannel {
        self.home_channel
    }

    /// Exact channel on which the backend may publish this frame.
    pub const fn transmit_channel(self) -> WifiChannel {
        self.channel_policy.channel()
    }

    pub const fn channel_policy(self) -> EspNowPeerChannelPolicy {
        self.channel_policy
    }

    pub const fn station(self) -> BoundVirtualInterface {
        self.station
    }

    pub const fn channel_context(self) -> ChannelContextId {
        self.station.channel_context
    }

    pub const fn phy_mode(self) -> EspNowPhyMode {
        self.phy_mode
    }

    pub const fn security(self) -> EspNowPeerSecurity {
        EspNowPeerSecurity::Plaintext
    }

    pub const fn encoded_len(self) -> usize {
        self.frame.encoded_len()
    }

    pub fn encode(self, output: &mut [u8]) -> Result<usize, EspNowV2WireError> {
        self.frame.encode(output)
    }
}

/// Protocol-admitted borrowed receive frame.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EspNowReceivedV1<'frame> {
    peer: EspNowPeerId,
    frame: EspNowV1Frame<'frame>,
}

impl<'frame> EspNowReceivedV1<'frame> {
    pub const fn peer(self) -> EspNowPeerId {
        self.peer
    }

    pub const fn frame(self) -> EspNowV1Frame<'frame> {
        self.frame
    }
}

/// Protocol-admitted borrowed v2 receive frame. Its element bodies remain
/// borrowed; use the codec iterator or caller-owned reassembly storage before
/// releasing the underlying RX lease.
#[derive(Clone, Copy, Debug)]
pub struct EspNowReceivedV2<'frame> {
    peer: EspNowPeerId,
    frame: EspNowV2Frame<'frame>,
}

impl<'frame> EspNowReceivedV2<'frame> {
    pub const fn peer(self) -> EspNowPeerId {
        self.peer
    }

    pub const fn frame(self) -> EspNowV2Frame<'frame> {
        self.frame
    }
}

/// Owned v1 datagram safe to retain after the RX staging lease is released.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EspNowOwnedReceivedV1 {
    peer: EspNowPeerId,
    destination: EspNowDestination,
    source: EspNowUnicastAddress,
    random_value: EspNowRandomValue,
    sequence_number: SequenceNumber,
    retry: bool,
    payload_length: u8,
    payload: [u8; ESP_NOW_V1_MAX_PAYLOAD_LEN],
}

impl EspNowOwnedReceivedV1 {
    /// Copy one already-admitted borrowed datagram into fixed-capacity owned
    /// storage. The wire codec proves the payload length is at most 250.
    pub fn copy_from(received: EspNowReceivedV1<'_>) -> Self {
        let frame = received.frame();
        let payload = frame.action().payload().bytes();
        let mut owned_payload = [0_u8; ESP_NOW_V1_MAX_PAYLOAD_LEN];
        owned_payload[..payload.len()].copy_from_slice(payload);
        Self {
            peer: received.peer(),
            destination: frame.destination(),
            source: frame.source(),
            random_value: frame.action().random_value(),
            sequence_number: frame.sequence_number(),
            retry: frame.retry(),
            payload_length: payload.len() as u8,
            payload: owned_payload,
        }
    }

    pub const fn peer(&self) -> EspNowPeerId {
        self.peer
    }

    pub const fn destination(&self) -> EspNowDestination {
        self.destination
    }

    pub const fn source(&self) -> EspNowUnicastAddress {
        self.source
    }

    pub const fn random_value(&self) -> EspNowRandomValue {
        self.random_value
    }

    pub const fn sequence_number(&self) -> SequenceNumber {
        self.sequence_number
    }

    pub const fn retry(&self) -> bool {
        self.retry
    }

    pub fn payload(&self) -> &[u8] {
        &self.payload[..usize::from(self.payload_length)]
    }
}

/// Owned v2 datagram safe to retain after the RX staging lease is released.
///
/// The large payload lives only in explicitly provisioned application
/// storage. Runtime dispatch continues to borrow the 1700-byte RX arena.
#[derive(Debug)]
pub struct EspNowOwnedReceivedV2 {
    peer: EspNowPeerId,
    destination: EspNowDestination,
    source: EspNowUnicastAddress,
    random_value: EspNowRandomValue,
    sequence_number: SequenceNumber,
    retry: bool,
    payload_length: u16,
    payload: [u8; ESP_NOW_V2_MAX_PAYLOAD_LEN],
}

impl EspNowOwnedReceivedV2 {
    pub fn copy_from(received: EspNowReceivedV2<'_>) -> Result<Self, EspNowV2WireError> {
        let frame = received.frame();
        let mut payload = [0_u8; ESP_NOW_V2_MAX_PAYLOAD_LEN];
        let payload_length = frame.action().copy_payload(&mut payload)?;
        Ok(Self {
            peer: received.peer(),
            destination: frame.destination(),
            source: frame.source(),
            random_value: frame.action().random_value(),
            sequence_number: frame.sequence_number(),
            retry: frame.retry(),
            payload_length: payload_length as u16,
            payload,
        })
    }

    pub const fn peer(&self) -> EspNowPeerId {
        self.peer
    }

    pub const fn destination(&self) -> EspNowDestination {
        self.destination
    }

    pub const fn source(&self) -> EspNowUnicastAddress {
        self.source
    }

    pub const fn random_value(&self) -> EspNowRandomValue {
        self.random_value
    }

    pub const fn sequence_number(&self) -> SequenceNumber {
        self.sequence_number
    }

    pub const fn retry(&self) -> bool {
        self.retry
    }

    pub fn payload(&self) -> &[u8] {
        &self.payload[..usize::from(self.payload_length)]
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EspNowSendError {
    Peer(EspNowPeerTableError),
    Wire(EspNowV1WireError),
}

impl fmt::Display for EspNowSendError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Peer(error) => write!(formatter, "ESP-NOW peer error: {error}"),
            Self::Wire(error) => write!(formatter, "ESP-NOW frame error: {error}"),
        }
    }
}

impl core::error::Error for EspNowSendError {}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EspNowV2SendError {
    Peer(EspNowPeerTableError),
    PeerNotV2Capable {
        peer: EspNowPeerId,
        capability: EspNowPeerCapability,
    },
    Wire(EspNowV2WireError),
}

impl fmt::Display for EspNowV2SendError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Peer(error) => write!(formatter, "ESP-NOW v2 peer error: {error}"),
            Self::PeerNotV2Capable { peer, capability } => write!(
                formatter,
                "ESP-NOW peer {peer:?} has {capability:?} capability, not v2"
            ),
            Self::Wire(error) => write!(formatter, "ESP-NOW v2 frame error: {error}"),
        }
    }
}

impl core::error::Error for EspNowV2SendError {}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EspNowReceiveError {
    StationBindingMismatch {
        configured: BoundVirtualInterface,
        active: BoundVirtualInterface,
    },
    ChannelMismatch {
        configured: WifiChannel,
        active: WifiChannel,
    },
    Wire(EspNowV1WireError),
    ForeignDestination(EspNowDestination),
    UnknownPeer(EspNowUnicastAddress),
}

impl fmt::Display for EspNowReceiveError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::StationBindingMismatch { configured, active } => write!(
                formatter,
                "ESP-NOW configured station binding {configured:?} differs from active binding {active:?}"
            ),
            Self::ChannelMismatch { configured, active } => write!(
                formatter,
                "ESP-NOW configured channel {configured:?} differs from active channel {active:?}"
            ),
            Self::Wire(error) => write!(formatter, "ESP-NOW frame error: {error}"),
            Self::ForeignDestination(destination) => {
                write!(formatter, "ESP-NOW frame is addressed to {destination:?}")
            }
            Self::UnknownPeer(source) => {
                write!(formatter, "ESP-NOW source {source:?} is not configured")
            }
        }
    }
}

impl core::error::Error for EspNowReceiveError {}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EspNowV2ReceiveError {
    StationBindingMismatch {
        configured: BoundVirtualInterface,
        active: BoundVirtualInterface,
    },
    ChannelMismatch {
        configured: WifiChannel,
        active: WifiChannel,
    },
    Wire(EspNowV2WireError),
    ForeignDestination(EspNowDestination),
    UnknownPeer(EspNowUnicastAddress),
    Peer(EspNowPeerTableError),
    PeerNotV2Capable {
        peer: EspNowPeerId,
        capability: EspNowPeerCapability,
    },
}

impl fmt::Display for EspNowV2ReceiveError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::StationBindingMismatch { configured, active } => write!(
                formatter,
                "ESP-NOW v2 configured station binding {configured:?} differs from active binding {active:?}"
            ),
            Self::ChannelMismatch { configured, active } => write!(
                formatter,
                "ESP-NOW v2 configured channel {configured:?} differs from active channel {active:?}"
            ),
            Self::Wire(error) => write!(formatter, "ESP-NOW v2 frame error: {error}"),
            Self::ForeignDestination(destination) => {
                write!(
                    formatter,
                    "ESP-NOW v2 frame is addressed to {destination:?}"
                )
            }
            Self::UnknownPeer(source) => {
                write!(formatter, "ESP-NOW v2 source {source:?} is not configured")
            }
            Self::Peer(error) => write!(formatter, "ESP-NOW v2 peer error: {error}"),
            Self::PeerNotV2Capable { peer, capability } => write!(
                formatter,
                "ESP-NOW peer {peer:?} has {capability:?} capability, not v2"
            ),
        }
    }
}

impl core::error::Error for EspNowV2ReceiveError {}
