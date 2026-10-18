#![no_std]
#![forbid(unsafe_code)]

//! Bounded in-process HCI transport between an LE Host and Controller.
//!
//! [`LeControllerHciResources`] owns bounded packet storage for one epoch and
//! splits into a Host transport accepted by `bt_hci::ExternalController` and
//! a raw [`InProcessHciControllerTransport`]. The in-process channel carries
//! HCI packet bodies with a separate typed packet kind, so no UART/H4 framing
//! exists inside the process. Both directions have bounded storage,
//! wake-driven backpressure and cancellation-safe waits.
//!
//! The Controller side can close both directions for good, or retire an epoch
//! whose queues are empty and restart it with a fresh generation; old Host
//! handles stay closed.
//! [`InProcessHciControllerTransport::try_receive_admitted`] takes commands
//! and the ACL data of the connection handles the Controller admits: data
//! for a busy connection waits without holding back commands or other
//! connections' data, and each handle keeps its order.
//!
//! The packet values, their validation and the Controller codecs are
//! `oer-bluetooth-hci`; this crate owns only the waiting transport. `M`
//! selects the `embassy-sync` mutex of the synchronization domain; the crate
//! owns no executor, timer or allocator.

#[cfg(test)]
extern crate std;

mod in_process;
mod packet;
mod resources;

pub use in_process::{
    HciChannelError, HciEpochIdentity, HciRestartError, HciRetired, HciRetirementError,
    InProcessHciControllerTransport, InProcessHciHostTransport, LeHostAclCreditSender,
};
pub use resources::{
    LeControllerHciEndpoints, LeControllerHciResources, LeControllerHciResourcesError,
};

#[cfg(test)]
mod tests;
