//! An open serial port's descriptor, as an `OwnedFd`.
//!
//! `serialport` hands a port's descriptor over only as a raw number. This
//! crate is the one place that takes ownership of it; it is a crate of its
//! own so that the root fixture installer, which links `oer-os-process`,
//! does not link `serialport`.

use std::os::fd::{FromRawFd as _, IntoRawFd as _, OwnedFd};

/// The descriptor of `port`, which closes when the result drops.
pub fn owned_descriptor(port: serialport::TTYPort) -> OwnedFd {
    // SAFETY: `TTYPort::into_raw_fd` forgets the port and returns its open
    // descriptor, transferring sole ownership, which the `OwnedFd` takes once.
    unsafe { OwnedFd::from_raw_fd(port.into_raw_fd()) }
}

#[cfg(test)]
mod tests {
    use std::io::{Read as _, Write as _};

    #[test]
    fn the_owned_descriptor_is_the_open_port() {
        let (main, mut secondary) = serialport::TTYPort::pair().unwrap();
        let mut file = std::fs::File::from(super::owned_descriptor(main));
        file.write_all(b"x").unwrap();
        let mut byte = [0];
        secondary.read_exact(&mut byte).unwrap();
        assert_eq!(&byte, b"x");
    }
}
