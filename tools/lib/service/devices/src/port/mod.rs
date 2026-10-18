//! A board's serial port: the one opener of every serial line the stand
//! uses, for consoles, resets, flash writes, the DUT session and peer
//! consoles.
//!
//! A [`Port`] is opened by path with its [`Settings`]: line rate, read
//! timeout, how the modem lines are left at open, and how long an open is
//! retried while the device is still busy or returning. It speaks no
//! protocol: what is written and read, and which reset sequence the lines
//! perform, belongs to its caller (the reset, console and
//! flash modules, the DUT link and the peer console).
//! Open errors retain the typed [`serialport::Error`] beneath their path
//! context, including errors before a protocol connection is established.

use std::{
    io,
    os::fd::{AsRawFd, IntoRawFd, RawFd},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use serialport::SerialPort as _;

/// How the modem lines are left when a port opens.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Lines {
    /// As the open leaves them: a caller that drives a reset sequence or
    /// attaches to a running target sets them itself.
    Kept,
    /// RTS released before DTR, so the lines never pass through the
    /// reset-with-boot-strap state: opening a USB serial port with the
    /// default lines resets an Espressif chip, and a UART bridge's lines
    /// drive EN and BOOT.
    Released,
}

/// How a port is opened.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Settings {
    /// Line rate in baud.
    pub baud: u32,
    /// How long a blocking read waits for a byte.
    pub timeout: Duration,
    /// The modem lines after the open.
    pub lines: Lines,
    /// How long a failed open is tried again: a port the reader of a
    /// previous session still holds, or one that is returning after a
    /// reset, opens a moment later.
    pub busy: Duration,
}

impl Settings {
    /// A board console: 115200 baud, 200 ms reads, lines released, one
    /// attempt.
    pub const CONSOLE: Self = Self {
        baud: 115_200,
        timeout: Duration::from_millis(200),
        lines: Lines::Released,
        busy: Duration::ZERO,
    };

    /// These settings with `timeout` for reads.
    #[must_use]
    pub const fn timeout(self, timeout: Duration) -> Self {
        Self { timeout, ..self }
    }

    /// These settings with the lines left as `lines`.
    #[must_use]
    pub const fn lines(self, lines: Lines) -> Self {
        Self { lines, ..self }
    }

    /// These settings, retrying a failed open for `busy`.
    #[must_use]
    pub const fn busy(self, busy: Duration) -> Self {
        Self { busy, ..self }
    }
}

/// Pause between attempts to open a busy port.
const BUSY_RETRY: Duration = Duration::from_millis(20);

#[derive(Debug)]
struct OpenFailure {
    path: PathBuf,
    source: serialport::Error,
}

impl std::fmt::Display for OpenFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "open {}: {}", self.path.display(), self.source)
    }
}

impl std::error::Error for OpenFailure {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.source)
    }
}

/// An open serial port.
pub struct Port {
    path: PathBuf,
    serial: serialport::TTYPort,
}

impl std::fmt::Debug for Port {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Port")
            .field("path", &self.path)
            .finish_non_exhaustive()
    }
}

impl Port {
    /// Open the port at `path` with `settings`.
    pub fn open(path: &Path, settings: Settings) -> io::Result<Self> {
        let deadline = Instant::now() + settings.busy;
        let serial = loop {
            match serialport::new(path.to_string_lossy(), settings.baud)
                .timeout(settings.timeout)
                .open_native()
            {
                Ok(serial) => break serial,
                Err(_) if Instant::now() < deadline => std::thread::sleep(BUSY_RETRY),
                Err(error) => {
                    return Err(io::Error::new(
                        io::Error::from(error.clone()).kind(),
                        OpenFailure {
                            path: path.to_owned(),
                            source: error,
                        },
                    ));
                }
            }
        };
        let mut port = Self {
            path: path.to_owned(),
            serial,
        };
        if settings.lines == Lines::Released {
            port.set_rts(false)?;
            port.set_dtr(false)?;
        }
        Ok(port)
    }

    /// The path the port was opened at.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Drive RTS (`true` asserts it).
    pub fn set_rts(&mut self, level: bool) -> io::Result<()> {
        Ok(self.serial.write_request_to_send(level)?)
    }

    /// Drive DTR (`true` asserts it).
    pub fn set_dtr(&mut self, level: bool) -> io::Result<()> {
        Ok(self.serial.write_data_terminal_ready(level)?)
    }

    /// Discard what was received and not read yet.
    pub fn clear_input(&mut self) -> io::Result<()> {
        Ok(self.serial.clear(serialport::ClearBuffer::Input)?)
    }

    /// Wait at most `timeout` for a blocking read.
    pub fn set_timeout(&mut self, timeout: Duration) -> io::Result<()> {
        Ok(self.serial.set_timeout(timeout)?)
    }

    /// The serial port itself, for espflash's connection.
    pub fn into_native(self) -> serialport::TTYPort {
        self.serial
    }
}

impl io::Read for Port {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        self.serial.read(buffer)
    }
}

impl io::Write for Port {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.serial.write(bytes)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.serial.flush()
    }
}

impl AsRawFd for Port {
    fn as_raw_fd(&self) -> RawFd {
        self.serial.as_raw_fd()
    }
}

impl IntoRawFd for Port {
    fn into_raw_fd(self) -> RawFd {
        self.serial.into_raw_fd()
    }
}

/// Retry `open` for `within`: a port that just returned, or that the reader
/// of a previous reset still holds, opens a moment later.
pub fn retrying<T, E>(
    within: Duration,
    mut open: impl FnMut() -> std::result::Result<T, E>,
) -> std::result::Result<T, E> {
    let deadline = std::time::Instant::now() + within;
    loop {
        match open() {
            Err(_) if std::time::Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(20));
            }
            result => return result,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_compose_from_the_console_defaults() {
        let settings = Settings::CONSOLE
            .timeout(Duration::from_millis(50))
            .lines(Lines::Kept)
            .busy(Duration::from_secs(2));
        assert_eq!(settings.baud, 115_200);
        assert_eq!(settings.timeout, Duration::from_millis(50));
        assert_eq!(settings.lines, Lines::Kept);
        assert_eq!(settings.busy, Duration::from_secs(2));
        assert_eq!(Settings::CONSOLE.lines, Lines::Released);
    }

    #[test]
    fn a_missing_port_fails_after_its_busy_window_naming_the_path() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("ttyACM-missing");
        let started = Instant::now();
        let error = Port::open(&path, Settings::CONSOLE.busy(Duration::from_millis(100)))
            .err()
            .unwrap();
        assert!(started.elapsed() >= Duration::from_millis(100));
        assert!(error.to_string().contains("ttyACM-missing"), "{error}");
        assert_eq!(
            error
                .get_ref()
                .unwrap()
                .source()
                .unwrap()
                .downcast_ref::<serialport::Error>()
                .unwrap()
                .kind(),
            serialport::ErrorKind::Io(io::ErrorKind::NotFound),
        );
        let started = Instant::now();
        assert!(Port::open(&path, Settings::CONSOLE).is_err());
        assert!(started.elapsed() < Duration::from_millis(100));
    }
}
