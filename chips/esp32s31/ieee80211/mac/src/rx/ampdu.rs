//! Hardware-bank ownership and public RX BlockAck classification.
//! Software sequence reordering belongs to the upper MAC.

use oer_ieee80211_wire::sequence::SequenceNumber;

use crate::{
    MacInterface,
    rx::{
        PUBLIC_HEADER_SIZE,
        hardware::{S31_RX_BLOCK_ACK_MAX_TID, S31RxBlockAckAgreement},
    },
};

// SOURCE: complete `libnet80211.a[ieee80211_ht.o]::
// ampdu_rx_start.constprop.0`. The vendor agreement owner selects the smaller
// of the peer request and `g_wifi_menuconfig+0x30`, whose normal configured
// value is 64. Its later ordinary activation path passes the constant 64 to
// `ic_add_rx_ba`; complete `libpp.a[hal_ampdu.o]::
// hal_agreement_add_rx_ba` publishes that value in the seven-bit hardware
// window. Keep the protocol/reorder window independent of the number of RX DMA
// descriptors: descriptors are recycled while this sequence window remains
// active.
pub const RX_BLOCK_ACK_MAX_WINDOW: u16 = 64;
use oer_ieee80211_wire::{
    frame::{FrameControlFields, FrameType, QosControl},
    sequence::SequenceControl,
    trigger::BarControlFields,
};

/// Public ordering identity of one protected QoS data MPDU.
///
/// These fields precede the encrypted body. Both station and access-point
/// receive paths use this exact classifier before deciding whether an active
/// agreement owns the frame.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RxBlockAckMpduKey {
    pub peer: [u8; 6],
    pub tid: u8,
    pub sequence: SequenceNumber,
    pub retry: bool,
}

/// Extract the role-neutral infrastructure RX BlockAck key.
///
/// `local_address` is the receiver address (Address 1). A connected STA also
/// supplies `expected_peer`; an AP leaves it absent and validates admission in
/// its peer table after extraction. Group, foreign, unprotected, non-QoS and
/// fragmented frames do not enter a reorder sequence space.
#[inline(always)]
pub fn rx_block_ack_mpdu_key(
    raw: &[u8],
    local_address: [u8; 6],
    expected_peer: Option<[u8; 6]>,
) -> Option<RxBlockAckMpduKey> {
    let frame_offset = PUBLIC_HEADER_SIZE;
    let frame_control =
        FrameControlFields::from_bytes([*raw.get(frame_offset)?, *raw.get(frame_offset + 1)?]);
    if frame_control.kind() != FrameType::Data
        || !frame_control.protected()
        || !frame_control.qos()
        || frame_control.more_fragments()
        || raw.get(frame_offset + 4..frame_offset + 10)? != local_address
    {
        return None;
    }
    let peer: [u8; 6] = raw
        .get(frame_offset + 10..frame_offset + 16)?
        .try_into()
        .ok()?;
    if expected_peer.is_some_and(|expected| expected != peer) {
        return None;
    }
    let sequence_control =
        u16::from_le_bytes([*raw.get(frame_offset + 22)?, *raw.get(frame_offset + 23)?]);
    if SequenceControl::from_bytes(sequence_control.to_le_bytes()).fragment() != 0 {
        return None;
    }
    let qos_offset = frame_offset + 24 + usize::from(frame_control.ds() == 3) * 6;
    Some(RxBlockAckMpduKey {
        peer,
        tid: QosControl::from_bytes([*raw.get(qos_offset)?, 0]).tid(),
        sequence: SequenceNumber::from_sequence_control(sequence_control),
        retry: frame_control.retry(),
    })
}

/// Receive BlockAck session and window start named by one BlockAckReq.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RxBlockAckRequestKey {
    pub peer: [u8; 6],
    pub tid: u8,
    pub starting_sequence: SequenceNumber,
}

const BLOCK_ACK_REQUEST_LENGTH: usize = 20;

