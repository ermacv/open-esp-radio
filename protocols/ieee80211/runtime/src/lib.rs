#![no_std]
#![forbid(unsafe_code)]
#![deny(
    clippy::let_underscore_must_use,
    clippy::let_underscore_untyped,
    clippy::struct_excessive_bools,
    clippy::fn_params_excessive_bools,
    clippy::wildcard_enum_match_arm
)]

//! The Wi-Fi runtime: the pumps that carry the sans-IO cores' steps out on
//! the lower-MAC port. It decides nothing: a pump is a loop over its core's
//! steps, holding no machine of its own.
//!
//! [`installation::drive`] carries a step of the installation machine
//! (`oer-protocols-ieee80211-core-installation`) out, for every role.

pub mod installation;
