//! Linux socket facilities that rustix does not model: link-layer, HCI and
//! L2CAP addresses and two raw socket options.
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

/// Linux `sockaddr_hci`: one controller index and HCI channel.
#[repr(C)]
pub struct HciAddress {
    family: libc::sa_family_t,
    index: u16,
    channel: u16,
}

// `sockaddr_hci` is `{ sa_family_t, u16, u16 }` without padding.
const _: () = assert!(size_of::<HciAddress>() == 6);

impl HciAddress {
    /// The raw user channel, with exclusive access to the controller.
    pub const USER_CHANNEL: u16 = 1;
    /// The management control channel.
    pub const CONTROL_CHANNEL: u16 = 3;

    pub fn new(index: u16, channel: u16) -> Self {
        Self {
            family: libc::AF_BLUETOOTH as libc::sa_family_t,
            index,
            channel,
        }
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

#[allow(
    unsafe_code,
    reason = "rustix binds foreign address types through SocketAddrArg"
)]
// SAFETY: `HciAddress` is `repr(C)` with the layout of Linux `sockaddr_hci`;
// the closure receives a pointer to it and its exact size for the call.
unsafe impl SocketAddrArg for HciAddress {
    unsafe fn with_sockaddr<R>(
        &self,
        f: impl FnOnce(*const SocketAddrOpaque, SocketAddrLen) -> R,
    ) -> R {
        f(
            (&raw const *self).cast(),
            size_of::<Self>() as SocketAddrLen,
        )
    }
}

/// Linux `sockaddr_l2` for the LE ATT fixed channel of a public device.
#[repr(C)]
pub struct L2capAddress {
    family: libc::sa_family_t,
    psm: u16,
    address: [u8; 6],
    cid: u16,
    kind: u8,
    // Initialized explicitly: the kernel reads the whole structure.
    padding: u8,
}

// `sockaddr_l2` is `{ sa_family_t, le16, bdaddr_t, le16, u8 }` padded to 14.
const _: () = assert!(size_of::<L2capAddress>() == 14);

impl L2capAddress {
    const ATT_CID: u16 = 4;
    const LE_PUBLIC: u8 = 1;

    /// ATT on an LE public address, in HCI byte order.
    pub fn att(address: [u8; 6]) -> Self {
        Self {
            family: libc::AF_BLUETOOTH as libc::sa_family_t,
            psm: 0,
            address,
            cid: Self::ATT_CID.to_le(),
            kind: Self::LE_PUBLIC,
            padding: 0,
        }
    }
}

#[allow(
    unsafe_code,
    reason = "rustix binds foreign address types through SocketAddrArg"
)]
// SAFETY: `L2capAddress` is `repr(C)` with the layout of Linux `sockaddr_l2`
// and no uninitialized bytes; the closure receives it and its exact size.
unsafe impl SocketAddrArg for L2capAddress {
    unsafe fn with_sockaddr<R>(
        &self,
        f: impl FnOnce(*const SocketAddrOpaque, SocketAddrLen) -> R,
    ) -> R {
        f(
            (&raw const *self).cast(),
            size_of::<Self>() as SocketAddrLen,
        )
    }
}

/// Require an unauthenticated, unencrypted link (`BT_SECURITY_LOW`) before
/// an L2CAP socket connects.
pub fn set_bluetooth_security_low(socket: impl AsFd) -> io::Result<()> {
    const SOL_BLUETOOTH: libc::c_int = 274;
    const BT_SECURITY: libc::c_int = 4;
    // Linux `struct bt_security { u8 level; u8 key_size; }`.
    let security: [u8; 2] = [1, 0];
    #[allow(unsafe_code, reason = "rustix has no Bluetooth socket options")]
    // SAFETY: the option value is an initialized two-byte `bt_security` whose
    // pointer and length stay valid for the call on a borrowed live socket.
    let result = unsafe {
        libc::setsockopt(
            socket.as_fd().as_raw_fd(),
            SOL_BLUETOOTH,
            BT_SECURITY,
            security.as_ptr().cast(),
            size_of_val(&security) as libc::socklen_t,
        )
    };
    if result == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
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