/// Extract the receive session a BlockAckReq addresses.
///
/// Only the TID and the Starting Sequence Control are read. The BAR Control
/// policy bits (BAR Ack Policy, Compressed, Multi-TID) are not validated,
/// and a TID above 7 is ignored.
///
/// SOURCE: `libnet80211.a::sta_recv_ctl`/`hostap_recv_ctl` dispatch control
/// subtype 8 to `ieee80211_recv_bar`, and `ieee80211_process_bar_info` reads
/// only `bar_control >> 12` and `ssc >> 4` (vendor-library RX BlockAckReq
/// answer).
pub fn rx_block_ack_request_key(
    raw: &[u8],
    local_address: [u8; 6],
    expected_peer: Option<[u8; 6]>,
) -> Option<RxBlockAckRequestKey> {
    let frame = raw.get(PUBLIC_HEADER_SIZE..PUBLIC_HEADER_SIZE + BLOCK_ACK_REQUEST_LENGTH)?;
    let frame_control = u16::from_le_bytes([frame[0], frame[1]]);
    if FrameControlFields::from_raw(frame_control).kind() != FrameType::Control
        || FrameControlFields::from_raw(frame_control).subtype() != 8
        || frame[4..10] != local_address
    {
        return None;
    }
    let peer: [u8; 6] = frame[10..16].try_into().ok()?;
    if expected_peer.is_some_and(|expected| expected != peer) {
        return None;
    }
    let control = u16::from_le_bytes([frame[16], frame[17]]);
    let tid = BarControlFields::from_bytes(control.to_le_bytes()).tid();
    if tid > 7 {
        return None;
    }
    Some(RxBlockAckRequestKey {
        peer,
        tid,
        starting_sequence: SequenceNumber::from_sequence_control(u16::from_le_bytes([
            frame[18], frame[19],
        ])),
    })
}

pub use oer_ieee80211_wire::block_ack::{
    ADDBA_STATUS_REQUEST_DECLINED, RxAddbaResponseError, write_declined_addba_response,
    write_successful_addba_response,
};

