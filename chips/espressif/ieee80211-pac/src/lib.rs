//! Typed Wi-Fi MAC register blocks and transactions shared between chips.
//!
//! Chips whose Wi-Fi MAC peripherals have identical layouts place this
//! crate's register blocks at their own base addresses. The generated
//! modules hold each block's owner, generic over its placement, with its
//! typed accessors and reviewed transactions; a chip PAC's generated module
//! names the chip's address, claim and owner over them, so the compiler
//! resolves the address statically. No chip is selected here and nothing is
//! dispatched at run time.
#![no_std]
#![forbid(unsafe_code)]

mod generated;

pub mod interface_address;

pub use generated::*;
