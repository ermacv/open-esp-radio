//! The vendor IEEE 802.15.4 reference firmware (`ieee802154-reference` of
//! the chip's vendor calibration projects) on the board under test, driven
//! through the IEEE 802.15.4 reference peer's line protocol (its README in
//! `hil/peers`): the radio configured and
//! receiving, optionally after one driver disable and enable, then its
//! register state read with `PEEK` and `ANALOG`. Replies are rewritten in
//! the calibration firmware's line formats, so the comparison reads both
//! vendor firmwares alike.
use crate::Result;
use oer_vendor_phy_calibration_capture::space::{self, Register, Space, analog_parts};
use std::io::{Read, Write as _};
use std::time::{Duration, Instant};

/// The line that opens every boot of the reference firmware.
const READY: &str = "@READY";
/// Longest wait for a boot's ready line or one command's reply.
const REPLY_TIMEOUT: Duration = Duration::from_secs(20);

/// The radio configuration both sides run: channel 15 at 21 dBm, the
/// power the session's default PIB transmits at.
pub(crate) const CHANNEL: u8 = 15;
pub(crate) const POWER_DBM: i8 = 21;
pub(crate) const PAN_ID: u16 = 0x1234;
pub(crate) const SHORT_ADDRESS: u16 = 0x0001;
pub(crate) const EXTENDED_ADDRESS: [u8; 8] = [0x02, 0x4f, 0x45, 0x52, 0x00, 0x00, 0x00, 0x01];

/// The frame a transmitting point sends: a data frame without an
/// acknowledgement request (frame control 0x8841), sequence zero, from the
/// configured short address to the broadcast address of the configured PAN,
/// with a one-byte payload.
const TRANSMIT_FRAME_CONTROL: u16 = 0x8841;
const BROADCAST: u16 = 0xffff;
const TRANSMIT_PAYLOAD: u8 = 0;

fn transmit_command() -> String {
    let bytes: Vec<u8> = TRANSMIT_FRAME_CONTROL
        .to_le_bytes()
        .into_iter()
        .chain([0])
        .chain(PAN_ID.to_le_bytes())
        .chain(BROADCAST.to_le_bytes())
        .chain(SHORT_ADDRESS.to_le_bytes())
        .chain([TRANSMIT_PAYLOAD])
        .collect();
    let frame: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    format!("TX 0 {frame}\n")
}

fn configuration() -> String {
    let extended: String = EXTENDED_ADDRESS
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    format!("CFG {CHANNEL} {PAN_ID:04x} {SHORT_ADDRESS:04x} {extended} 0 {POWER_DBM}\n")
}

/// One console session with the reference firmware.
pub(crate) struct Peer {
    serial: oer_lab_board::port::Port,
    /// Everything the firmware printed.
    pub(crate) console: String,
    /// Whether the radio is taken through one disable and enable before
    /// the reads.
    restart: bool,
}

impl Peer {
    /// The firmware of a board just reset through `serial`, once ready.
    pub(crate) fn boot(
        serial: oer_lab_board::port::Port,
        restart: bool,
    ) -> Result<Self> {
        let mut peer = Self {
            serial,
            console: String::new(),
            restart,
        };
        peer.wait(|console| ready_lines(console) >= 1, "the ready line")?;
        peer.prepare()?;
        Ok(peer)
    }

    /// Configure the radio and receive, after one disable and enable of
    /// the driver when the point restarts it.
    fn prepare(&mut self) -> Result<()> {
        let mut commands = vec![configuration(), "RX\n".into()];
        if self.restart {
            commands.extend([
                "OFF\n".into(),
                "ON\n".into(),
                configuration(),
                "RX\n".into(),
            ]);
        }
        for command in commands {
            let name = command
                .split_whitespace()
                .next()
                .unwrap_or_default()
                .to_owned();
            let done = ok_lines(&self.console, &name);
            self.serial.write_all(command.as_bytes())?;
            self.wait(|console| ok_lines(console, &name) > done, &name)?;
        }
        Ok(())
    }

    /// Transmit one frame without CCA and wait for its completion.
    pub(crate) fn transmit(&mut self) -> Result<()> {
        let done = transmissions(&self.console);
        self.serial.write_all(transmit_command().as_bytes())?;
        self.wait(
            |console| transmissions(console) > done,
            "transmit completion",
        )?;
        match complete_lines(&self.console)
            .filter(|l| l.starts_with("@TXDONE") || l.starts_with("@TXFAIL"))
            .last()
        {
            Some(line) if line.starts_with("@TXFAIL") => {
                Err(format!("the reference firmware failed to transmit: {line}").into())
            }
            _ => Ok(()),
        }
    }

    fn read_some(&mut self) -> Result<()> {
        let mut buffer = [0; 256];
        match self.serial.read(&mut buffer) {
            Ok(read) => self
                .console
                .push_str(&String::from_utf8_lossy(&buffer[..read])),
            Err(error) if error.kind() == std::io::ErrorKind::TimedOut => {}
            Err(error) => return Err(error.into()),
        }
        if let Some(line) = self.console.lines().find(|l| l.starts_with("@ERR")) {
            return Err(format!("the reference firmware refused: {line}").into());
        }
        Ok(())
    }

