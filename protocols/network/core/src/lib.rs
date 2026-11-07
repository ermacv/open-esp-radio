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
#![cfg_attr(
    test,
    allow(
        clippy::arithmetic_side_effects,
        clippy::disallowed_macros,
        clippy::disallowed_methods,
        clippy::indexing_slicing,
        clippy::unwrap_used
    )
)]

//! A sans-IO network core with one owner (see the crate's README).
//!
//! A core's values carry its [`scope`]'s brand, so another core's handle does
//! not compile here:
//!
//! ```compile_fail
//! # use oer_protocols_network_core::{Core, Interface, scope};
//! # use oer_protocols_network_wire::ip::Dscp;
//! # fn f(a: &mut [[u8; 64]; 1], b: &mut [[u8; 64]; 1], interface: Interface) {
//! scope(a, |_, brand| {
//!     let mut first: Core<'_, 64, 1, 1, 1, 1> = Core::new(brand, interface);
//!     scope(b, |_, brand| {
//!         let mut second: Core<'_, 64, 1, 1, 1, 1> = Core::new(brand, interface);
//!         let socket = second.open(1, Dscp::default()).unwrap();
//!         first.receive(&socket).unwrap();
//!     });
//! });
//! # }
//! ```
//!
//! Nor does another pool's block:
//!
//! ```compile_fail
//! # use oer_protocols_network_core::scope;
//! # fn f(a: &mut [[u8; 64]; 1], b: &mut [[u8; 64]; 1]) {
//! scope(a, |mut first, _| {
//!     scope(b, |mut second, _| {
//!         let block = second.take().unwrap();
//!         first.give(block);
//!     });
//! });
//! # }
//! ```
//!
//! The same code within one scope compiles:
//!
//! ```
//! # use oer_protocols_network_core::{Core, Interface, scope};
//! # use oer_protocols_network_wire::ip::Dscp;
//! # fn f(a: &mut [[u8; 64]; 1], interface: Interface) {
//! scope(a, |mut pool, brand| {
//!     let mut core: Core<'_, 64, 1, 1, 1, 1> = Core::new(brand, interface);
//!     let socket = core.open(1, Dscp::default()).unwrap();
//!     core.receive(&socket).unwrap();
//!     let block = pool.take().unwrap();
//!     pool.give(block);
//! });
//! # }
//! ```

mod block;
mod interface;
mod machine;
mod neighbor;

pub use block::{Block, BlockTooShort, CoreBrand, Pool, scope};
pub use interface::{Interface, InterfaceError};
pub use machine::{
    Class, CommandId, Core, Datagram, Demand, FramePort, Grant, NetEvent, OpenRefused, Outcome,
    Remote, SendRefusal, SendRefused, SlotTooShort, SocketId, StaleSocket, Unsent,
};

#[cfg(test)]
mod tests;
