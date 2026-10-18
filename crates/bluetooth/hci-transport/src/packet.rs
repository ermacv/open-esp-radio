//! One retained HCI packet body and its typed kind.

use bt_hci::PacketKind;

#[derive(Clone, Copy)]
pub(crate) struct PacketSlot<const PACKET_CAPACITY: usize> {
    pub(crate) kind: PacketKind,
    pub(crate) length: usize,
    pub(crate) bytes: [u8; PACKET_CAPACITY],
}

impl<const PACKET_CAPACITY: usize> PacketSlot<PACKET_CAPACITY> {
    pub(crate) const EMPTY: Self = Self {
        kind: PacketKind::Event,
        length: 0,
        bytes: [0; PACKET_CAPACITY],
    };
}
