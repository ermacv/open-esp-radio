//! Closed typed Wi-Fi MAC register transactions shared between chips.
//!
//! Chips whose Wi-Fi MAC peripherals have identical layouts place the register
//! blocks of [`oer_ieee80211_pac_raw`] at their own base addresses. This
//! crate composes the reviewed transactions over those blocks once. Every
//! function takes the register block by reference, so a chip PAC passes its
//! addressed peripheral and the compiler resolves the address statically; no
//! chip is selected here and nothing is dispatched at run time.
#![no_std]
#![forbid(unsafe_code)]

use oer_ieee80211_pac_raw as svd;

mod generated;

pub mod interface_address;

pub use generated::MacInterface;
