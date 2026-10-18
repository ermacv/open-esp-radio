//! The one flash writer: an image bundle's [`Snapshot`] (its flash contents
//! read once into owned bytes) through one espflash connection to the ROM
//! stub, for the chip its profile names.
//!
//! Each segment whose flash contents already match is skipped after an MD5
//! comparison, so an unchanged bootloader or partition table costs one
//! checksum. Segments are written in the bundle's order, which puts the OTA
//! selection last: an interrupted write leaves the previous selection
//! pointing at an image whose checksum no longer validates instead of a
//! half-written one. A failure of the serial link is retried; a failure of
//! the image is not. Transport errors retain their typed serialport or espflash
//! cause through port context, so connection failures are retried and diagnostics
//! include the cause. Serialport reports POSIX EIO before connection as
//! `ErrorKind::Unknown`; these port failures are retried within the same bound.
//! Unclassified application `io::ErrorKind::Other` errors are not retried.
//! The serial port starts with a three-second timeout as espflash
//! requires before its first connection handshake.
//! `write_bins_to_flash` also performs the configured post-write reset. The
//! writer leaves that transition to espflash; repeating it would send a stub
//! command to a chip that has already returned to the ROM loader.

use std::{
    error::Error,
    fmt,
    path::{Path, PathBuf},
    time::Duration,
};

use oer_image_bundle::Snapshot;

/// Where the writer leaves the chip after the last segment.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum After {
    /// Reset into the written image.
    HardReset,
    /// Stay in the ROM's download mode.
    StayInBootloader,
}

impl After {
    /// What a write followed by `start` leaves to the start.
    pub const fn of(start: oer_chip_profile::Start) -> Self {
        match start {
            oer_chip_profile::Start::Reset => Self::HardReset,
            oer_chip_profile::Start::PowerOn => Self::StayInBootloader,
        }
    }
}

/// Write the bytes of `snapshot` into the flash of the `chip` (its espflash
/// name) board at `port` through one connection, retrying a failed link.
pub fn write(port: &Path, chip: &str, snapshot: &Snapshot, after: After) -> crate::Result<()> {
    let chip = espflash_chip(chip)?;
    with_retries(port, || write_once(port, chip, snapshot, after))
}

/// The espflash name of the chip at `port`, from its ROM; the chip is reset
/// into its application afterwards.
pub fn detect(port: &Path) -> crate::Result<String> {
    with_retries(port, || {
        let mut flasher = connect(port, After::HardReset, None)?;
        let chip = flasher.chip();
        flasher.connection().reset_after(true, chip)?;
        Ok(chip.to_string())
    })
}

fn connect(
    port: &Path,
    after: After,
    chip: Option<espflash::target::Chip>,
) -> crate::Result<espflash::flasher::Flasher> {
    use espflash::{
        connection::{Connection, ResetAfterOperation, ResetBeforeOperation},
        flasher::Flasher,
    };
    use serialport::SerialPortType;

    let port_name = port.to_string_lossy().into_owned();
    let usb = serialport::available_ports()?
        .into_iter()
        .find(|info| {
            info.port_name == port_name
                || std::fs::canonicalize(&info.port_name).ok() == std::fs::canonicalize(port).ok()
        })
        .and_then(|info| match info.port_type {
            SerialPortType::UsbPort(usb) => Some(usb),
            _ => None,
        })
        .ok_or_else(|| format!("{port_name} is not an attached USB serial port"))?;
    let serial = crate::port::Port::open(
        port,
        crate::port::Settings::CONSOLE
            .lines(crate::port::Lines::Kept)
            // Flasher::connect calls Connection::begin before setting its own
            // timeout; reset banner reads and flushes need a bounded wait too.
            .timeout(Duration::from_secs(3)),
    )?
    .into_native();
    let connection = Connection::new(
        serial,
        usb,
        match after {
            After::HardReset => ResetAfterOperation::HardReset,
            After::StayInBootloader => ResetAfterOperation::NoReset,
        },
        ResetBeforeOperation::DefaultReset,
        115_200,
    );
    Ok(Flasher::connect(connection, true, false, true, chip, None)?)
}

