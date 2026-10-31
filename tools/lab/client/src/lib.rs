//! A client of the LAB service, the one owner of the stand's leases and
//! hardware: the IPC protocol ([`protocol`]) and the connection a command
//! holds its lease by ([`client`]). `cargo fw`, `cargo hil` and `cargo
//! stand` depend on this crate alone; the service's scheduler and drivers
//! stay out of their builds.
#![forbid(unsafe_code)]

pub mod client;
pub mod protocol;
