//! A board's console: the one serial line reader of the stand's commands,
//! bounded captures, and the ROM's reset line.
//!
//! A console is a [`Port`] opened through `oer_devices::reset`'s openers only,
//! which never reset the chip on their own.

use crate::port::Port;
use std::{
    io::{Read as _, Write as _},
    path::Path,
    sync::mpsc,
    time::{Duration, Instant},
};

/// The lines a console receives. Dropping it stops its reader and waits
/// until the reader has closed the port, so the port never outlives it (nor
/// the device lock its owner holds beside it).
pub struct Lines {
    receiver: Option<mpsc::Receiver<Vec<u8>>>,
    reader: Option<std::thread::JoinHandle<()>>,
}

impl std::ops::Deref for Lines {
    type Target = mpsc::Receiver<Vec<u8>>;

    fn deref(&self) -> &Self::Target {
        self.receiver.as_ref().expect("present until dropped")
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

/// The lines `serial` receives, until the [`Lines`] are dropped. An empty
/// message only keeps the reader alive and is no line.
pub fn lines(mut serial: Port) -> Lines {
    let (lines, received) = mpsc::channel();
    let reader = std::thread::spawn(move || {
        let mut pending = Vec::new();
        let mut buffer = [0; 4096];
        loop {
            match serial.read(&mut buffer) {
                Ok(0) => {}
                Ok(read) => pending.extend_from_slice(&buffer[..read]),
                Err(error) if error.kind() == std::io::ErrorKind::TimedOut => {}
                Err(_) => return,
            }
            while let Some(end) = pending.iter().position(|&byte| byte == b'\n') {
                let mut line = pending.drain(..=end).collect::<Vec<_>>();
                line.pop();
                if line.last() == Some(&b'\r') {
                    line.pop();
                }
                if lines.send(line).is_err() {
                    return;
                }
            }
            // A dropped receiver is noticed at the next line; stop polling an
            // idle console once nobody listens.
            if lines.send(Vec::new()).is_err() {
                return;
            }
        }
    });
    Lines {
        receiver: Some(received),
        reader: Some(reader),
    }
}

/// Copy `lines` to the terminal and `log` until `duration` passes or a line
/// contains `until`; returns whether one did.
pub fn capture(
    lines: &mpsc::Receiver<Vec<u8>>,
    duration: Duration,
    until: Option<&str>,
    log: &Path,
) -> crate::Result<bool> {
    let mut file = std::fs::File::create(log)?;
    let deadline = Instant::now() + duration;
    while let Some(remaining) = deadline.checked_duration_since(Instant::now()) {
        match lines.recv_timeout(remaining) {
            Ok(line) if line.is_empty() => {}
            Ok(line) => {
                file.write_all(&line)?;
                file.write_all(b"\n")?;
                let text = String::from_utf8_lossy(&line);
                println!("{text}");
                if until.is_some_and(|until| text.contains(until)) {
                    return Ok(true);
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout | mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
    Ok(false)
}

/// Everything `serial` receives within `watch`, as text.
pub fn read_for(mut serial: Port, watch: Duration) -> String {
    let started = Instant::now();
    let mut console = Vec::new();
    let mut buffer = [0_u8; 1024];
    while started.elapsed() < watch {
        match serial.read(&mut buffer) {
            Ok(read) => console.extend_from_slice(&buffer[..read]),
            Err(error) if error.kind() == std::io::ErrorKind::TimedOut => {}
            Err(_) => break,
        }
    }
    String::from_utf8_lossy(&console).into_owned()
}

/// The ROM's `rst:… boot:…` line among `lines` within `within`.
pub fn rom_line(lines: &mpsc::Receiver<Vec<u8>>, within: Duration) -> Option<String> {
    let deadline = Instant::now() + within;
    while let Some(remaining) = deadline.checked_duration_since(Instant::now()) {
        match lines.recv_timeout(remaining) {
            Ok(line) => {
                let text = String::from_utf8_lossy(&line).trim().to_owned();
                if is_reset_line(&text) {
                    return Some(text);
                }
            }
            Err(_) => return None,
        }
    }
    None
}

/// The ROM's last `rst:… boot:…` line in `banner`.
pub fn reset_line(banner: &str) -> Option<&str> {
    banner
        .lines()
        .map(str::trim)
        .rfind(|line| is_reset_line(line))
}

/// Whether `line` is a ROM reset line: a ROM that answers, booting from
/// flash or waiting for a download.
pub fn is_reset_line(line: &str) -> bool {
    line.starts_with("rst:") && line.contains("boot:")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines_of(text: &[&str], keep_open: Duration) -> mpsc::Receiver<Vec<u8>> {
        let (sender, received) = mpsc::channel();
        let text = text
            .iter()
            .map(|line| line.as_bytes().to_vec())
            .collect::<Vec<_>>();
        std::thread::spawn(move || {
            for line in text {
                sender.send(Vec::new()).unwrap();
                sender.send(line).unwrap();
            }
            std::thread::sleep(keep_open);
        });
        received
    }

    #[test]
    fn the_capture_ends_at_the_expected_line_or_its_deadline() {
        let directory = tempfile::tempdir().unwrap();
        let log = directory.path().join("console.log");
        let started = Instant::now();
        let seen = capture(
            &lines_of(&["boot", "READY 1", "later"], Duration::from_secs(30)),
            Duration::from_secs(20),
            Some("READY"),
            &log,
        )
        .unwrap();
        assert!(seen);
        assert!(started.elapsed() < Duration::from_secs(10));
        assert_eq!(std::fs::read_to_string(&log).unwrap(), "boot\nREADY 1\n");

        let started = Instant::now();
        let seen = capture(
            &lines_of(&["boot"], Duration::from_secs(30)),
            Duration::from_millis(300),
            Some("READY"),
            &log,
        )
        .unwrap();
        assert!(!seen);
        assert!(started.elapsed() < Duration::from_secs(10));
        assert_eq!(std::fs::read_to_string(&log).unwrap(), "boot\n");
    }

    #[test]
    fn the_rom_reset_line_is_found_in_its_banner_and_among_lines() {
        let banner = "ESP-ROM:chip-b-eco2-20250121\r\nBuild:Jan 21 2025\r\n\
                      rst:0x1 (POWERON),boot:0x18 (SPI_FAST_FLASH_BOOT)\r\nSPI mode:DIO\r\n";
        assert_eq!(
            reset_line(banner),
            Some("rst:0x1 (POWERON),boot:0x18 (SPI_FAST_FLASH_BOOT)")
        );
        assert_eq!(reset_line("garbage"), None);
        let lines = lines_of(
            &[
                "ESP-ROM:chip-b-eco2-20250121",
                "rst:0x15 (USB_UART_HPSYS),boot:0x18 (SPI_FAST_FLASH_BOOT)",
            ],
            Duration::from_secs(1),
        );
        assert_eq!(
            rom_line(&lines, Duration::from_secs(1)).as_deref(),
            Some("rst:0x15 (USB_UART_HPSYS),boot:0x18 (SPI_FAST_FLASH_BOOT)")
        );
        assert_eq!(
            rom_line(
                &lines_of(&["app"], Duration::ZERO),
                Duration::from_millis(200)
            ),
            None
        );
    }
}
