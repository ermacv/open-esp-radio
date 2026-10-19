//! The reviewed relation between the vendor `phy_param` object and the
//! committed calibration words production publishes, in one place for its
//! three readers: the comparison probes publish the words
//! (`projection`, feature `projection`), the Blobray tracking scenario compares them between
//! the pinned vendor roots and the compiled probes ([`committed`]), and the
//! hardware calibration cross-check compares them between the vendor
//! `phy_param` and a production snapshot captured on the board (both).
#![no_std]

pub mod committed;
#[cfg(feature = "projection")]
pub mod projection;
