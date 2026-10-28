//! Owned Host ACL packets and standard completion events of LE links.

use bt_hci::{
    PacketKind,
    cmd::{Cmd, Opcode, controller_baseband::HostNumberOfCompletedPackets},
    data::{AclBroadcastFlag, AclPacket, AclPacketBoundary},
    event::EventKind,
    param::{ConnHandle, Error as HciError},
};

use core::num::NonZeroU8;

use crate::{HciBuildError, HciControllerResponse, encoding::Length};

/// Maximum LE ACL payload reported by the initial Controller profile.
pub const LE_ACL_DATA_PACKET_CAPACITY: usize = 251;
/// Complete HCI ACL packet body, including its four-byte ACL header.
pub const LE_CONTROLLER_ACL_PACKET_CAPACITY: usize = LE_ACL_DATA_PACKET_CAPACITY + 4;
/// Complete Number Of Completed Packets event size without an H4 indicator.
pub const LE_NUMBER_OF_COMPLETED_PACKETS_EVENT_CAPACITY: usize = 7;
/// Invalid-parameters Command Complete for Host Number Of Completed Packets.
pub const LE_HOST_COMPLETED_PACKETS_ERROR_EVENT_CAPACITY: usize = 6;

/// Controller-to-Host ACL policy accepted from the current HCI epoch.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LeControllerToHostAclProfile {
    maximum_payload: usize,
    total_packets: Option<u16>,
    flow_controlled: bool,
}

impl LeControllerToHostAclProfile {
    /// The Host's Controller-to-Host ACL buffer profile.
    pub const fn new(
        maximum_payload: usize,
        total_packets: Option<u16>,
        flow_controlled: bool,
    ) -> Self {
        Self {
            maximum_payload,
            total_packets,
            flow_controlled,
        }
    }

    /// Maximum data portion of one Controller-to-Host HCI ACL packet.
    pub const fn maximum_payload(self) -> usize {
        self.maximum_payload
    }

    /// Host packet capacity when declared in this reset epoch.
    pub const fn total_packets(self) -> Option<u16> {
        self.total_packets
    }

    /// Whether ACL flow control is enabled for Controller-to-Host delivery.
    pub const fn is_flow_controlled(self) -> bool {
        self.flow_controlled
    }
}

/// Response-less Host credit return: the packets the Host completed, per
/// connection handle.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LeHostCompletedPacketsCommand<'a> {
    /// Validated handle and count pairs, four octets each.
    pairs: &'a [u8],
}

impl<'a> LeHostCompletedPacketsCommand<'a> {
    /// Standard Host Number Of Completed Packets opcode.
    pub const OPCODE: Opcode = HostNumberOfCompletedPackets::OPCODE;

    /// Decode Host Number Of Completed Packets.
    pub fn decode(
        command: crate::HciCommandPacket<'a>,
    ) -> Result<Self, LeHostCompletedPacketsDecodeError> {
        if command.opcode() != Self::OPCODE {
            return Err(LeHostCompletedPacketsDecodeError::Unsupported);
        }
        let Some((&count, pairs)) = command.parameters().split_first() else {
            return Err(LeHostCompletedPacketsDecodeError::Malformed);
        };
        if Some(pairs.len()) != usize::from(count).checked_mul(4)
            || completed_pairs(pairs).any(|(handle, _)| handle > 0x0eff)
        {
            return Err(LeHostCompletedPacketsDecodeError::Malformed);
        }
        Ok(Self { pairs })
    }

    /// Each pair's connection handle and completed packets, in command
    /// order.
    pub fn pairs(self) -> impl Iterator<Item = (ConnHandle, u16)> + 'a {
        // Decode admitted every raw handle before this typed iterator exists.
        completed_pairs(self.pairs).map(|(handle, count)| (ConnHandle(handle), count))
    }
}

fn completed_pairs(mut bytes: &[u8]) -> impl Iterator<Item = (u16, u16)> + '_ {
    core::iter::from_fn(move || {
        let ([low, high, count_low, count_high], rest) = bytes.split_first_chunk::<4>()?;
        bytes = rest;
        Some((
            u16::from_le_bytes([*low, *high]),
            u16::from_le_bytes([*count_low, *count_high]),
        ))
    })
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// Why a command is not a valid Host Number Of Completed Packets.
pub enum LeHostCompletedPacketsDecodeError {
    /// Another opcode.
    Unsupported,
    /// The opcode matched but its parameters are invalid.
    Malformed,
}

