#![no_std]
#![forbid(unsafe_code)]

//! Cooperative admission supplied by the executor binding, alongside time.
//! A hardware future checks this port between complete, bounded atoms. The
//! binding accounts for ready tasks, interrupt facts awaiting delivery, and
//! the current task poll's execution guard. Drivers do not inspect a run queue
//! or bind to a particular executor. There is no default no-peer policy.

/// A timer whose executor can admit another bounded hardware atom.
///
/// `true` requires the caller to yield before starting another atom. A task
/// becoming ready after a `false` answer can wait for at most that next atom.
/// The binding also returns true before its continuous-poll guard expires,
/// reserving enough time for the atom and the caller's return path.
pub trait CooperativeTimer: oer_time::Timer {
    fn should_yield(&self) -> bool;
}

/// Maximum response delay for a ready peer: one complete hardware atom.
pub const MAX_ATOM_MICROS: u32 = 50;
/// Safety guard even when the executor observes no ready peers.
pub const MAX_POLL_MICROS: u32 = 1_000;
/// Admission reserves time for the next atom and the caller's return path.
pub const POLL_ADMISSION_MICROS: u32 = MAX_POLL_MICROS - 2 * MAX_ATOM_MICROS;
