//! The B0 serial boundary accepts complete H4 command packets, one at a time.
use oer_bluetooth_hci::HciCommandPacket;

const COMMAND: u8 = 1;
const HEADER: usize = 3;
const MAXIMUM: usize = HEADER + u8::MAX as usize;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error {
    PacketKind,
    Unconsumed,
    Incomplete,
    Length,
}
pub struct Commands {
    bytes: [u8; MAXIMUM],
    used: usize,
    expected: Option<usize>,
    receiving: bool,
}
impl Commands {
    pub const fn new() -> Self {
        Self {
            bytes: [0; MAXIMUM],
            used: 0,
            expected: None,
            receiving: false,
        }
    }
    pub fn complete(&self) -> bool {
        self.expected == Some(self.used)
    }
    /// No byte of a new packet can replace an undelivered complete command.
    pub fn push(&mut self, byte: u8) -> Result<bool, Error> {
        if self.complete() {
            return Err(Error::Unconsumed);
        }
        if !self.receiving {
            if byte != COMMAND {
                return Err(Error::PacketKind);
            }
            self.receiving = true;
            return Ok(false);
        }
        let place = self.bytes.get_mut(self.used).ok_or(Error::Length)?;
        *place = byte;
        self.used += 1;
        if self.used == HEADER {
            self.expected = Some(HEADER + usize::from(self.bytes[HEADER - 1]));
        }
        Ok(self.complete())
    }
    pub fn packet(&self) -> Result<HciCommandPacket<'_>, Error> {
        if !self.complete() {
            return Err(Error::Incomplete);
        }
        let bytes = self.bytes.get(..self.used).ok_or(Error::Length)?;
        HciCommandPacket::from_hci_bytes(bytes).map_err(|_| Error::Length)
    }
    /// Consume only after the Controller copied the command's semantic token.
    pub fn consume(&mut self) -> Result<(), Error> {
        if !self.complete() {
            return Err(Error::Incomplete);
        }
        self.used = 0;
        self.expected = None;
        self.receiving = false;
        Ok(())
    }
}
impl Default for Commands {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use oer_bluetooth_hci::bt_hci::cmd::{Cmd, controller_baseband::Reset};

    #[test]
    fn fragmented_reset_is_published_only_after_its_complete_header() {
        let mut commands = Commands::new();
        for byte in [1, 3, 12] {
            assert_eq!(commands.push(byte), Ok(false));
            assert_eq!(commands.packet(), Err(Error::Incomplete));
        }
        assert_eq!(commands.push(0), Ok(true));
        let packet = commands.packet().expect("complete reset");
        assert_eq!(packet.opcode(), Reset::OPCODE);
        assert!(packet.parameters().is_empty());
    }
    #[test]
    fn maximum_command_keeps_all_parameter_bytes() {
        let mut commands = Commands::new();
        for byte in [1, 0, 252, 255] {
            assert_eq!(commands.push(byte), Ok(false));
        }
        for value in 0..255_u8 {
            assert_eq!(commands.push(value), Ok(value == 254));
        }
        let packet = commands.packet().expect("maximum command");
        assert_eq!(packet.parameters().len(), 255);
        assert!(packet.parameters().iter().copied().eq(0..255_u8));
    }
    #[test]
    fn a_complete_command_cannot_be_overwritten_by_the_next_frame() {
        let mut commands = Commands::new();
        for byte in [1, 3, 12, 0] {
            commands.push(byte).expect("reset bytes");
        }
        assert_eq!(commands.push(1), Err(Error::Unconsumed));
        assert_eq!(
            commands.packet().expect("retained reset").opcode(),
            Reset::OPCODE
        );
        commands.consume().expect("semantic token copied");
        assert_eq!(commands.push(1), Ok(false));
    }
    #[test]
    fn unsupported_packet_kind_is_a_reported_boundary_error() {
        let mut commands = Commands::new();
        assert_eq!(commands.push(2), Err(Error::PacketKind));
        assert_eq!(commands.push(1), Ok(false));
        assert_eq!(commands.packet(), Err(Error::Incomplete));
    }
    #[test]
    fn premature_consumption_does_not_destroy_a_partial_header() {
        let mut commands = Commands::new();
        for byte in [1, 3] {
            commands.push(byte).expect("partial header");
        }
        assert_eq!(commands.consume(), Err(Error::Incomplete));
        assert_eq!(commands.push(12), Ok(false));
        assert_eq!(commands.push(0), Ok(true));
        assert_eq!(
            commands.packet().expect("retained header").opcode(),
            Reset::OPCODE
        );
    }
}
