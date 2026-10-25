//! The B0 serial boundary accepts complete H4 command packets, one at a time.
use oer_bluetooth_hci::HciCommandPacket;
use oer_time::{Duration, Instant};

/// An unfinished command expires after this inter-byte silence.
pub const FRAME_GAP: Duration = Duration::from_millis(100);

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
    deadline: Option<Instant>,
}
impl Commands {
    pub const fn new() -> Self {
        Self {
            bytes: [0; MAXIMUM],
            used: 0,
            expected: None,
            receiving: false,
            deadline: None,
        }
    }
    pub fn complete(&self) -> bool {
        self.expected == Some(self.used)
    }
    /// No byte of a new packet can replace an undelivered complete command.
    pub fn push(&mut self, byte: u8, now: Instant) -> Result<bool, Error> {
        if self.complete() {
            return Err(Error::Unconsumed);
        }
        self.deadline = Some(now.saturating_add(FRAME_GAP));
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
    /// Check before reading the FIFO, so a fresh H4 kind is never consumed
    /// as part of the frame that expired. Complete commands never expire.
    pub fn expire(&mut self, now: Instant) -> bool {
        if self.receiving && !self.complete() && self.deadline.is_some_and(|d| now >= d) {
            self.reset();
            return true;
        }
        false
    }
    /// When the unfinished command expires, if one is being received.
    pub fn deadline(&self) -> Option<Instant> {
        if self.receiving && !self.complete() {
            self.deadline
        } else {
            None
        }
    }
    pub fn reset(&mut self) {
        self.used = 0;
        self.expected = None;
        self.receiving = false;
        self.deadline = None;
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
        self.reset();
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
            assert_eq!(commands.push(byte, Instant::EPOCH), Ok(false));
            assert_eq!(commands.packet(), Err(Error::Incomplete));
        }
        assert_eq!(commands.push(0, Instant::EPOCH), Ok(true));
        let packet = commands.packet().expect("complete reset");
        assert_eq!(packet.opcode(), Reset::OPCODE);
        assert!(packet.parameters().is_empty());
    }
    #[test]
    fn maximum_command_keeps_all_parameter_bytes() {
        let mut commands = Commands::new();
        for byte in [1, 0, 252, 255] {
            assert_eq!(commands.push(byte, Instant::EPOCH), Ok(false));
        }
        for value in 0..255_u8 {
            assert_eq!(commands.push(value, Instant::EPOCH), Ok(value == 254));
        }
        let packet = commands.packet().expect("maximum command");
        assert_eq!(packet.parameters().len(), 255);
        assert!(packet.parameters().iter().copied().eq(0..255_u8));
    }
    #[test]
    fn a_complete_command_cannot_be_overwritten_by_the_next_frame() {
        let mut commands = Commands::new();
        for byte in [1, 3, 12, 0] {
            commands.push(byte, Instant::EPOCH).expect("reset bytes");
        }
        assert_eq!(commands.push(1, Instant::EPOCH), Err(Error::Unconsumed));
        assert_eq!(
            commands.packet().expect("retained reset").opcode(),
            Reset::OPCODE
        );
        commands.consume().expect("semantic token copied");
        assert_eq!(commands.push(1, Instant::EPOCH), Ok(false));
    }
    #[test]
    fn unsupported_packet_kind_is_a_reported_boundary_error() {
        let mut commands = Commands::new();
        assert_eq!(commands.push(2, Instant::EPOCH), Err(Error::PacketKind));
        commands.reset();
        for byte in [1, 3, 12, 0] {
            commands
                .push(byte, Instant::EPOCH)
                .expect("Reset after stray byte");
        }
        assert_eq!(
            commands.packet().expect("retained Reset").opcode(),
            Reset::OPCODE
        );
    }
    #[test]
    fn premature_consumption_does_not_destroy_a_partial_header() {
        let mut commands = Commands::new();
        for byte in [1, 3] {
            commands.push(byte, Instant::EPOCH).expect("partial header");
        }
        assert_eq!(commands.consume(), Err(Error::Incomplete));
        assert_eq!(commands.push(12, Instant::EPOCH), Ok(false));
        assert_eq!(commands.push(0, Instant::EPOCH), Ok(true));
        assert_eq!(
            commands.packet().expect("retained header").opcode(),
            Reset::OPCODE
        );
    }
    #[test]
    fn broken_header_expires_and_the_next_reset_survives() {
        use oer_time::Clock;
        let clock = oer_time_virtual::VirtualClock::<1>::new();
        let mut commands = Commands::new();
        for byte in [1, 3, 12] {
            commands.push(byte, clock.now()).expect("partial Reset");
        }
        assert!(!commands.expire(clock.now()));
        clock.advance(FRAME_GAP).expect("representable gap");
        assert!(commands.expire(clock.now()));
        assert!(!commands.expire(clock.now()), "one error per broken frame");
        for byte in [1, 3, 12, 0] {
            commands.push(byte, clock.now()).expect("fresh Reset");
        }
        assert_eq!(
            commands.packet().expect("fresh packet").opcode(),
            Reset::OPCODE
        );
    }
}
