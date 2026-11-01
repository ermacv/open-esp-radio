//! How the station's network channels tell their two ends, in one type the
//! composition names: the station end (its runner and control) and the
//! network end (the network device and the network's stop), on one hart or
//! on two.
//!
//! On one hart every end waits on a waker of its own ([`SameHart`]). On two,
//! no waker crosses harts: each end waits on its task's executor signal and
//! the other hart raises it ([`AcrossHarts`]). Every notify is a type, so
//! every call is direct.

use core::marker::PhantomData;

use oer_memory::{AtomicWaker, QueueNotify};
use oer_sync::PriorityWaker;

/// The notify types of the station's network channels.
pub trait Notifies: 'static {
    /// A queue end of the network side: told when the station published or
    /// freed a cell.
    type Network: QueueNotify;
    /// A queue end of the station side: told when the network published or
    /// freed a cell.
    type Station: QueueNotify;
    /// The network side's link and stop request: told across a priority
    /// change on one hart.
    type NetworkWaker: QueueNotify;
    /// The station side's stop answer, likewise.
    type StationWaker: QueueNotify;
}

/// Both ends on one hart: each queue end its own atomic waker, the link and
/// the stop a priority waker.
pub struct SameHart;

impl Notifies for SameHart {
    type Network = AtomicWaker;
    type Station = AtomicWaker;
    type NetworkWaker = PriorityWaker;
    type StationWaker = PriorityWaker;
}

/// The network end on another hart than the station's: `N` raises the
/// network task's executor signal, `S` the station task's. One signal wakes
/// one task, so every network end is polled by the network's task and every
/// station end by the station's.
pub struct AcrossHarts<N, S>(PhantomData<fn() -> (N, S)>);

impl<N: QueueNotify + Copy + 'static, S: QueueNotify + Copy + 'static> Notifies
    for AcrossHarts<N, S>
{
    type Network = N;
    type Station = S;
    type NetworkWaker = N;
    type StationWaker = S;
}
