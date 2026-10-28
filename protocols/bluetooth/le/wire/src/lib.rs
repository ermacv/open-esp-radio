#![no_std]
#![forbid(unsafe_code)]
#![deny(
    clippy::panic,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::unreachable,
    clippy::todo,
    clippy::unimplemented,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    clippy::disallowed_macros,
    clippy::disallowed_methods
)]

//! Stateless Bluetooth LE over-the-air fields, PDU codecs and test payloads.
//! No protocol state, clock domain or backend ownership lives here.

#[cfg(test)]
extern crate std;

mod address;
pub mod advertising;
pub mod advertising_header;
pub mod air_time;
mod channel;
pub mod connectable_advertising;
pub mod connection;
pub mod data_length;
pub mod dtm;
mod pdu;
pub mod scanning;

pub use address::{
    LE_DEVICE_ADDRESS_LENGTH, LeDeviceAddress, LeDeviceAddressKind, LePublicDeviceAddress,
};
pub use channel::{
    AdvertisingChannel, AdvertisingChannels, ChannelError, DataChannel, TestChannel,
};
pub use pdu::{
    AccessAddress, AdvertisingPdu, CrcInit, DataPdu, DataPduKind, PduError, TestPayloadType,
};

pub mod control;
pub mod data_header;
pub mod encryption;
