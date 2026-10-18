#![forbid(unsafe_code)]

//! A session's CPU load window: each core's idle cycles in its executor's
//! wait over the measured traffic window (`cpu-load`, the executor's
//! `idle-accounting`). Without the feature the window is empty and the
//! session publishes no load.

use oer_hil_protocol::network::CpuLoadEvidence;

/// An open window; it stays on the task that opened it.
pub(in crate::product_hil) struct CpuLoadWindow {
    #[cfg(feature = "cpu-load")]
    window: oer_espressif_executor_embassy::idle_accounting::Window,
}

impl CpuLoadWindow {
    /// Open a window now.
    pub(in crate::product_hil) fn begin() -> Self {
        Self {
            #[cfg(feature = "cpu-load")]
            window: oer_espressif_executor_embassy::idle_accounting::Window::begin(),
        }
    }

    /// Close the window now: each core's load over it.
    pub(in crate::product_hil) fn finish(self) -> Option<CpuLoadEvidence> {
        #[cfg(feature = "cpu-load")]
        {
            let report = self.window.finish();
            Some(CpuLoadEvidence {
                window_cycles: report.window_cycles,
                idle_cycles: core::array::from_fn(|core| report.idle_cycles(core)),
            })
        }
        #[cfg(not(feature = "cpu-load"))]
        None
    }
}
