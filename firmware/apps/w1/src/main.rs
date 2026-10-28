#![cfg_attr(target_arch = "riscv32", no_std, no_main)]
#![forbid(unsafe_code)]
#![cfg_attr(target_arch = "riscv32", feature(type_alias_impl_trait))]

//! W1: a passive scan on the product receive path. The radio system brings
//! Wi-Fi up on the configured channel (as W0); the receive runner binds its
//! arena on the radio executor (P2), receives with the MAC and WDEVPWR
//! interrupts at P3 and hands staged frames to the application (P1), which
//! feeds them to the survey for a 10 s window from the first frame. A timer
//! stress phase, two hold cycles of every staging lease (the walker
//! exhausts; a staged release restarts it while the ownership experiment
//! rechecks the held buffers, [`own`]) and the stop follow; after the exit
//! the image prints its report block and verdict ([`report`]).
//!
//! Nothing is transmitted. The stand's identity comes from the build
//! (`OER_W1_CHANNEL`, `OER_W1_BSSID`, `OER_W1_SSID_HEX`): the BSSID of the
//! stand router's AP interface, which identifies the stand, and the SSID its
//! beacons must carry; without them the image prints
//! `OER-RESULT: fail w1 unconfigured`. No SSID, unmasked BSSID or base MAC is
//! printed.

#[cfg(target_arch = "riscv32")]
include!("entry.rs");
#[cfg(all(test, not(target_arch = "riscv32")))]
#[path = "own/crc32.rs"]
mod crc32;
#[cfg(all(test, not(target_arch = "riscv32")))]
mod survey;
#[cfg(not(target_arch = "riscv32"))]
fn main() {}