/// Error response generated only for invalid Host completed-packet parameters.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LeHostCompletedPacketsErrorEvent {
    bytes: [u8; LE_HOST_COMPLETED_PACKETS_ERROR_EVENT_CAPACITY],
}

impl LeHostCompletedPacketsErrorEvent {
    /// Build the sole error response allowed for this special command.
    pub fn invalid_parameters() -> Self {
        let opcode = LeHostCompletedPacketsCommand::OPCODE.to_raw().to_le_bytes();
        Self {
            bytes: [
                EventKind::CommandComplete.0,
                4,
                1,
                opcode[0],
                opcode[1],
                HciError::INVALID_HCI_PARAMETERS.to_status().into_inner(),
            ],
        }
    }

    /// Complete HCI Event body without an H4 indicator.
    pub const fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }
}

impl HciControllerResponse for LeHostCompletedPacketsErrorEvent {
    fn kind(&self) -> PacketKind {
        PacketKind::Event
    }

    fn as_bytes(&self) -> &[u8] {
        self.as_bytes()
    }
}

/// Why a consumed Host ACL packet cannot enter the LE connection it names.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LeHostAclPacketRejection {
    /// No Host-visible connection exists yet.
    NoLiveConnection,
    /// The packet names a handle other than the connection it is copied for.
    UnknownConnectionIdentifier,
    /// LE-U accepts only point-to-point ACL packets.
    UnsupportedBroadcast,
    /// Host-to-Controller LE-U accepts only start-non-flushable or continuation.
    UnsupportedPacketBoundary,
    /// The payload exceeds the Controller's reported ACL packet length.
    PayloadTooLong {
        /// Complete Host payload length.
        length: usize,
        /// Maximum length reported for this Controller epoch.
        capacity: usize,
    },
    /// A zero-length Host ACL packet cannot contain an L2CAP fragment.
    EmptyPayload,
}

/// One borrowed LL fragment retained by its parent HCI packet owner.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LeHostAclFragment<'packet> {
    payload: &'packet [u8],
    continuing: bool,
}

/// One Controller-to-Host ACL packet copied from an accepted LL Data PDU.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LeControllerAclPacket {
    bytes: [u8; LE_CONTROLLER_ACL_PACKET_CAPACITY],
    length: Length<u16, LE_CONTROLLER_ACL_PACKET_CAPACITY>,
}

impl LeControllerAclPacket {
    /// The connection handle the packet names.
    pub fn handle(&self) -> ConnHandle {
        // Construction admits the handle before encoding its header flags.
        ConnHandle(u16::from_le_bytes([self.bytes[0], self.bytes[1] & 0x0f]))
    }

    /// Copy one nonempty LL fragment received on the connection `handle`.
    pub fn copy_from_ll(handle: ConnHandle, continuing: bool, payload: &[u8]) -> Option<Self> {
        if handle.raw() > 0x0eff
            || payload.is_empty()
            || payload.len() > LE_ACL_DATA_PACKET_CAPACITY
        {
            return None;
        }
        let boundary = if continuing {
            AclPacketBoundary::Continuing
        } else {
            AclPacketBoundary::FirstFlushable
        };
        let encoded_handle = handle.into_inner() | ((boundary as u16) << 12);
        let payload_length = payload.len() as u16;
        let mut bytes = [0; LE_CONTROLLER_ACL_PACKET_CAPACITY];
        let [handle_low, handle_high] = encoded_handle.to_le_bytes();
        let [length_low, length_high] = payload_length.to_le_bytes();
        let [a, b, c, d, tail @ ..] = &mut bytes;
        *a = handle_low;
        *b = handle_high;
        *c = length_low;
        *d = length_high;
        // The payload admission above proves that every source octet fits.
        for (destination, source) in tail.iter_mut().zip(payload) {
            *destination = *source;
        }
        Some(Self {
            bytes,
            length: Length::<u16, LE_CONTROLLER_ACL_PACKET_CAPACITY>::new(
                payload_length.checked_add(4)?,
            )?,
        })
    }

    /// Copy the next Host-sized fragment while retaining the L2CAP boundary.
    pub fn next_host_fragment(&self, offset: usize, maximum: usize) -> Option<Self> {
        let payload = self.as_bytes().get(4..)?;
        if maximum == 0 || offset >= payload.len() {
            return None;
        }
        let remaining = payload.get(offset..)?;
        let fragment = remaining.get(..maximum.min(remaining.len()))?;
        let encoded_handle = u16::from_le_bytes([self.bytes[0], self.bytes[1]]);
        let handle = self.handle();
        let continuing =
            offset != 0 || ((encoded_handle >> 12) & 0x03) == AclPacketBoundary::Continuing as u16;
        Self::copy_from_ll(handle, continuing, fragment)
    }