pub const RX_BLOCK_ACK_BANK_COUNT: usize = 8;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RxBlockAckSessionsError {
    DelayedPolicyUnsupported,
    InvalidTid(u8),
    InvalidWindow(u16),
    NonzeroTimeout(u16),
    ActivationBusy,
    InterfaceActive(MacInterface),
    StaleActivation,
    NoFreePendingSlot,
    NoFreePeerSlot,
    NoFreeHardwareBank,
    Response(RxAddbaResponseError),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct PendingRxBlockAck {
    requested_window: u16,
    starting_sequence: SequenceNumber,
    dialog_token: u8,
    tid: u8,
    peer_index: u8,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct RxBlockAckPeer {
    interface: MacInterface,
    address: [u8; 6],
}

impl PendingRxBlockAck {
    const EMPTY_PEER_INDEX: u8 = u8::MAX;
    const EMPTY: Self = Self {
        requested_window: 0,
        starting_sequence: SequenceNumber::ZERO,
        dialog_token: 0,
        tid: 0,
        peer_index: Self::EMPTY_PEER_INDEX,
    };

    const fn occupied(&self) -> bool {
        self.peer_index != Self::EMPTY_PEER_INDEX
    }
}

/// One validated-boundary request to establish peer-to-local RX ordering.
///
/// Parsing remains owned by the portable 802.11 layer. This value binds the
/// parsed action to its authenticated link peer before the request crosses
/// into hardware-bank ownership.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RxBlockAckRequest {
    pub interface: MacInterface,
    pub peer: [u8; 6],
    pub dialog_token: u8,
    pub tid: u8,
    pub immediate: bool,
    pub requested_window: u16,
    pub timeout_tu: u16,
    pub starting_sequence: SequenceNumber,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RxBlockAckSnapshot {
    pub hardware_index: u8,
    pub interface: MacInterface,
    pub peer: [u8; 6],
    pub tid: u8,
    pub window: u16,
    pub starting_sequence: SequenceNumber,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RxBlockAckIdentity {
    pub hardware_index: u8,
    pub interface: MacInterface,
    pub peer: [u8; 6],
    pub tid: u8,
}

impl RxBlockAckSnapshot {
    pub const fn identity(self) -> RxBlockAckIdentity {
        RxBlockAckIdentity {
            hardware_index: self.hardware_index,
            interface: self.interface,
            peer: self.peer,
            tid: self.tid,
        }
    }
}

#[derive(Clone, Copy)]
struct ActiveRxBlockAck {
    window: u16,
    starting_sequence: SequenceNumber,
    peer_index: u8,
    tid: u8,
}

impl ActiveRxBlockAck {
    const EMPTY: Self = Self {
        window: 0,
        starting_sequence: SequenceNumber::ZERO,
        peer_index: PendingRxBlockAck::EMPTY_PEER_INDEX,
        tid: 0,
    };

    const fn occupied(&self) -> bool {
        self.peer_index != PendingRxBlockAck::EMPTY_PEER_INDEX
    }
}

/// One private activation transaction spanning software and hardware state.
///
/// A caller cannot construct or clone this value. It must either return it to
/// [`RxBlockAckSessions::commit`] after programming the hardware and
/// transmitting the response, or to [`RxBlockAckSessions::cancel`] after
/// undoing any hardware publication.
pub struct RxBlockAckActivation {
    generation: u32,
    hardware: S31RxBlockAckAgreement,
    response_body: [u8; 9],
    negotiated: RxBlockAckSnapshot,
    replaced: Option<RxBlockAckSnapshot>,
}

impl RxBlockAckActivation {
    pub const fn hardware(&self) -> S31RxBlockAckAgreement {
        self.hardware
    }

    pub const fn response_body(&self) -> &[u8; 9] {
        &self.response_body
    }

    pub const fn negotiated(&self) -> RxBlockAckSnapshot {
        self.negotiated
    }

    pub const fn replaced(&self) -> Option<RxBlockAckSnapshot> {
        self.replaced
    }
}

/// Fixed VIF-aware owner of the eight ordinary S31 receive BlockAck banks.
///
/// SOURCE: complete `libnet80211.a[ieee80211_ht.o]::
/// ampdu_rx_start.constprop.0` stores one independent agreement per peer/TID and
/// bounds the negotiated software window by the configured maximum. Complete
/// `libpp.a[hal_ampdu.o]::hal_agreement_add_rx_ba` owns eight
/// hardware banks and receives the literal hardware window 64 on the ordinary
/// receive path. The protocol window and hardware window therefore remain
/// deliberately distinct here. One instance owns every active STA and AP
/// agreement; peer identity includes the MAC interface so equal peer/TID
/// values on different VIFs cannot alias one bank or reorder sequence space.
pub struct RxBlockAckSessions<const PEER_CAPACITY: usize = 1> {
    peers: [Option<RxBlockAckPeer>; PEER_CAPACITY],
    pending: [PendingRxBlockAck; RX_BLOCK_ACK_BANK_COUNT],
    active: [ActiveRxBlockAck; RX_BLOCK_ACK_BANK_COUNT],
    generation: u32,
    in_flight: Option<u32>,
    maximum_window: u16,
}

impl<const PEER_CAPACITY: usize> RxBlockAckSessions<PEER_CAPACITY> {
    /// An empty peer table with geometry checked at compile time.
    ///
    /// ```compile_fail,E0080
    /// use oer_esp32s31_ieee80211_mac::rx::ampdu::RxBlockAckSessions;
    /// let _ = RxBlockAckSessions::<0>::new();
    /// ```
    ///
    /// ```compile_fail,E0080
    /// use oer_esp32s31_ieee80211_mac::rx::ampdu::RxBlockAckSessions;
    /// let _ = RxBlockAckSessions::<256>::new();
    /// ```
    pub const fn new() -> Self {
        const {
            assert!(
                PEER_CAPACITY != 0,
                "RX BlockAck peer table must not be empty"
            );
            assert!(
                PEER_CAPACITY <= u8::MAX as usize,
                "RX BlockAck peer index must fit its compact owner"
            );
        }
        Self {
            peers: [None; PEER_CAPACITY],
            pending: [PendingRxBlockAck::EMPTY; RX_BLOCK_ACK_BANK_COUNT],
            active: [ActiveRxBlockAck::EMPTY; RX_BLOCK_ACK_BANK_COUNT],
            generation: 0,
            in_flight: None,
            maximum_window: RX_BLOCK_ACK_MAX_WINDOW,
        }
    }

    /// Construct an agreement owner with an integration-qualified reorder
    /// limit no wider than the vendor maximum.
    ///
    /// The integrating layer uses this when its independent staging pool must
    /// retain credits beyond the negotiated reorder window. Hardware still
    /// receives its separately qualified 64-entry agreement geometry.
    pub const fn with_maximum_window(maximum_window: u16) -> Result<Self, RxBlockAckSessionsError> {
        if maximum_window == 0 || maximum_window > RX_BLOCK_ACK_MAX_WINDOW {
            return Err(RxBlockAckSessionsError::InvalidWindow(maximum_window));
        }
        Ok(Self {
            maximum_window,
            ..Self::new()
        })
    }

    pub const fn maximum_window(&self) -> u16 {
        self.maximum_window
    }

    /// Reinitialize the complete software bank image after the physical MAC
    /// and all eight hardware agreements have already been reset.
    ///
    /// Role transitions reuse the statically allocated session owner. The
    /// resource profile, rather than the role implementation, owns how much
    /// downstream reorder capacity is available; resetting an AP epoch must
    /// therefore not silently restore the vendor maximum.
    pub fn reset_after_hardware_reset(&mut self) {
        let maximum_window = self.maximum_window;
        *self = Self {
            maximum_window,
            ..Self::new()
        };
    }

    /// Prepare one inactive VIF without disturbing agreements owned by the
    /// other interface.
    ///
    /// Active agreements must be stopped and cleared through their normal
    /// hardware transaction before this edge. A transaction already in flight
    /// is a global bank-owner boundary, so role start waits for it rather than
    /// guessing which software generation hardware currently observes.
    pub fn prepare_interface(
        &mut self,
        interface: MacInterface,
    ) -> Result<(), RxBlockAckSessionsError> {
        if self.in_flight.is_some() {
            return Err(RxBlockAckSessionsError::ActivationBusy);
        }
        if self
            .snapshots_for(interface)
            .into_iter()
            .any(|agreement| agreement.is_some())
        {
            return Err(RxBlockAckSessionsError::InterfaceActive(interface));
        }
        for pending in &mut self.pending {
            if pending.occupied()
                && self.peers[usize::from(pending.peer_index)]
                    .is_some_and(|peer| peer.interface == interface)
            {
                *pending = PendingRxBlockAck::EMPTY;
            }
        }
        self.reclaim_unused_peers();
        Ok(())
    }

    /// Admit one parsed immediate ADDBA request into the shared pending bank.
    /// A newer request for the same peer/TID replaces the older unexecuted
    /// request without consuming another hardware-bank candidate.
    pub fn offer(&mut self, request: RxBlockAckRequest) -> Result<(), RxBlockAckSessionsError> {
        let RxBlockAckRequest {
            interface,
            peer,
            dialog_token,
            tid,
            immediate,
            requested_window,
            timeout_tu,
            starting_sequence,
        } = request;
        if !immediate {
            return Err(RxBlockAckSessionsError::DelayedPolicyUnsupported);
        }
        if tid > S31_RX_BLOCK_ACK_MAX_TID {
            return Err(RxBlockAckSessionsError::InvalidTid(tid));
        }
        if requested_window == 0 || requested_window > 0x03ff {
            return Err(RxBlockAckSessionsError::InvalidWindow(requested_window));
        }
        if timeout_tu != 0 {
            return Err(RxBlockAckSessionsError::NonzeroTimeout(timeout_tu));
        }
        self.reclaim_unused_peers();
        let existing_peer_index = self.peer_index(interface, peer);
        let pending_index = self
            .pending
            .iter()
            .position(|pending| {
                existing_peer_index.is_some_and(|peer_index| {
                    pending.occupied()
                        && usize::from(pending.peer_index) == peer_index
                        && pending.tid == tid
                })
            })
            .or_else(|| self.pending.iter().position(|pending| !pending.occupied()))
            .ok_or(RxBlockAckSessionsError::NoFreePendingSlot)?;
        let peer_index = match existing_peer_index {
            Some(index) => index,
            None => self
                .peers
                .iter()
                .position(Option::is_none)
                .ok_or(RxBlockAckSessionsError::NoFreePeerSlot)?,
        };
        self.peers[peer_index] = Some(RxBlockAckPeer {
            interface,
            address: peer,
        });
        self.pending[pending_index] = PendingRxBlockAck {
            peer_index: peer_index as u8,
            dialog_token,
            tid,
            requested_window,
            starting_sequence,
        };
        Ok(())
    }

    /// Begin the oldest pending request by bank index and reserve its hardware
    /// bank until the caller commits or cancels the returned transaction.
    pub fn begin_pending(
        &mut self,
    ) -> Result<Option<RxBlockAckActivation>, RxBlockAckSessionsError> {
        if self.in_flight.is_some() {
            return Err(RxBlockAckSessionsError::ActivationBusy);
        }
        let Some((pending_index, request)) = self
            .pending
            .iter()
            .enumerate()
            .find_map(|(index, request)| request.occupied().then_some((index, *request)))
        else {
            return Ok(None);
        };
        let hardware_index = self
            .active
            .iter()
            .position(|agreement| {
                agreement.occupied()
                    && agreement.peer_index == request.peer_index
                    && agreement.tid == request.tid
            })
            .or_else(|| {
                self.active
                    .iter()
                    .position(|agreement| !agreement.occupied())
            })
            .ok_or(RxBlockAckSessionsError::NoFreeHardwareBank)?;
        // Keep the parsed request pending when all physical banks are busy.
        // A later peer teardown can free a bank without requiring the peer to
        // retransmit its action frame merely because local control scheduling
        // reached this edge first.
        self.pending[pending_index] = PendingRxBlockAck::EMPTY;
        let replaced = self.snapshot_at(hardware_index);
        self.active[hardware_index] = ActiveRxBlockAck::EMPTY;
        let peer = self.peers[usize::from(request.peer_index)]
            .expect("occupied request owns one peer-table entry");
        let window = request.requested_window.min(self.maximum_window);
        let mut response_body = [0_u8; 9];
        write_successful_addba_response(
            &mut response_body,
            request.dialog_token,
            request.tid,
            window,
        )
        .map_err(RxBlockAckSessionsError::Response)?;

        self.generation = next_rx_block_ack_generation(self.generation);
        self.in_flight = Some(self.generation);
        let negotiated = RxBlockAckSnapshot {
            hardware_index: hardware_index as u8,
            interface: peer.interface,
            peer: peer.address,
            tid: request.tid,
            window,
            starting_sequence: request.starting_sequence,
        };
        Ok(Some(RxBlockAckActivation {
            generation: self.generation,
            hardware: S31RxBlockAckAgreement {
                hardware_index: hardware_index as u8,
                interface: peer.interface,
                peer: peer.address,
                tid: request.tid,
                starting_sequence: request.starting_sequence,
                // The vendor ordinary receive hardware leaf receives 64 even
                // when the negotiated/reorder window is smaller.
                window: RX_BLOCK_ACK_MAX_WINDOW,
            },
            response_body,
            negotiated,
            replaced,
        }))
    }

    pub fn commit(
        &mut self,
        activation: RxBlockAckActivation,
    ) -> Result<RxBlockAckSnapshot, RxBlockAckSessionsError> {
        if self.in_flight != Some(activation.generation) {
            return Err(RxBlockAckSessionsError::StaleActivation);
        }
        self.in_flight = None;
        let snapshot = activation.negotiated;
        let peer_index = self
            .peer_index(snapshot.interface, snapshot.peer)
            .expect("activation peer remains retained until commit");
        self.active[usize::from(snapshot.hardware_index)] = ActiveRxBlockAck {
            window: snapshot.window,
            starting_sequence: snapshot.starting_sequence,
            peer_index: peer_index as u8,
            tid: snapshot.tid,
        };
        self.reclaim_unused_peers();
        Ok(snapshot)
    }

    pub fn cancel(
        &mut self,
        activation: RxBlockAckActivation,
    ) -> Result<(), RxBlockAckSessionsError> {
        if self.in_flight != Some(activation.generation) {
            return Err(RxBlockAckSessionsError::StaleActivation);
        }
        self.in_flight = None;
        self.reclaim_unused_peers();
        Ok(())
    }

    /// Remove the software owner before the caller clears the returned bank.
    pub fn stop(
        &mut self,
        interface: MacInterface,
        peer: [u8; 6],
        tid: u8,
    ) -> Option<RxBlockAckSnapshot> {
        let peer_index = self.peer_index(interface, peer)?;
        let index = self.active.iter().position(|agreement| {
            agreement.occupied()
                && usize::from(agreement.peer_index) == peer_index
                && agreement.tid == tid
        })?;
        let snapshot = self.snapshot_at(index);
        self.active[index] = ActiveRxBlockAck::EMPTY;
        self.reclaim_unused_peers();
        snapshot
    }

    /// Remove an unexecuted request after the caller has explicitly declined
    /// it on air. Active agreement state is deliberately untouched.
    pub fn discard_pending(&mut self, interface: MacInterface, peer: [u8; 6], tid: u8) -> bool {
        let Some(peer_index) = self.peer_index(interface, peer) else {
            return false;
        };
        let mut discarded = false;
        for pending in &mut self.pending {
            if pending.occupied()
                && usize::from(pending.peer_index) == peer_index
                && pending.tid == tid
            {
                *pending = PendingRxBlockAck::EMPTY;
                discarded = true;
            }
        }
        self.reclaim_unused_peers();
        discarded
    }

    /// Remove every agreement owned by one peer before clearing the returned
    /// hardware banks. The fixed result preserves bank identity and needs no
    /// allocation in AP peer teardown.
    pub fn stop_peer(
        &mut self,
        interface: MacInterface,
        peer: [u8; 6],
    ) -> [Option<RxBlockAckSnapshot>; RX_BLOCK_ACK_BANK_COUNT] {
        let Some(peer_index) = self.peer_index(interface, peer) else {
            return [None; RX_BLOCK_ACK_BANK_COUNT];
        };
        for pending in &mut self.pending {
            if pending.occupied() && usize::from(pending.peer_index) == peer_index {
                *pending = PendingRxBlockAck::EMPTY;
            }
        }
        let stopped = core::array::from_fn(|index| {
            if self.active[index].occupied()
                && usize::from(self.active[index].peer_index) == peer_index
            {
                let snapshot = self.snapshot_at(index);
                self.active[index] = ActiveRxBlockAck::EMPTY;
                snapshot
            } else {
                None
            }
        });
        self.reclaim_unused_peers();
        stopped
    }

    pub fn snapshots(&self) -> [Option<RxBlockAckSnapshot>; RX_BLOCK_ACK_BANK_COUNT] {
        core::array::from_fn(|index| self.snapshot_at(index))
    }

    pub fn snapshots_for(
        &self,
        interface: MacInterface,
    ) -> [Option<RxBlockAckSnapshot>; RX_BLOCK_ACK_BANK_COUNT] {
        core::array::from_fn(|index| {
            self.snapshot_at(index)
                .filter(|agreement| agreement.interface == interface)
        })
    }

    #[inline(always)]
    fn peer_index(&self, interface: MacInterface, peer: [u8; 6]) -> Option<usize> {
        self.peers.iter().position(|entry| {
            *entry
                == Some(RxBlockAckPeer {
                    interface,
                    address: peer,
                })
        })
    }

    fn snapshot_at(&self, hardware_index: usize) -> Option<RxBlockAckSnapshot> {
        let agreement = self.active[hardware_index];
        if !agreement.occupied() {
            return None;
        }
        Some(RxBlockAckSnapshot {
            hardware_index: hardware_index as u8,
            interface: self.peers[usize::from(agreement.peer_index)]
                .expect("occupied agreement owns one peer-table entry")
                .interface,
            peer: self.peers[usize::from(agreement.peer_index)]
                .expect("occupied agreement owns one peer-table entry")
                .address,
            tid: agreement.tid,
            window: agreement.window,
            starting_sequence: agreement.starting_sequence,
        })
    }

    fn reclaim_unused_peers(&mut self) {
        if self.in_flight.is_some() {
            return;
        }
        for peer_index in 0..self.peers.len() {
            let referenced =
                self.pending.iter().any(|pending| {
                    pending.occupied() && usize::from(pending.peer_index) == peer_index
                }) || self.active.iter().any(|agreement| {
                    agreement.occupied() && usize::from(agreement.peer_index) == peer_index
                });
            if !referenced {
                self.peers[peer_index] = None;
            }
        }
    }
}

impl<const PEER_CAPACITY: usize> Default for RxBlockAckSessions<PEER_CAPACITY> {
    fn default() -> Self {
        Self::new()
    }
}

const fn next_rx_block_ack_generation(current: u32) -> u32 {
    let next = current.wrapping_add(1);
    if next == 0 { 1 } else { next }
}

#[cfg(test)]
mod tests;
