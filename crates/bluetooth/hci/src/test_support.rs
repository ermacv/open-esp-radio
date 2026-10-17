//! Synchronous HCI packet encoding for codec tests.

use bt_hci::{PacketKind, transport::PacketToController};

use crate::HciCommandPacket;

/// Serialize a typed Host command and decode it as the Controller sees it.
pub(crate) fn command_packet<'buffer, T: PacketToController>(
    command: &T,
    buffer: &'buffer mut [u8],
) -> HciCommandPacket<'buffer> {
    assert_eq!(T::KIND, PacketKind::Cmd, "only commands reach the codecs");
    let capacity = buffer.len();
    let mut remaining = &mut buffer[..];
    command
        .write_hci(&mut remaining)
        .expect("the command fits the test buffer");
    let written = capacity - remaining.len();
    HciCommandPacket::from_hci_bytes(&buffer[..written]).expect("bt-hci writes a complete command")
}