    /// ACL payload length excluding the four-byte HCI header.
    pub fn payload_length(&self) -> usize {
        self.as_bytes().iter().skip(4).len()
    }

    /// Complete HCI ACL body without an H4 indicator.
    pub fn as_bytes(&self) -> &[u8] {
        self.length.borrow(&self.bytes)
    }
}

impl LeHostAclFragment<'_> {
    /// Payload bytes for the next Link Layer Data PDU.
    pub const fn payload(&self) -> &[u8] {
        self.payload
    }

    /// Whether this fragment continues a previously started L2CAP PDU.
    pub const fn is_continuing(&self) -> bool {
        self.continuing
    }
}

/// One copied HCI ACL packet retaining its Host credit through LL acknowledgement.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LeHostAclPacket {
    boundary: AclPacketBoundary,
    bytes: [u8; LE_ACL_DATA_PACKET_CAPACITY],
    length: u16,
    acknowledged: u16,
    in_flight_end: Option<NonZeroU8>,
}

impl LeHostAclPacket {
    /// Validate and copy a packet for the sole Host-visible LE connection.
    pub fn copy_from(
        packet: AclPacket<'_>,
        live_handle: Option<ConnHandle>,
        configured_capacity: u16,
    ) -> Result<Self, LeHostAclPacketRejection> {
        let Some(live_handle) = live_handle else {
            return Err(LeHostAclPacketRejection::NoLiveConnection);
        };
        if packet.handle() != live_handle {
            return Err(LeHostAclPacketRejection::UnknownConnectionIdentifier);
        }
        if packet.broadcast_flag() != AclBroadcastFlag::PointToPoint {
            return Err(LeHostAclPacketRejection::UnsupportedBroadcast);
        }
        if !matches!(
            packet.boundary_flag(),
            AclPacketBoundary::FirstNonFlushable | AclPacketBoundary::Continuing
        ) {
            return Err(LeHostAclPacketRejection::UnsupportedPacketBoundary);
        }
        let capacity = usize::from(configured_capacity).min(LE_ACL_DATA_PACKET_CAPACITY);
        if packet.data().len() > capacity {
            return Err(LeHostAclPacketRejection::PayloadTooLong {
                length: packet.data().len(),
                capacity,
            });
        }
        if packet.data().is_empty() {
            return Err(LeHostAclPacketRejection::EmptyPayload);
        }
        let mut bytes = [0; LE_ACL_DATA_PACKET_CAPACITY];
        // The configured and physical capacities were checked above.
        for (destination, source) in bytes.iter_mut().zip(packet.data()) {
            *destination = *source;
        }
        Ok(Self {
            boundary: packet.boundary_flag(),
            bytes,
            length: packet.data().len() as u16,
            acknowledged: 0,
            in_flight_end: None,
        })
    }

    /// Next unacknowledged fragment, bounded by the active LL payload profile.
    pub fn next_fragment(&self, maximum: usize) -> Option<LeHostAclFragment<'_>> {
        if maximum == 0 || self.in_flight_end.is_some() || self.acknowledged == self.length {
            return None;
        }
        let start = usize::from(self.acknowledged);
        let remaining = self.bytes.get(start..usize::from(self.length))?;
        let payload = remaining.get(..maximum.min(remaining.len()))?;
        Some(LeHostAclFragment {
            payload,
            continuing: start != 0 || self.boundary == AclPacketBoundary::Continuing,
        })
    }

    /// Commit that the exact next fragment entered the radio-owned TX graph.
    pub fn fragment_enqueued(&mut self, length: usize) -> Result<(), HciBuildError> {
        if let Some(end) = self.in_flight_end {
            return Err(HciBuildError::AclFragmentInFlight { end: end.get() });
        }
        let invalid = || HciBuildError::InvalidAclFragmentLength {
            length,
            acknowledged: self.acknowledged,
            packet_length: self.length,
        };
        if length == 0 || length > u8::MAX as usize {
            return Err(invalid());
        }
        let end = usize::from(self.acknowledged)
            .checked_add(length)
            .ok_or_else(invalid)?;
        if end > usize::from(self.length) {
            return Err(invalid());
        }
        let end = u8::try_from(end)
            .ok()
            .and_then(NonZeroU8::new)
            .ok_or_else(invalid)?;
        self.in_flight_end = Some(end);
        Ok(())
    }

    /// Apply a radio completion; `false` retains the same fragment for retransmission.
    pub fn observe_fragment_completion(&mut self, acknowledged: bool) {
        if acknowledged && let Some(end) = self.in_flight_end.take() {
            self.acknowledged = u16::from(end.get());
        }
    }

    /// Whether every LL fragment of this one HCI packet was acknowledged.
    pub const fn is_complete(&self) -> bool {
        self.acknowledged == self.length
    }
}

