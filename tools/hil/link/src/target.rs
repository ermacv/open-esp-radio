//! What the link needs from the stand: the serial line it opened.

/// A serial line the stand opened for the link. The stand's board I/O
/// (`oer_lab_board::port::Port`) is the one opener of every serial line;
/// the link only reads and writes what it is handed, and takes the
/// descriptor over for its nonblocking reader.
pub trait SerialLine: std::io::Read + std::io::Write + std::os::fd::AsRawFd + Send {
    /// Hand the open descriptor over.
    fn into_owned_descriptor(self: Box<Self>) -> std::os::fd::OwnedFd;
}

impl<T> SerialLine for T
where
    T: std::io::Read + std::io::Write + std::os::fd::AsRawFd + Into<std::os::fd::OwnedFd> + Send,
{
    fn into_owned_descriptor(self: Box<Self>) -> std::os::fd::OwnedFd {
        (*self).into()
    }
}
