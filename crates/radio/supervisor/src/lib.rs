#![no_std]
#![forbid(unsafe_code)]

//! Executor-independent supervisor of the portable radio service port.
//!
//! `oer-radio` declares the executor-free service contracts; this runtime
//! transports them through a one-slot mailbox and drives the complete local
//! role epoch. Private domains keep bounded message values, transport
//! cancellation, stopped planning, active-role polling and the sole
//! owner-holding actor separate. Only the actor/runner domain owns physical
//! role frontiers; the mailbox transports value requests and reports.
//!
//! # Cancellation
//!
//! Dropping a client future stops waiting; it does not retract a command
//! already published to the actor. The port drains that command's response
//! before publishing another command.

#[cfg(test)]
extern crate std;

mod active;
mod actor;
mod dispatch;
mod message;
mod transport;

pub use active::{
    WifiActiveRoleControl, WifiActiveRoleExit, WifiRoleFrontier, drive_wifi_active_role,
    drive_wifi_active_role_pinned, finish_wifi_active_role,
};
pub use actor::{
    WifiRoleEpochOutcome, WifiRoleEpochRunner, WifiSupervisorPrepareFailure,
    WifiSupervisorPrepared, WifiSupervisorTask, prepare_wifi_supervisor, run_wifi_supervisor_actor,
};
pub use dispatch::{WifiStoppedDispatch, dispatch_wifi_stopped_command};
pub use message::{
    WifiStartKind, WifiSupervisorCommand, WifiSupervisorError, WifiSupervisorResponse,
};
pub use transport::{
    WifiSupervisorControlError, WifiSupervisorControlResources, WifiSupervisorEndpoint,
    WifiSupervisorEndpoints, WifiSupervisorMailbox,
};

#[cfg(test)]
mod tests;
