//! Linux socket facilities that rustix does not model: the link-layer
//! address and two raw socket options.
//!
//! Each address type is constructed only from validated fields, so its raw
//! representation is always a complete, fully initialized address of its
//! family. This module is the only place in the HIL host that passes raw
//! memory to socket system calls.

use std::{
    io,
    os::fd::{AsFd, AsRawFd as _},
};

use rustix::net::addr::{SocketAddrArg, SocketAddrLen, SocketAddrOpaque};

/// `sockaddr_ll` naming one interface for a packet socket that sees every
/// Ethernet protocol.
pub struct LinkAddress(libc::sockaddr_ll);

impl LinkAddress {
    pub fn new(interface: u32) -> Result<Self, std::num::TryFromIntError> {
        Ok(Self(libc::sockaddr_ll {
            sll_family: libc::AF_PACKET as u16,
            sll_protocol: (libc::ETH_P_ALL as u16).to_be(),
            sll_ifindex: interface.try_into()?,
            sll_hatype: 0,
            sll_pkttype: 0,
            sll_halen: 0,
            sll_addr: [0; 8],
        }))
    }
}

#[allow(
    unsafe_code,
    reason = "rustix binds foreign address types through SocketAddrArg"
)]
// SAFETY: the closure receives a pointer to this initialized `sockaddr_ll`
// and its exact size, both valid for the duration of the call.
unsafe impl SocketAddrArg for LinkAddress {
    unsafe fn with_sockaddr<R>(
        &self,
        f: impl FnOnce(*const SocketAddrOpaque, SocketAddrLen) -> R,
    ) -> R {
        f(
            (&raw const self.0).cast(),
            size_of::<libc::sockaddr_ll>() as SocketAddrLen,
        )
    }
}

/// Send and receive only through network interface `interface`
/// (`SO_BINDTODEVICE`; unprivileged since Linux 5.7 on an unbound socket).
pub fn bind_to_device(socket: impl AsFd, interface: &str) -> io::Result<()> {
    if interface.is_empty() || interface.len() >= libc::IFNAMSIZ || interface.contains('\0') {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("`{interface}` is not an interface name"),
        ));
    }
    #[allow(unsafe_code, reason = "rustix has no SO_BINDTODEVICE option")]
    // SAFETY: the option value is the interface name's initialized bytes,
    // whose pointer and length stay valid for the call on a borrowed live
    // socket; the kernel reads at most that length.
    let result = unsafe {
        libc::setsockopt(
            socket.as_fd().as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_BINDTODEVICE,
            interface.as_ptr().cast(),
            interface.len() as libc::socklen_t,
        )
    };
    if result == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

/// The socket's cumulative dropped-packet count from `SO_MEMINFO`, including
/// a final loss with no later ancillary message.
pub fn dropped_packets(socket: impl AsFd) -> io::Result<u32> {
    // Linux uapi `linux/sock_diag.h`: `SK_MEMINFO_DROPS` is entry eight.
    let mut info = [0_u32; 9];
    let mut length = size_of_val(&info) as libc::socklen_t;
    #[allow(unsafe_code, reason = "rustix has no SO_MEMINFO option")]
    // SAFETY: the output array and its length are valid, writable storage for
    // the exact supplied size on a borrowed live socket.
    let result = unsafe {
        libc::getsockopt(
            socket.as_fd().as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_MEMINFO,
            info.as_mut_ptr().cast(),
            &raw mut length,
        )
    };
    if result != 0 {
        return Err(io::Error::last_os_error());
    }
    if (length as usize) < size_of_val(&info) {
        return Err(io::Error::other(
            "SO_MEMINFO omitted socket drop accounting",
        ));
    }
    Ok(info[8])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_socket_binds_to_an_interface_by_name_and_refuses_a_bad_name() {
        let socket = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        bind_to_device(&socket, "lo").unwrap();
        assert_eq!(
            bind_to_device(&socket, "").unwrap_err().kind(),
            io::ErrorKind::InvalidInput
        );
        assert_eq!(
            bind_to_device(&socket, "an-interface-name-too-long")
                .unwrap_err()
                .kind(),
            io::ErrorKind::InvalidInput
        );
        assert!(bind_to_device(&socket, "oer-absent0").is_err());
    }
}
