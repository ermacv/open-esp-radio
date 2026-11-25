#![no_std]
#![forbid(unsafe_code)]
#![cfg_attr(
    test,
    allow(
        clippy::disallowed_methods,
        clippy::disallowed_macros,
        clippy::unreachable,
        clippy::arithmetic_side_effects
    )
)]

//! Ethernet, VLAN, ARP, IP addresses and headers, ICMP echo and transport header fields, DHCP
//! messages, and the Internet checksum.
//!
//! The codec owns byte order, bit fields, lengths and the structural
//! dependencies between these fields. It returns checked values and borrowed
//! views of the caller's bytes and writes into the caller's buffer. A parse
//! tells a malformed header from a valid one it does not interpret; it never
//! decides what a packet means for a radio.

pub mod address;
pub mod arp;
pub mod checksum;
mod code;
pub mod dhcp;
pub mod ethernet;
pub mod icmp;
pub mod ip;
pub mod transport;
pub mod udp_frame;
pub mod vlan;
