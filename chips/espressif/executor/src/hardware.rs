//! The chip HAL implements this contract using its PAC source identities.
//! No vendor interrupt enum or singleton crosses the executor boundary.

pub trait ExecutorPlatform {
    /// Sources of FROM_CPU 0 through 3, in line order.
    const SOFTWARE_SOURCES: [u16; 4];
    /// Sources of SYSTIMER targets 0 through 2, in comparator order.
    const ALARM_SOURCES: [u16; 3];
    /// Whether a debugger is attached; waits must not stall its access.
    fn debugger_connected() -> bool;
    /// Read the current hart's pending matrix without changing its routes.
    fn is_pending(source: u16) -> bool;
}
