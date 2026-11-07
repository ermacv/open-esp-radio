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
//! A core's values carry its brand, a type, so another network's handle
//! does not compile here:
//!
//! ```compile_fail,E0308
//! # use oer_protocols_network_core::{Core, Interface, claim};
//! # use oer_protocols_network_wire::ip::Dscp;
//! struct First;
//! struct Second;
//! # fn f(a: &mut [[u8; 64]; 1], b: &mut [[u8; 64]; 1], interface: Interface) {
//! let (_, brand) = claim::<First, 64, 1>(a).unwrap();
//! let mut first: Core<'_, First, 64, 1, 1, 1, 1> = Core::new(brand, interface);
//! let (_, brand) = claim::<Second, 64, 1>(b).unwrap();
//! let mut second: Core<'_, Second, 64, 1, 1, 1, 1> = Core::new(brand, interface);
//! let socket = second.open(1, Dscp::default()).unwrap();
//! first.receive(&socket).unwrap();
//! # }
//! ```
//!
//! Nor does another network's block:
//!
//! ```compile_fail,E0308
//! # use oer_protocols_network_core::claim;
//! struct First;
//! struct Second;
//! # fn f(a: &mut [[u8; 64]; 1], b: &mut [[u8; 64]; 1]) {
//! let (mut first, _) = claim::<First, 64, 1>(a).unwrap();
//! let (mut second, _) = claim::<Second, 64, 1>(b).unwrap();
//! let block = second.take().unwrap();
//! first.give(block);
//! # }
//! ```
//!
//! Nor does a block of a brand that is a subtype of another's:
//!
//! ```compile_fail,E0308
//! # use oer_protocols_network_core::claim;
//! type Wider = for<'a> fn(&'a ());
//! type Narrower = fn(&'static ());
//! # fn f(a: &mut [[u8; 64]; 1], b: &mut [[u8; 64]; 1]) {
//! let (mut wider, _) = claim::<Wider, 64, 1>(a).unwrap();
//! let (mut narrower, _) = claim::<Narrower, 64, 1>(b).unwrap();
//! let block = wider.take().unwrap();
//! narrower.give(block);
//! # }
//! ```
//!
//! The same code within one brand compiles:
//!
//! ```
//! # use oer_protocols_network_core::{Core, Interface, claim};
//! # use oer_protocols_network_wire::ip::Dscp;
//! struct Net;
//! # fn f(a: &mut [[u8; 64]; 1], interface: Interface) {
//! let (mut pool, brand) = claim::<Net, 64, 1>(a).unwrap();
//! let mut core: Core<'_, Net, 64, 1, 1, 1, 1> = Core::new(brand, interface);
//! let socket = core.open(1, Dscp::default()).unwrap();
//! core.receive(&socket).unwrap();
//! let block = pool.take().unwrap();
//! pool.give(block);
//! # }
//! ```
//!
//! A brand is a type, so tasks and their queues can name a block:
//!
//! ```
//! # use oer_protocols_network_core::{Block, claim};
//! # use std::sync::Mutex;
//! struct Net;
//! type Lane = Mutex<Option<Block<'static, Net, 64>>>;
//! static TO_NETWORK: Lane = Mutex::new(None);
//! static BACK: Lane = Mutex::new(None);
//!
//! async fn application(mut block: Block<'static, Net, 64>) {
//!     block.fill(b"filled by the application").unwrap();
//!     *TO_NETWORK.lock().unwrap() = Some(block);
//! }
//!
//! async fn network() {
//!     let block = TO_NETWORK.lock().unwrap().take().unwrap();
//!     assert_eq!(block.octets(), b"filled by the application");
//!     *BACK.lock().unwrap() = Some(block);
//! }
//!
//! fn task<F: Future<Output = ()> + Send + 'static>(future: F) -> F {
//!     future
//! }
//!
//! let storage = Box::leak(Box::new([[0; 64]; 1]));
//! let (mut pool, _brand) = claim::<Net, 64, 1>(storage).unwrap();
//! let block = pool.take().unwrap();
//! let (first, second) = (task(application(block)), task(network()));
//! let run = async {
//!     first.await;
//!     second.await;
//! };
//! let mut run = std::pin::pin!(run);
//! let mut cx = std::task::Context::from_waker(std::task::Waker::noop());
//! assert!(run.as_mut().poll(&mut cx).is_ready());
//! pool.give(BACK.lock().unwrap().take().unwrap());
//! assert_eq!(pool.outstanding(), 0);
//! ```

mod block;
mod interface;
mod machine;
mod neighbor;
mod owner;

pub use block::{
    Block, BlockTooShort, BrandRefusal, ClaimRefused, Claimed, CoreBrand, Pool, claim,
};
pub use interface::{Interface, InterfaceError};
pub use machine::{
    CommandId, CommandState, Core, Datagram, Demand, Eviction, Grant, NetEvent, OpenRefused,
    Outcome, Queued, Remote, RetiredResults, SendRefusal, SendRefused, SocketId, StaleSocket,
};
pub use owner::{NetworkOwner, Retired, Unsent};

#[cfg(test)]
mod tests;
