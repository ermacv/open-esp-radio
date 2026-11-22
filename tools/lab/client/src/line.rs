//! A console the service runs, as a local socket: the serial line a HIL
//! link (`oer-tools-hil-link`) reads and writes. The harness's devices
//! under test and `cargo fw run` of a HIL image both open their link on it.

use std::{
    io::{Read, Write},
    os::unix::net::UnixStream,
};

use crate::client::Running;

/// A local socket the link reads and writes as its serial line, joined to
/// the service's console operation: closing the link's end cancels it.
pub fn bridge(console: Running) -> std::io::Result<UnixStream> {
    let (line, served) = UnixStream::pair()?;
    let mut to_link = served.try_clone()?;
    let mut from_link = served;
    let input = console.input();
    let mut output = console;
    std::thread::spawn(move || {
        let mut buffer = vec![0u8; 4096];
        loop {
            match output.read(&mut buffer) {
                Ok(0) | Err(_) => break,
                Ok(read) => {
                    let Some(chunk) = buffer.get(..read) else {
                        break;
                    };
                    if to_link.write_all(chunk).is_err() {
                        break;
                    }
                }
            }
        }
        let _ = to_link.shutdown(std::net::Shutdown::Both);
    });
    std::thread::spawn(move || {
        let mut buffer = vec![0u8; 4096];
        loop {
            match from_link.read(&mut buffer) {
                Ok(0) | Err(_) => break,
                Ok(read) => {
                    let Some(chunk) = buffer.get(..read) else {
                        break;
                    };
                    if input.send(chunk).is_err() {
                        break;
                    }
                }
            }
        }
        // The link is gone: its console stops.
        let _ = input.cancel();
    });
    Ok(line)
}
