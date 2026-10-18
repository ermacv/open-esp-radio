//! What a session needs from its cell: the board under test and the serial
//! line the stand opens for it.

use std::path::Path;

/// A serial line the stand opened for the link. The stand's board I/O
/// (`oer_devices::port::Port`) is the one opener of every serial line;
/// the link only reads and writes what it is handed, and takes a capture's
/// descriptor over for its nonblocking reader.
pub trait SerialLine: std::io::Read + std::io::Write + std::os::fd::AsRawFd + Send {
    /// Hand the open descriptor over.
    fn into_raw_descriptor(self: Box<Self>) -> std::os::fd::RawFd;
}

impl<T> SerialLine for T
where
    T: std::io::Read + std::io::Write + std::os::fd::AsRawFd + std::os::fd::IntoRawFd + Send,
{
    fn into_raw_descriptor(self: Box<Self>) -> std::os::fd::RawFd {
        (*self).into_raw_fd()
    }
}

/// Opens a board's console for a capture, on the capture's worker once the
/// capture owns its output.
pub type ConsoleOpener = Box<dyn FnOnce() -> std::io::Result<Box<dyn SerialLine>> + Send>;

/// The board under test as the link reaches it. The stand implements it for
/// the board it leased.
pub trait Dut {
    /// The board's console port.
    fn console(&self) -> &Path;

    /// Open the board's console and restart its application through it, so
    /// a capture sees the boot from its first byte.
    fn console_with_reset(&self) -> ConsoleOpener;
}
