//! Application-owned security resources, independent of radio and storage media.
//!
//! RAM bonds survive connection changes and Host reconstruction only while the
//! caller retains the store. They do not survive SoC reset or loss of power. Importing
//! a bond does not prove how its original pairing was authenticated.

pub mod bonds;
pub mod comparison;
pub mod console;
pub mod epoch;
pub mod gatt;

#[cfg(test)]
mod bootstrap_tests;
