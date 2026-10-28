//! The standard LE Rand command and its completion.

use bt_hci::{
    cmd::{Cmd, Opcode, le::LeRand},
    param::{Error as HciError, Status},
};

/// Validated parameterless LE Rand request, separate from software bootstrap.
#[derive(Debug)]
pub struct LeRandCommand(pub(crate) ());

impl LeRandCommand {
    /// Standard Bluetooth opcode, owned by the typed HCI command definition.
    pub const OPCODE: Opcode = LeRand::OPCODE;
}

/// Complete LE Rand response retained across output backpressure.
///
/// Debug output intentionally excludes random material. Failure responses
/// contain zeroed, invalid return octets, never partial entropy.
#[derive(Clone, Copy, Eq, PartialEq)]
pub struct LeRandCommandCompleteEvent {
    bytes: [u8; 14],
}

impl core::fmt::Debug for LeRandCommandCompleteEvent {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("LeRandCommandCompleteEvent")
            .field("status", &self.bytes[5])
            .finish_non_exhaustive()
    }
}

impl LeRandCommandCompleteEvent {
    /// Successful completion carrying `random`.
    pub fn success(random: [u8; 8]) -> Self {
        Self::new(Status::SUCCESS, random)
    }

    /// Failed completion with zeroed return octets.
    pub fn error(error: HciError) -> Self {
        Self::new(error.to_status(), [0; 8])
    }

    fn new(status: Status, random: [u8; 8]) -> Self {
        let [low, high] = LeRandCommand::OPCODE.to_raw().to_le_bytes();
        let [a, b, c, d, e, f, g, h] = random;
        let bytes = [
            0x0e,
            12,
            1,
            low,
            high,
            status.into_inner(),
            a,
            b,
            c,
            d,
            e,
            f,
            g,
            h,
        ];
        Self { bytes }
    }

    /// Complete HCI Event without an H4 indicator.
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }
}

impl crate::HciControllerResponse for LeRandCommandCompleteEvent {
    fn kind(&self) -> bt_hci::PacketKind {
        bt_hci::PacketKind::Event
    }
    fn as_bytes(&self) -> &[u8] {
        self.as_bytes()
    }
}