    fn wait(&mut self, done: impl Fn(&str) -> bool, what: &str) -> Result<()> {
        let started = Instant::now();
        while !done(&self.console) {
            if started.elapsed() > REPLY_TIMEOUT {
                return Err(format!("no {what} from the reference firmware").into());
            }
            self.read_some()?;
        }
        Ok(())
    }

    /// The values of `registers` in `space`, in the calibration firmware's
    /// line format. A read that resets the chip shows as a new ready line,
    /// and one that hangs it is ended by a board reset: the register is
    /// recorded as unreadable, and the radio is prepared again before the
    /// reads continue.
    pub(crate) fn registers(&mut self, space: Space, registers: &[Register]) -> Result<String> {
        let mut lines = String::new();
        for register in registers {
            let request = match space {
                Space::Mmio => format!("PEEK {:08x}\n", register.address),
                Space::Analog => {
                    let (block, reg) = analog_parts(register.address);
                    format!("ANALOG {block:02x} {reg:02x}\n")
                }
            };
            let boots = ready_lines(&self.console);
            let answered = replies(&self.console, space).len();
            self.serial.write_all(request.as_bytes())?;
            let started = Instant::now();
            loop {
                let values = replies(&self.console, space);
                if values.len() > answered {
                    let (address, value) = values[answered];
                    if address != register.address {
                        return Err(format!(
                            "the reference firmware answered {address:#x} for {}",
                            register.name
                        )
                        .into());
                    }
                    lines.push_str(&space.line(address, value));
                    break;
                }
                if ready_lines(&self.console) > boots {
                    lines.push_str(
                        &oer_vendor_phy_calibration_capture::vendor::unreadable_line(
                            register.address,
                        ),
                    );
                    self.prepare()?;
                    break;
                }
                if started.elapsed() > REPLY_TIMEOUT {
                    // A read that stalls the bus hangs the chip instead of
                    // resetting it: reset the board and treat it alike.
                    oer_lab_board::reset::reset_usb_serial_jtag(&mut self.serial)?;
                    self.wait(|console| ready_lines(console) > boots, "the ready line")?;
                    lines.push_str(
                        &oer_vendor_phy_calibration_capture::vendor::unreadable_line(
                            register.address,
                        ),
                    );
                    self.prepare()?;
                    break;
                }
                self.read_some()?;
            }
        }
        Ok(lines)
    }
}

fn complete_lines(console: &str) -> impl Iterator<Item = &str> {
    let complete = console.rfind('\n').map_or("", |end| &console[..end]);
    complete.lines().map(str::trim)
}

fn ready_lines(console: &str) -> usize {
    complete_lines(console)
        .filter(|l| l.starts_with(READY))
        .count()
}

fn transmissions(console: &str) -> usize {
    complete_lines(console)
        .filter(|l| l.starts_with("@TXDONE") || l.starts_with("@TXFAIL"))
        .count()
}

fn ok_lines(console: &str, command: &str) -> usize {
    let expected = format!("@OK {command}");
    complete_lines(console).filter(|l| *l == expected).count()
}

/// Every complete `PEEK` or `ANALOG` reply of `console`, in order, as
/// (address, value).
fn replies(console: &str, space: Space) -> Vec<(u32, u32)> {
    complete_lines(console)
        .filter_map(|line| {
            let fields: Vec<&str> = line.split(' ').collect();
            match (space, fields.as_slice()) {
                (Space::Mmio, ["@PEEK", address, value]) => Some((
                    u32::from_str_radix(address, 16).ok()?,
                    u32::from_str_radix(value, 16).ok()?,
                )),
                (Space::Analog, ["@ANALOG", block, reg, value]) => Some((
                    space::analog_address(
                        u8::from_str_radix(block, 16).ok()?,
                        u8::from_str_radix(reg, 16).ok()?,
                    ),
                    u32::from_str_radix(value, 16).ok()?,
                )),
                _ => None,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replies_are_read_in_order_and_boots_counted() {
        let console = "@READY protocol=1 target=chip-a\n@OK CFG\n@PEEK 20100434 0000abcd\n\
                       @ANALOG 61 09 0e\n@PEEK 20100438 00000001\n@PEEK 2010";
        assert_eq!(
            replies(console, Space::Mmio),
            [(0x2010_0434, 0xabcd), (0x2010_0438, 1)]
        );
        assert_eq!(
            replies(console, Space::Analog),
            [(space::analog_address(0x61, 0x09), 0x0e)]
        );
        assert_eq!(ready_lines(console), 1);
        assert_eq!(ok_lines(console, "CFG"), 1);
        assert!(configuration().starts_with("CFG 15 1234 0001 024f4552"));
        assert_eq!(transmit_command(), "TX 0 4188003412ffff010000\n");
        assert_eq!(transmissions("@TXDONE ack=-\n@TXFAIL 3\n@TXDONE"), 2);
    }
}
