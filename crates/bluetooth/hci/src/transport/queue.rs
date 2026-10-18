//! Synchronous Controller-to-Host FIFO with transactional packet admission.

use super::packet::{PacketSlot, decode_complete_packet, validate_complete_packet};
use bt_hci::{ControllerToHostPacket, FromHciBytesError, PacketKind};

/// A failed publication or receive operation on the bounded HCI queue.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ControllerToHostQueueError {
    /// Every statically allocated queue slot is occupied.
    Full,
    /// No packet is available to receive.
    Empty,
    /// A Host-to-Controller command cannot enter this directional queue.
    InvalidDirection,
    /// The packet body exceeds this queue profile's fixed storage.
    PacketTooLong {
        /// Supplied packet length.
        length: usize,
        /// Maximum packet length retained by this queue.
        capacity: usize,
    },
    /// The destination cannot retain the complete oldest packet.
    DestinationTooSmall {
        /// Required destination length.
        required: usize,
        /// Supplied destination length.
        available: usize,
    },
    /// `bt-hci` rejected the packet header or declared payload.
    InvalidPacket(FromHciBytesError),
    /// A valid packet was followed by bytes outside its declared HCI length.
    TrailingBytes,
    /// Retained storage no longer decodes to the packet admitted at publish.
    ///
    /// Safe code cannot create this state. The variant keeps future hardware
    /// adapters fail-closed if a storage invariant is ever violated.
    CorruptRetainedPacket,
}

/// Fixed-capacity FIFO from the sole Controller owner to one HCI Host reader.
///
/// Publication validates a complete `bt-hci` packet before changing queue
/// state. A full queue never overwrites an older event or ACL packet. Receive
/// similarly retains the oldest slot if the caller's buffer is too small, so
/// cancellation or retry cannot silently consume controller state.
pub struct ControllerToHostQueue<const DEPTH: usize, const PACKET_CAPACITY: usize> {
    slots: [PacketSlot<PACKET_CAPACITY>; DEPTH],
    head: usize,
    length: usize,
}

impl<const DEPTH: usize, const PACKET_CAPACITY: usize>
    ControllerToHostQueue<DEPTH, PACKET_CAPACITY>
{
    /// Construct an empty queue with no allocator or runtime registration.
    pub const fn new() -> Self {
        assert!(DEPTH > 0, "an HCI queue needs at least one packet slot");
        assert!(
            PACKET_CAPACITY > 0,
            "an HCI queue packet slot must retain at least one byte"
        );
        Self {
            slots: [PacketSlot::EMPTY; DEPTH],
            head: 0,
            length: 0,
        }
    }

    /// Number of packets currently retained for the Host.
    pub const fn len(&self) -> usize {
        self.length
    }

    /// Whether no packet is currently retained for the Host.
    pub const fn is_empty(&self) -> bool {
        self.length == 0
    }

    /// Number of statically allocated packet slots.
    pub const fn capacity(&self) -> usize {
        DEPTH
    }

    /// Length of the oldest packet without borrowing its contents.
    pub const fn front_len(&self) -> Option<usize> {
        if self.length == 0 {
            None
        } else {
            Some(self.slots[self.head].length)
        }
    }

    /// Validate and publish one complete packet emitted by the Controller.
    pub fn publish(
        &mut self,
        kind: PacketKind,
        bytes: &[u8],
    ) -> Result<(), ControllerToHostQueueError> {
        if kind == PacketKind::Cmd {
            return Err(ControllerToHostQueueError::InvalidDirection);
        }
        if bytes.len() > PACKET_CAPACITY {
            return Err(ControllerToHostQueueError::PacketTooLong {
                length: bytes.len(),
                capacity: PACKET_CAPACITY,
            });
        }
        validate_complete_packet(kind, bytes)?;
        if self.length == DEPTH {
            return Err(ControllerToHostQueueError::Full);
        }

        let tail = (self.head + self.length) % DEPTH;
        let slot = &mut self.slots[tail];
        slot.kind = kind;
        slot.length = bytes.len();
        slot.bytes[..bytes.len()].copy_from_slice(bytes);
        self.length += 1;
        Ok(())
    }

    /// Copy and decode the oldest packet into caller-owned Host read storage.
    ///
    /// The slot is consumed only after the complete packet has been copied and
    /// decoded. Cleared slot bytes cannot retain an earlier event or key-bearing
    /// control payload across a later controller epoch.
    pub fn receive<'buffer>(
        &mut self,
        buffer: &'buffer mut [u8],
    ) -> Result<ControllerToHostPacket<'buffer>, ControllerToHostQueueError> {
        if self.length == 0 {
            return Err(ControllerToHostQueueError::Empty);
        }

        let slot = &mut self.slots[self.head];
        if buffer.len() < slot.length {
            return Err(ControllerToHostQueueError::DestinationTooSmall {
                required: slot.length,
                available: buffer.len(),
            });
        }

        let packet_length = slot.length;
        let kind = slot.kind;
        buffer[..packet_length].copy_from_slice(&slot.bytes[..packet_length]);
        let packet = decode_complete_packet(kind, &buffer[..packet_length])
            .map_err(|_| ControllerToHostQueueError::CorruptRetainedPacket)?;

        slot.bytes[..packet_length].fill(0);
        slot.length = 0;
        self.head = (self.head + 1) % DEPTH;
        self.length -= 1;
        Ok(packet)
    }
}

impl<const DEPTH: usize, const PACKET_CAPACITY: usize> Default
    for ControllerToHostQueue<DEPTH, PACKET_CAPACITY>
{
    fn default() -> Self {
        Self::new()
    }
}
