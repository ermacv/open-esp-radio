#![no_std]
#![forbid(unsafe_code)]
#![deny(
    clippy::let_underscore_must_use,
    clippy::let_underscore_untyped,
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::unreachable,
    clippy::todo,
    clippy::unimplemented,
    clippy::disallowed_macros,
    clippy::disallowed_methods
)]
#![cfg_attr(
    test,
    allow(
        clippy::let_underscore_must_use,
        clippy::let_underscore_untyped,
        clippy::arithmetic_side_effects,
        clippy::indexing_slicing,
        clippy::panic,
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::unreachable,
        clippy::todo,
        clippy::unimplemented,
        clippy::disallowed_macros,
        clippy::disallowed_methods,
    )
)]

//! The asynchronous glue between the Wi-Fi lower-MAC port and the sans-IO
//! cores: it waits on port facts, `oer_portable_time::Timer` deadlines and
//! the application's commands, hands each input to the cores and carries
//! out their actions through the port. It decides no protocol question and
//! neither parses nor builds frames; it binds no executor.
//!
//! The station runs as two tasks on one executor. [`datapath`] is the
//! actor, the one task that owns the radio's data port and carries the
//! data both ways; [`lifecycle`] owns the station machine and its arbiter
//! and commands the actor only in messages. [`station`] holds the
//! application's [`station::Control`] and the vocabulary both tasks share.
//! [`monitor`] provides bounded capture and injection handoffs. Hardware
//! access and concrete task assembly belong to the chip backend and its
//! integration.
//!
//! The lifecycle's command receiver and the network's queue ends wait
//! through `ready(&mut self)`; a sender's wait returns an
//! `oer_portable_memory::WakingSpscSendPermit` that holds its exclusive
//! producer borrow. `send` publishes without a fallible second capacity
//! check. Cancelling the wait or dropping the permit consumes neither a
//! value nor queue capacity; the receiver may run on another thread.

pub mod access_point;

/// A producing port call's answer split into its input's state and the item
/// it could not keep ([`Unkept`](oer_protocols_ieee80211_contracts_port::Unkept)),
/// if any.
pub(crate) fn answered<'id, T, I, R>(
    answer: oer_protocols_ieee80211_contracts_port::PortAnswer<'id, T, I, R>,
) -> (
    oer_protocols_ieee80211_contracts_port::Input<I, R>,
    Option<oer_protocols_ieee80211_contracts_port::Unkept<'id, T>>,
)
where
    T: oer_protocols_ieee80211_contracts_port::PortTypes<'id>,
{
    match answer {
        Ok(answer) => (
            oer_protocols_ieee80211_contracts_port::Input::Accepted(answer),
            None,
        ),
        Err(unkeeping) => (unkeeping.input, Some(unkeeping.unkept)),
    }
}
pub mod datapath;
pub mod lifecycle;
pub mod monitor;
mod network_return;
pub mod station;

#[cfg(test)]
pub(crate) mod test_fixtures;

pub use monitor::injection as monitor_injection;
pub use monitor::{
    MonitorCaptureFrame, MonitorCaptureMetadata, MonitorCapturePool, MonitorCaptureReceiver,
    MonitorCaptureResources, MonitorCaptureSink,
};

#[cfg(test)]
extern crate std;
