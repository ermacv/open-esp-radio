//! A board's USB Serial/JTAG port: opening it without a reset, resetting the
//! chip into its application and reading its console lines.

use std::{
    io::Read as _,
    path::Path,
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

use serialport::{SerialPort as _, TTYPort};

use crate::Result;

pub const BAUD: u32 = 115_200;
/// How long a blocking console read waits.
const READ_TIMEOUT: Duration = Duration::from_millis(100);
/// How long RTS holds the chip in reset.
const RESET_HOLD: Duration = Duration::from_millis(100);

/// Open `path` and leave the lines as the open sets them.
pub fn open_kept(path: &Path, timeout: Duration) -> Result<TTYPort> {
    serialport::new(path.to_string_lossy(), BAUD)
        .timeout(timeout)
        .open_native()
        .map_err(|error| format!("open {}: {error}", path.display()).into())
}

/// Open the console at `path` without resetting the chip: RTS is released
/// before DTR, so the lines never pass through the reset-with-boot-strap
/// state an open with both asserted would leave on release.
pub fn open_without_reset(path: &Path) -> Result<TTYPort> {
    let mut port = open_kept(path, READ_TIMEOUT)?;
    port.write_request_to_send(false)?;
    port.write_data_terminal_ready(false)?;
    Ok(port)
}

/// Reset the chip behind `port` into its application, discarding what the
/// previous boot sent: DTR (the boot strap) released, then an RTS pulse.
/// The chip restarts on RTS's rising edge, so old input is drained before it.
pub fn reset_into_application(port: &mut TTYPort) -> Result<()> {
    port.write_data_terminal_ready(false)?;
    thread::sleep(RESET_HOLD);
    port.clear(serialport::ClearBuffer::Input)?;
    port.write_request_to_send(true)?;
    thread::sleep(RESET_HOLD);
    port.write_request_to_send(false)?;
    Ok(())
}

/// The lines a console receives, from a reader thread that stops when this
/// is dropped.
pub struct Lines {
    receiver: Option<mpsc::Receiver<Vec<u8>>>,
    reader: Option<thread::JoinHandle<()>>,
}

impl Lines {
    /// The next line within `timeout`; `None` on a timeout, `Err` once the
    /// port is gone.
    pub fn next(&self, timeout: Duration) -> std::result::Result<Option<Vec<u8>>, ()> {
        let receiver = self.receiver.as_ref().ok_or(())?;
        let deadline = Instant::now() + timeout;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            match receiver.recv_timeout(remaining) {
                Ok(line) if line.is_empty() && !remaining.is_zero() => {}
                Ok(line) if line.is_empty() => return Ok(None),
                Ok(line) => return Ok(Some(line)),
                Err(mpsc::RecvTimeoutError::Timeout) => return Ok(None),
                Err(mpsc::RecvTimeoutError::Disconnected) => return Err(()),
            }
        }
    }
}

impl Drop for Lines {
    fn drop(&mut self) {
        drop(self.receiver.take());
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
    }
}

/// The lines `port` receives, without their line ends. An empty message only
/// keeps the reader polling and is no line.
pub fn lines(mut port: TTYPort) -> Lines {
    let (sender, receiver) = mpsc::channel();
    let reader = thread::spawn(move || {
        let mut pending = Vec::new();
        let mut buffer = [0; 4096];
        loop {
            match port.read(&mut buffer) {
                Ok(read) => pending.extend_from_slice(buffer.get(..read).unwrap_or_default()),
                Err(error) if error.kind() == std::io::ErrorKind::TimedOut => {}
                Err(_) => return,
            }
            for line in split_lines(&mut pending) {
                if sender.send(line).is_err() {
                    return;
                }
            }
            if sender.send(Vec::new()).is_err() {
                return;
            }
        }
    });
    Lines {
        receiver: Some(receiver),
        reader: Some(reader),
    }
}

/// Take every complete line out of `pending`, without `\n` or `\r\n`.
pub fn split_lines(pending: &mut Vec<u8>) -> Vec<Vec<u8>> {
    let mut lines = Vec::new();
    while let Some(end) = pending.iter().position(|&byte| byte == b'\n') {
        let mut line = pending.drain(..=end).collect::<Vec<_>>();
        line.pop();
        if line.last() == Some(&b'\r') {
            line.pop();
        }
        lines.push(line);
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn complete_lines_are_split_and_the_rest_is_kept() {
        let mut pending = b"first\r\nsecond\nthi".to_vec();
        assert_eq!(
            split_lines(&mut pending),
            [b"first".to_vec(), b"second".to_vec()]
        );
        assert_eq!(pending, b"thi");
        pending.extend_from_slice(b"rd\r\n");
        assert_eq!(split_lines(&mut pending), [b"third".to_vec()]);
        assert!(pending.is_empty());
    }
}
