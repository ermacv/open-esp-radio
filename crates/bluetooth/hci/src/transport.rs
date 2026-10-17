//! Complete Controller-to-Host packet validation.

use bt_hci::{FromHciBytesError, PacketKind};

mod packet;

/// Why a Controller-to-Host packet body is not one complete packet.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ControllerToHostPacketError {
    /// A Host-to-Controller command does not travel this direction.
    InvalidDirection,
    /// `bt-hci` rejected the packet header or declared payload.
    InvalidPacket(FromHciBytesError),
    /// A valid packet was followed by bytes outside its declared HCI length.
    TrailingBytes,
}

/// Validate one complete Controller-to-Host packet body of `kind`.
///
/// Commands travel the other direction; every other packet must decode
/// through `bt-hci` and end exactly at the length its header declares.
pub fn validate_controller_to_host_packet(
    kind: PacketKind,
    bytes: &[u8],
) -> Result<(), ControllerToHostPacketError> {
    packet::validate_complete_packet(kind, bytes)
}

#[cfg(test)]
mod tests;
