#![cfg_attr(
    test,
    expect(
        clippy::cast_possible_truncation,
        clippy::let_underscore_must_use,
        reason = "test code: a cast or a dropped result in a test loses no product's value"
    )
)]
#![no_std]
#![forbid(unsafe_code)]
#![cfg_attr(
    test,
    allow(
        clippy::disallowed_methods,
        clippy::disallowed_macros,
        clippy::unreachable,
        clippy::arithmetic_side_effects
    )
)]

//! A sans-IO network core with one owner (see the crate's README).
//!
//! A core's values carry its brand, so another network's handle does not
//! compile here. A brand lives as long as its scope
//! ([`with_local_network`]).
//!
//! ```compile_fail,E0521
//! # use oer_protocols_network_core::{BlockPlace, Core, Interface, claim, with_local_network};
//! # use oer_protocols_network_wire::ip::Dscp;
//! # fn f<'m>(a: &'m mut [BlockPlace<'m, 64>; 1], b: &'m mut [BlockPlace<'m, 64>; 1], interface: Interface) {
//! with_local_network(|first| {
//!     with_local_network(|second| {
//!         let (_, brand) = claim(first, a);
//!         let mut first: Core<'_, _, 64, 1, 1, 1, 1> = Core::new(brand, interface);
//!         let (_, brand) = claim(second, b);
//!         let mut second: Core<'_, _, 64, 1, 1, 1, 1> = Core::new(brand, interface);
//!         let socket = second.open(1, Dscp::default()).unwrap();
//!         first.receive(&socket).unwrap();
//!     });
//! });
//! # }
//! ```
//!
//! Nor does another network's block:
//!
//! ```compile_fail,E0521
//! # use oer_protocols_network_core::{BlockPlace, claim, with_local_network};
//! # fn f<'m>(a: &'m mut [BlockPlace<'m, 64>; 1], b: &'m mut [BlockPlace<'m, 64>; 1]) {
//! with_local_network(|first| {
//!     with_local_network(|second| {
//!         let (mut first, _) = claim(first, a);
//!         let (mut second, _) = claim(second, b);
//!         let block = second.take().unwrap();
//!         first.give(block);
//!     });
//! });
//! # }
//! ```
//!
//! Nor its group:
//!
//! ```compile_fail,E0521
//! # use oer_protocols_network_core::{BlockPlace, claim, with_local_network};
//! # fn f<'m>(a: &'m mut [BlockPlace<'m, 64>; 1], b: &'m mut [BlockPlace<'m, 64>; 1]) {
//! with_local_network(|first| {
//!     with_local_network(|second| {
//!         let (mut first, _) = claim(first, a);
//!         let (mut second, _) = claim(second, b);
//!         let group = second.group(1).unwrap();
//!         first.take_as(group);
//!     });
//! });
//! # }
//! ```
//!
//! Nor does a block of a longer scope stand for one of a shorter:
//!
//! ```compile_fail
//! # use oer_protocols_network_core::{Block, LocalNetwork};
//! fn shorter<'m, 'long: 'short, 'short>(
//!     block: Block<'m, LocalNetwork<'long>, 64>,
//! ) -> Block<'m, LocalNetwork<'short>, 64> {
//!     block
//! }
//! ```
//!
//! The same calls, each within its brand, compile in two scopes at once:
//!
//! ```
//! # use oer_protocols_network_core::{BlockPlace, Core, Interface, claim, with_local_network};
//! # use oer_protocols_network_wire::ip::Dscp;
//! # fn f<'m>(a: &'m mut [BlockPlace<'m, 64>; 1], b: &'m mut [BlockPlace<'m, 64>; 1], interface: Interface) {
//! with_local_network(|first| {
//!     with_local_network(|second| {
//!         let (mut pool, brand) = claim(first, a);
//!         let mut core: Core<'_, _, 64, 1, 1, 1, 1> = Core::new(brand, interface);
//!         let (mut other, brand) = claim(second, b);
//!         let mut other_core: Core<'_, _, 64, 1, 1, 1, 1> = Core::new(brand, interface);
//!         let socket = core.open(1, Dscp::default()).unwrap();
//!         core.receive(&socket).unwrap();
//!         let socket = other_core.open(1, Dscp::default()).unwrap();
//!         other_core.receive(&socket).unwrap();
//!         let group = pool.group(1).unwrap();
//!         let block = pool.take_as(group).unwrap();
//!         pool.give(block);
//!         let block = other.take().unwrap();
//!         other.give(block);
//!         pool.loans().pool(1, 0).unwrap();
//!         pool.loans().grant(0, 1).unwrap();
//!         let loan = pool.loans().lend(0).unwrap();
//!         pool.loans().give_back(loan);
//!     });
//! });
//! # }
//! ```

mod block;
mod brand;
mod dhcp;
mod diagnostics;
mod echo;
mod interface;
mod loans;
mod machine;
mod neighbor;
mod owner;
mod payload;
mod places;
mod serial;

pub use block::{
    Block, BlockPlace, BlockTooShort, Claimed, CoreBrand, GROUPS, Group, GroupsFull, Pool, claim,
};
pub use brand::{Brand, LocalNetwork, NetworkClaim, with_local_network};
pub use dhcp::{DNS_SERVERS, DhcpDrop, DhcpState, Lease, LeaseFault, Servers};
pub use diagnostics::Count;
pub use echo::{ECHO_RATE, ECHOES, IcmpDrop};
pub use interface::{Interface, InterfaceError, Ipv4};
pub use loans::{Loan, Loans, LoansRefused, NotLent};
pub use machine::{
    AddressState, CommandId, CommandState, Configuration, Core, Datagram, Declined, Demand, Ends,
    Eviction, FillFailed, Grant, LendTo, NetEvent, OpenRefused, Outcome, Queued, Remote,
    RetiredResults, SendRefusal, SendRefused, SocketId, StaleSocket, WorkId,
};
pub use owner::{Broken, Intake, LentDatagram, NetworkOwner, Retired, Transmitted, Unsent};
pub use payload::{Payload, PayloadFull};

#[cfg(test)]
mod tests;
