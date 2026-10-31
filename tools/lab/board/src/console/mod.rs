//! A board's console: the one serial line reader of the stand's commands,
//! bounded reads, and the ROM's reset line.
//!
//! A console is a [`Port`] opened through `crate::reset`'s openers only,
//! which never reset the chip on their own.

use crate::port::Port;
use std::{
    io::Read as _,
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

/// Everything `serial` receives within `watch`, as text.
pub fn read_for(serial: &mut Port, watch: Duration) -> String {
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
                if oer_lab_model::rom::is_reset_line(&text) {
                    return Some(text);
                }
            }
            Err(_) => return None,
        }
    }
    None
}
