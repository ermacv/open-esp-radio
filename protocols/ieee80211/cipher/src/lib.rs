#![no_std]
#![forbid(unsafe_code)]

//! Stateless IEEE 802.11 frame protection and bounded AES key wrapping.
//! Packet-number allocation, replay frontiers and key negotiation belong to
//! the cores; callers supply key material and packet numbers to these transforms.

#[cfg(test)]
extern crate std;

pub mod aes;
pub mod bip;
mod ccm;
pub mod ccmp_mpdu;
pub mod management_ccmp;
