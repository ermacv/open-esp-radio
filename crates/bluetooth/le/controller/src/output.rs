//! Bounded queue of complete Controller-to-Host packets.

use bt_hci::PacketKind;
use oer_bluetooth_hci::INITIAL_CONTROLLER_TO_HOST_PACKET_CAPACITY;

/// Largest Controller-to-Host packet body.
pub const HCI_PACKET_CAPACITY: usize = INITIAL_CONTROLLER_TO_HOST_PACKET_CAPACITY;

/// One complete Controller-to-Host packet without an H4 indicator.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HciPacket {
    kind: PacketKind,
    length: u16,
    bytes: [u8; HCI_PACKET_CAPACITY],
}

impl HciPacket {
    const EMPTY: Self = Self {
        kind: PacketKind::Event,
        length: 0,
        bytes: [0; HCI_PACKET_CAPACITY],
    };

    /// Packet class.
    pub const fn kind(&self) -> PacketKind {
        self.kind
    }

    /// Complete packet body.
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes[..usize::from(self.length)]
    }
}

/// FIFO of packets waiting for the Host transport.
///
/// One slot is kept for the response to the command in progress; events that
/// arrive while only that slot is free are dropped and counted.
pub(crate) struct Output<const CAPACITY: usize> {
    slots: [HciPacket; CAPACITY],
    head: usize,
    len: usize,
    dropped: u32,
}

impl<const CAPACITY: usize> Output<CAPACITY> {
    pub(crate) const fn new() -> Self {
        assert!(
            CAPACITY >= 2,
            "the output needs a response slot and an event slot"
        );
        Self {
            slots: [HciPacket::EMPTY; CAPACITY],
            head: 0,
            len: 0,
            dropped: 0,
        }
    }

    pub(crate) const fn has_response_slot(&self) -> bool {
        self.len < CAPACITY
    }

    pub(crate) const fn dropped(&self) -> u32 {
        self.dropped
    }

    /// Free slots, the response slot included.
    pub(crate) const fn free(&self) -> usize {
        CAPACITY - self.len
    }

    /// Queue Controller-to-Host ACL data; the caller checked the room.
    pub(crate) fn push_acl(&mut self, bytes: &[u8]) {
        assert!(
            self.len + 1 < CAPACITY && self.push(PacketKind::AclData, bytes),
            "ACL data leaves the response slot free"
        );
    }

    /// Queue a command response; the caller admitted the command only while a
    /// slot was free.
    pub(crate) fn push_response(&mut self, bytes: &[u8]) {
        assert!(
            self.push(PacketKind::Event, bytes),
            "a response slot is reserved"
        );
    }

    /// Queue an asynchronous event, keeping the response slot free.
    pub(crate) fn push_event(&mut self, bytes: &[u8]) {
        if self.len + 1 >= CAPACITY || !self.push(PacketKind::Event, bytes) {
            self.dropped = self.dropped.saturating_add(1);
        }
    }

    fn push(&mut self, kind: PacketKind, bytes: &[u8]) -> bool {
        if self.len == CAPACITY || bytes.len() > HCI_PACKET_CAPACITY {
            return false;
        }
        let slot = &mut self.slots[(self.head + self.len) % CAPACITY];
        slot.kind = kind;
        slot.length = bytes.len() as u16;
        slot.bytes[..bytes.len()].copy_from_slice(bytes);
        self.len += 1;
        true
    }

    pub(crate) fn front(&self) -> Option<&HciPacket> {
        (self.len > 0).then(|| &self.slots[self.head])
    }

    pub(crate) fn pop(&mut self) {
        if self.len > 0 {
            self.head = (self.head + 1) % CAPACITY;
            self.len -= 1;
        }
    }
}
