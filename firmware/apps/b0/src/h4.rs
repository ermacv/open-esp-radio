//! The B0 serial boundary accepts complete H4 command packets, one at a time.
//! The frame codec is `oer_bluetooth_hci::h4`; this module adds the silence
//! after which an unfinished command is dropped.
use oer_bluetooth_hci::{
    HciCommandPacket,
    h4::{CommandReceiver, Error},
};
use oer_time::{Duration, Instant};

/// An unfinished command expires after this inter-byte silence.
pub const FRAME_GAP: Duration = Duration::from_millis(100);

pub struct Commands {
    frame: CommandReceiver,
    deadline: Option<Instant>,
}
impl Commands {
    pub const fn new() -> Self {
        Self {
            frame: CommandReceiver::new(),
            deadline: None,
        }
    }
    pub fn complete(&self) -> bool {
        self.frame.complete()
    }
    /// No byte of a new packet can replace an undelivered complete command.
    pub fn push(&mut self, byte: u8, now: Instant) -> Result<bool, Error> {
        let complete = self.frame.push(byte)?;
        self.deadline = Some(now.saturating_add(FRAME_GAP));
        Ok(complete)
    }
    /// Check before reading the FIFO, so a fresh H4 kind is never consumed
    /// as part of the frame that expired. Complete commands never expire.
    pub fn expire(&mut self, now: Instant) -> bool {
        if self.deadline().is_some_and(|d| now >= d) {
            self.reset();
            return true;
        }
        false
    }
    /// When the unfinished command expires, if one is being received.
    pub fn deadline(&self) -> Option<Instant> {
        if self.frame.started() && !self.frame.complete() {
            self.deadline
        } else {
            None
        }
    }
    pub fn reset(&mut self) {
        self.frame.reset();
        self.deadline = None;
    }
    pub fn packet(&self) -> Result<HciCommandPacket<'_>, Error> {
        self.frame.packet()
    }
    /// Consume only after the Controller copied the command's semantic token.
    pub fn consume(&mut self) -> Result<(), Error> {
        self.frame.consume()?;
        self.deadline = None;
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
    use oer_bluetooth_hci::{Cmd, Reset};

    #[test]
    fn a_command_is_published_through_the_hci_codec() {
        let mut commands = Commands::new();
        for byte in [1, 3, 12] {
            assert_eq!(commands.push(byte, Instant::EPOCH), Ok(false));
            assert_eq!(commands.packet().err(), Some(Error::Incomplete));
        }
        assert_eq!(commands.push(0, Instant::EPOCH), Ok(true));
        assert_eq!(
            commands.packet().expect("complete reset").opcode(),
            Reset::OPCODE
        );
        assert_eq!(commands.push(1, Instant::EPOCH), Err(Error::Unconsumed));
        commands.consume().expect("semantic token copied");
        assert_eq!(commands.push(1, Instant::EPOCH), Ok(false));
    }
    #[test]
    fn a_stray_byte_arms_no_deadline() {
        let mut commands = Commands::new();
        assert!(commands.push(2, Instant::EPOCH).is_err());
        assert_eq!(commands.deadline(), None);
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
