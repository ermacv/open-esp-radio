//! The Linux socket address that rustix does not model: the link-layer one.
//!
//! The address is constructed only from validated fields, so its raw
//! representation is always a complete, fully initialized address of its
//! family. This module is the only place on the host that passes raw memory to
//! socket system calls.

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
