//! Receive counters sampled when a connected station epoch ends.
//!
//! The connected owner reads the MAC's cumulative receive counters once, at
//! its exit edge, and keeps the latest sample here. A beacon-loss exit can
//! then be told apart from a receiver that dropped frames for lack of
//! buffers, without an observer on the datapath.

use core::cell::Cell;

use embassy_sync::blocking_mutex::{Mutex, raw::CriticalSectionRawMutex};

/// Cumulative MAC receive counters at a connected station's exit.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ConnectedExitRxCounters {
    pub mpdu: u32,
    pub data_success: u32,
    pub other_unicast: u32,
    pub fcs_error: u32,
    pub abort: u32,
    pub buffer_full: u32,
    pub fifo_overflow: u32,
}

static LAST: Mutex<CriticalSectionRawMutex, Cell<Option<ConnectedExitRxCounters>>> =
    Mutex::new(Cell::new(None));

/// Only the chip supervisor has an exit edge to sample.
#[cfg(target_arch = "riscv32")]
pub(crate) fn record(statistics: oer_esp32s31_hal::ieee80211::mac::MacRxStatisticsSnapshot) {
    let primary = statistics.primary;
    let counters = ConnectedExitRxCounters {
        mpdu: u32::from(primary.mpdu_count),
        data_success: u32::from(primary.data_success),
        other_unicast: u32::from(primary.other_unicast),
        fcs_error: u32::from(primary.fcs_error),
        abort: u32::from(primary.abort),
        buffer_full: u32::from(primary.buffer_full),
        fifo_overflow: u32::from(primary.fifo_overflow),
    };
    LAST.lock(|last| last.set(Some(counters)));
}

/// The counters of the latest connected exit, once.
pub fn take_connected_exit_rx() -> Option<ConnectedExitRxCounters> {
    LAST.lock(Cell::take)
}
