//! The LAB service: one process per host owns the stand's queue, its leases
//! and every access to the leased hardware; `cargo fw`, `cargo hil` and
//! `cargo stand` are its clients over one Unix socket (`oer_lab_client`).
//!
//! The service keeps the scheduler of `oer-lab-arbiter`: who is granted
//! what, and which resources must be recovered first. Its drivers
//! ([`service::Drivers`]) run the operations and the recoveries.
#![forbid(unsafe_code)]

pub mod router;
pub mod run;
pub mod service;
pub mod stand;
