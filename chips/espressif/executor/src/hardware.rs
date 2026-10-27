//! The chip HAL implements this contract using its PAC source identities.
//! No vendor interrupt enum or singleton crosses the executor boundary.

pub trait ExecutorPlatform {
    /// A checked identity in the chip's fact-defined hart geometry.
    type Hart;
    /// Sources of FROM_CPU 0 through 3, in line order.
    const SOFTWARE_SOURCES: [u16; 4];
    /// Sources of SYSTIMER targets 0 through 2, in comparator order.
    const ALARM_SOURCES: [u16; 3];
    /// Whether a debugger is attached; waits must not stall its access.
    fn debugger_connected(hart: Self::Hart) -> bool;
    /// Read the current hart's pending matrix without changing its routes.
    fn is_pending(hart: Self::Hart, source: u16) -> bool;
}