fn write_once(
    port: &Path,
    chip: espflash::target::Chip,
    snapshot: &Snapshot,
    after: After,
) -> crate::Result<()> {
    use espflash::{image_format::Segment as Region, target::DefaultProgressCallback};

    let context = |operation, source| -> Box<dyn Error + Send + Sync> {
        Box::new(FlashFailure {
            port: port.to_owned(),
            operation,
            source,
        })
    };
    let mut flasher =
        connect(port, after, Some(chip)).map_err(|source| context("connect ROM loader", source))?;
    let regions = snapshot
        .segments
        .iter()
        .map(|segment| Region::new(segment.offset, &segment.data))
        .collect::<Vec<_>>();
    flasher
        .write_bins_to_flash(&regions, &mut DefaultProgressCallback)
        .map_err(|source| context("write segments", source.into()))?;
    Ok(())
}

/// espflash's chip named `name` (a chip profile's `espflash-chip`).
pub fn espflash_chip(name: &str) -> crate::Result<espflash::target::Chip> {
    <espflash::target::Chip as std::str::FromStr>::from_str(name)
        .map_err(|_| format!("espflash knows no chip `{name}`").into())
}

/// How many times a write is attempted when espflash's link to the chip
/// fails on its way.
pub const ATTEMPTS: usize = 3;

/// Run `write` again when espflash's link to the chip failed on its way;
/// each retry is reported on stderr, which a job's log keeps.
fn with_retries<T>(port: &Path, mut write: impl FnMut() -> crate::Result<T>) -> crate::Result<T> {
    retry_transient(ATTEMPTS, &mut write, |attempt, error| {
        eprintln!(
            "flash attempt {attempt} of {ATTEMPTS} through {} failed: {error}; retrying",
            port.display()
        );
    })
}

fn retry_transient<T>(
    attempts: usize,
    operation: &mut impl FnMut() -> crate::Result<T>,
    mut between: impl FnMut(usize, &str),
) -> crate::Result<T> {
    let mut attempt = 1;
    loop {
        match operation() {
            Ok(value) => return Ok(value),
            Err(error) if attempt < attempts && transient_failure(error.as_ref()) => {
                between(attempt, &error.to_string());
                attempt += 1;
            }
            Err(error) => return Err(error),
        }
    }
}

/// Keep the port context without erasing espflash's typed failure and cause.
#[derive(Debug)]
struct FlashFailure {
    port: PathBuf,
    operation: &'static str,
    source: Box<dyn Error + Send + Sync>,
}

impl fmt::Display for FlashFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "flash through {} ({}): {}",
            self.port.display(),
            self.operation,
            self.source
        )?;
        let mut cause = self.source.source();
        while let Some(error) = cause {
            write!(formatter, ": {error}")?;
            cause = error.source();
        }
        Ok(())
    }
}

impl Error for FlashFailure {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(self.source.as_ref())
    }
}

/// Connection and transport failures can clear on reconnect. Image and
/// configuration failures must keep their original error without a retry.
fn transient_failure(mut error: &(dyn Error + 'static)) -> bool {
    loop {
        if let Some(error) = error.downcast_ref::<espflash::Error>() {
            return matches!(
                error,
                espflash::Error::Connection(_) | espflash::Error::Flashing(_)
            );
        }
        // serialport reports POSIX EIO during open as Unknown, before espflash
        // can wrap it as Connection. Retry that typed port failure rather than
        // every io::ErrorKind::Other or a platform-specific diagnostic string.
        if let Some(error) = error.downcast_ref::<serialport::Error>()
            && error.kind() == serialport::ErrorKind::Unknown
        {
            return true;
        }
        if let Some(error) = error.downcast_ref::<std::io::Error>()
            && matches!(
                error.kind(),
                std::io::ErrorKind::TimedOut
                    | std::io::ErrorKind::WouldBlock
                    | std::io::ErrorKind::BrokenPipe
                    | std::io::ErrorKind::ConnectionReset
                    | std::io::ErrorKind::ConnectionAborted
                    | std::io::ErrorKind::Interrupted
                    | std::io::ErrorKind::UnexpectedEof
            )
        {
            return true;
        }
        match error.source() {
            Some(cause) => error = cause,
            None => return false,
        }
    }
}

#[cfg(test)]
mod tests;