/// Owned standard Number Of Completed Packets event for the sole LE handle.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LeNumberOfCompletedPacketsEvent {
    bytes: [u8; LE_NUMBER_OF_COMPLETED_PACKETS_EVENT_CAPACITY],
}

impl LeNumberOfCompletedPacketsEvent {
    /// Report a nonzero batch of returned Host-to-Controller packet credits.
    pub fn new(handle: ConnHandle, completed: u16) -> Result<Self, HciBuildError> {
        if completed == 0 {
            return Err(HciBuildError::ZeroCompletedPackets);
        }
        let handle = handle.into_inner().to_le_bytes();
        let completed = completed.to_le_bytes();
        Ok(Self {
            bytes: [
                0x13,
                0x05,
                0x01,
                handle[0],
                handle[1],
                completed[0],
                completed[1],
            ],
        })
    }

    /// Complete HCI Event body without an H4 indicator.
    pub const fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }
}

impl HciControllerResponse for LeNumberOfCompletedPacketsEvent {
    fn kind(&self) -> PacketKind {
        PacketKind::Event
    }

    fn as_bytes(&self) -> &[u8] {
        self.as_bytes()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invalid_fragment_admission_retains_payload_and_credit_state() {
        let data = [0x5a; 60];
        let mut owned = LeHostAclPacket::copy_from(
            packet(AclPacketBoundary::FirstNonFlushable, &data),
            Some(ConnHandle::new(1)),
            251,
        )
        .unwrap();

        for length in [0, 61, 256, usize::MAX] {
            let before = owned.clone();
            assert_eq!(
                owned.fragment_enqueued(length),
                Err(HciBuildError::InvalidAclFragmentLength {
                    length,
                    acknowledged: 0,
                    packet_length: 60,
                })
            );
            assert_eq!(owned, before);
            assert_eq!(owned.next_fragment(usize::MAX).unwrap().payload(), data);
        }

        owned.fragment_enqueued(27).unwrap();
        let before = owned.clone();
        assert_eq!(
            owned.fragment_enqueued(1),
            Err(HciBuildError::AclFragmentInFlight { end: 27 })
        );
        assert_eq!(owned, before);
        owned.observe_fragment_completion(false);
        assert_eq!(owned, before);
        owned.observe_fragment_completion(true);
        let before = owned.clone();
        assert_eq!(
            owned.fragment_enqueued(34),
            Err(HciBuildError::InvalidAclFragmentLength {
                length: 34,
                acknowledged: 27,
                packet_length: 60,
            })
        );
        assert_eq!(owned, before);
        assert_eq!(
            owned.next_fragment(usize::MAX).unwrap().payload(),
            &data[27..]
        );
        owned.fragment_enqueued(33).unwrap();
        owned.observe_fragment_completion(true);
        assert!(owned.is_complete());
        assert_eq!(
            owned.fragment_enqueued(1),
            Err(HciBuildError::InvalidAclFragmentLength {
                length: 1,
                acknowledged: 60,
                packet_length: 60,
            })
        );
    }

    #[test]
    fn completion_events_reject_zero_and_preserve_the_full_credit_count() {
        assert_eq!(
            LeNumberOfCompletedPacketsEvent::new(ConnHandle::new(1), 0),
            Err(HciBuildError::ZeroCompletedPackets)
        );
        assert_eq!(
            LeNumberOfCompletedPacketsEvent::new(ConnHandle::new(1), u16::MAX)
                .unwrap()
                .as_bytes(),
            [0x13, 5, 1, 1, 0, 0xff, 0xff]
        );
    }

    #[test]
    fn controller_acl_rejects_a_forged_out_of_domain_handle_before_fragmentation() {
        for handle in [0x0f00, 0x0fff, u16::MAX] {
            assert!(LeControllerAclPacket::copy_from_ll(ConnHandle(handle), false, &[1]).is_none());
        }
        let owned = LeControllerAclPacket::copy_from_ll(ConnHandle(0x0eff), false, &[1]).unwrap();
        assert_eq!(owned.handle(), ConnHandle(0x0eff));
        assert_eq!(owned.next_host_fragment(0, 1), Some(owned));
    }

    #[test]
    fn controller_acl_fragment_bounds_preserve_the_entire_admitted_payload() {
        let data = [0x5a; LE_ACL_DATA_PACKET_CAPACITY];
        let owned = LeControllerAclPacket::copy_from_ll(ConnHandle::new(1), false, &data).unwrap();
        assert_eq!(owned.payload_length(), data.len());
        assert_eq!(owned.as_bytes().len(), LE_CONTROLLER_ACL_PACKET_CAPACITY);
        assert_eq!(owned.next_host_fragment(0, usize::MAX), Some(owned));
        assert!(owned.next_host_fragment(usize::MAX, 1).is_none());
        assert_eq!(
            owned
                .next_host_fragment(250, usize::MAX)
                .unwrap()
                .payload_length(),
            1
        );
        assert!(
            LeControllerAclPacket::copy_from_ll(ConnHandle::new(1), false, &[0; 252]).is_none()
        );
    }

    fn packet<'a>(boundary: AclPacketBoundary, data: &'a [u8]) -> AclPacket<'a> {
        AclPacket::new(
            ConnHandle::new(1),
            boundary,
            AclBroadcastFlag::PointToPoint,
            data,
        )
    }

    #[test]
    fn one_hci_credit_survives_fragment_retransmission_until_final_ack() {
        let data = [0x5a; 60];
        let mut packet = LeHostAclPacket::copy_from(
            packet(AclPacketBoundary::FirstNonFlushable, &data),
            Some(ConnHandle::new(1)),
            251,
        )
        .unwrap();

        let first = packet.next_fragment(27).unwrap();
        assert!(!first.is_continuing());
        assert_eq!(first.payload(), &data[..27]);
        packet.fragment_enqueued(first.payload().len()).unwrap();
        packet.observe_fragment_completion(false);
        assert!(packet.next_fragment(27).is_none());
        packet.observe_fragment_completion(true);

        let second = packet.next_fragment(27).unwrap();
        assert!(second.is_continuing());
        assert_eq!(second.payload(), &data[27..54]);
        packet.fragment_enqueued(second.payload().len()).unwrap();
        packet.observe_fragment_completion(true);

        let last = packet.next_fragment(27).unwrap();
        assert!(last.is_continuing());
        assert_eq!(last.payload(), &data[54..]);
        packet.fragment_enqueued(last.payload().len()).unwrap();
        packet.observe_fragment_completion(true);
        assert!(packet.is_complete());
        assert!(packet.next_fragment(27).is_none());
    }

    #[test]
    fn continuation_hci_packet_never_mints_a_new_ll_start() {
        let data = [1; 30];
        let mut packet = LeHostAclPacket::copy_from(
            packet(AclPacketBoundary::Continuing, &data),
            Some(ConnHandle::new(1)),
            251,
        )
        .unwrap();
        let first = packet.next_fragment(27).unwrap();
        assert!(first.is_continuing());
        packet.fragment_enqueued(27).unwrap();
        packet.observe_fragment_completion(true);
        assert!(packet.next_fragment(27).unwrap().is_continuing());
    }

    #[test]
    fn rejected_packets_cannot_cross_handle_or_profile_boundaries() {
        let data = [0; 28];
        assert_eq!(
            LeHostAclPacket::copy_from(
                packet(AclPacketBoundary::FirstNonFlushable, &data),
                Some(ConnHandle::new(2)),
                251,
            ),
            Err(LeHostAclPacketRejection::UnknownConnectionIdentifier)
        );
        assert_eq!(
            LeHostAclPacket::copy_from(
                packet(AclPacketBoundary::FirstNonFlushable, &data),
                Some(ConnHandle::new(1)),
                27,
            ),
            Err(LeHostAclPacketRejection::PayloadTooLong {
                length: 28,
                capacity: 27,
            })
        );
        assert_eq!(
            LeHostAclPacket::copy_from(
                packet(AclPacketBoundary::FirstFlushable, &data),
                Some(ConnHandle::new(1)),
                251,
            ),
            Err(LeHostAclPacketRejection::UnsupportedPacketBoundary)
        );
    }

    #[test]
    fn completion_event_is_the_standard_single_handle_shape() {
        assert_eq!(
            LeNumberOfCompletedPacketsEvent::new(ConnHandle::new(1), 4)
                .unwrap()
                .as_bytes(),
            [0x13, 0x05, 0x01, 0x01, 0x00, 0x04, 0x00]
        );
    }

    #[test]
    fn controller_acl_packet_preserves_ll_boundary_and_payload() {
        use bt_hci::ControllerToHostPacket;

        for (continuing, expected) in [
            (false, AclPacketBoundary::FirstFlushable),
            (true, AclPacketBoundary::Continuing),
        ] {
            let owned =
                LeControllerAclPacket::copy_from_ll(ConnHandle::new(1), continuing, &[1, 2, 3])
                    .unwrap();
            let (decoded, remaining) = ControllerToHostPacket::from_hci_bytes_with_kind(
                PacketKind::AclData,
                owned.as_bytes(),
            )
            .unwrap();
            assert!(remaining.is_empty());
            let ControllerToHostPacket::Acl(packet) = decoded else {
                panic!("ACL kind must decode as Controller-to-Host ACL")
            };
            assert_eq!(packet.handle(), ConnHandle::new(1));
            assert_eq!(packet.boundary_flag(), expected);
            assert_eq!(packet.data(), [1, 2, 3]);
        }
        assert!(LeControllerAclPacket::copy_from_ll(ConnHandle::new(1), false, &[]).is_none());
    }

    #[test]
    fn controller_acl_packet_fragments_to_the_host_buffer_boundary() {
        use bt_hci::ControllerToHostPacket;

        let owned =
            LeControllerAclPacket::copy_from_ll(ConnHandle::new(1), false, &[1, 2, 3, 4, 5])
                .unwrap();
        for (offset, expected_boundary, expected_data) in [
            (0, AclPacketBoundary::FirstFlushable, &[1, 2][..]),
            (2, AclPacketBoundary::Continuing, &[3, 4][..]),
            (4, AclPacketBoundary::Continuing, &[5][..]),
        ] {
            let fragment = owned.next_host_fragment(offset, 2).unwrap();
            let (ControllerToHostPacket::Acl(decoded), remaining) =
                ControllerToHostPacket::from_hci_bytes_with_kind(
                    PacketKind::AclData,
                    fragment.as_bytes(),
                )
                .unwrap()
            else {
                panic!("Host fragment changed HCI packet kind")
            };
            assert!(remaining.is_empty());
            assert_eq!(decoded.boundary_flag(), expected_boundary);
            assert_eq!(decoded.data(), expected_data);
        }
        assert!(owned.next_host_fragment(5, 2).is_none());
        assert!(owned.next_host_fragment(0, 0).is_none());
    }

    #[test]
    fn host_completed_packets_name_each_handle_with_its_count() {
        let command = LeHostCompletedPacketsCommand::decode(crate::HciCommandPacket::new(
            LeHostCompletedPacketsCommand::OPCODE,
            &[2, 1, 0, 2, 0, 2, 0, 3, 0],
        ))
        .unwrap();
        assert_eq!(
            command.pairs().collect::<std::vec::Vec<_>>(),
            [(ConnHandle::new(1), 2), (ConnHandle::new(2), 3)]
        );

        let empty = LeHostCompletedPacketsCommand::decode(crate::HciCommandPacket::new(
            LeHostCompletedPacketsCommand::OPCODE,
            &[0],
        ))
        .unwrap();
        assert_eq!(empty.pairs().count(), 0);

        for parameters in [&[][..], &[1, 1, 0, 1][..], &[1, 0, 0x10, 1, 0][..]] {
            assert_eq!(
                LeHostCompletedPacketsCommand::decode(crate::HciCommandPacket::new(
                    LeHostCompletedPacketsCommand::OPCODE,
                    parameters,
                )),
                Err(LeHostCompletedPacketsDecodeError::Malformed)
            );
        }

        use bt_hci::{
            FromHciBytes,
            event::{CommandCompleteWithStatus, Event},
        };
        let invalid = LeHostCompletedPacketsErrorEvent::invalid_parameters();
        let Event::CommandComplete(error) =
            Event::from_hci_bytes_complete(invalid.as_bytes()).unwrap()
        else {
            panic!("invalid credits require Command Complete")
        };
        assert_eq!(error.cmd_opcode, LeHostCompletedPacketsCommand::OPCODE);
        assert_eq!(
            CommandCompleteWithStatus::try_from(error).unwrap().status,
            HciError::INVALID_HCI_PARAMETERS.to_status()
        );
    }
}
