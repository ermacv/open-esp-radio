//! Optional observation of real RX allocation ownership, without a clock or
//! recorder dependency in the radio. Disabled in ordinary production builds.

/// Allocation transition, identified by physical buffer rather than descriptor.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RxOwnershipEdge {
    /// Exclusive DMA buffer lease is about to leave the completed-unit owner.
    Detached,
    /// The final lease has returned, immediately before publishing `Released`.
    Released,
    /// The ring owner completed software append/publication for this buffer.
    /// This does not prove that hardware has fetched it or settled a reload.
    Republished,
    /// A stopped-ring owner reclaimed the allocation instead of live append.
    /// This does not prove that the new ring has started.
    ReclaimedWhileStopped,
}

/// Caller-owned diagnostic sink installed before any ring borrows an arena.
///
/// Calls may occur on different cores, including from the final lease's Drop.
/// The sink must be bounded, nonblocking and must not panic or reenter radio
/// code. It receives no authority to access payloads, DMA or the ring. A sink
/// shared between arenas must keep their identities separate. `arena` is only
/// an opaque identity, never a dereferenceable capability.
///
/// Timestamping and record storage belong to the board/application. Timestamps
/// must be comparable across calling contexts. Recorder overflow, incomplete
/// lifecycles and clock errors must remain visible, not become zero durations.
/// Publication callbacks follow software append; their times include callback
/// overhead and do not establish RF or hardware-fetch timing guarantees.
pub trait RxOwnershipObserver: Sync {
    /// Observe one transition for the arena's physical buffer index.
    fn observe(&self, arena: usize, buffer: usize, edge: RxOwnershipEdge);
}
