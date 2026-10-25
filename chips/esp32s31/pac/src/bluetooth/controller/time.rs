//! Single transactions for the reviewed always-awake controller-time latch.
//!
//! Publication performs one fresh-read RMW followed by a device fence. The
//! request bit is observed with exactly one read, and the first latched-time
//! word is read after a fence. Which request owns the latch, and when it may
//! be read, belongs to the HAL.

#![deny(unsafe_code)]

/// First latched controller-time word read after hardware clears the request.
///
/// The value remains a wrapping positional `u32`: its physical unit and
/// effective counter width are not established by current evidence.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BluetoothControllerLatchedTime(u32);

impl BluetoothControllerLatchedTime {
    /// Retain one complete `BTLC_TIMER_CAPTURE_0` image.
    pub const fn from_bits(bits: u32) -> Self {
        Self(bits)
    }

    /// Return the complete positional image.
    pub const fn bits(self) -> u32 {
        self.0
    }
}

impl crate::BluetoothTaskRegisters {
    /// Publish one controller-time latch request.
    ///
    /// The transaction performs one fresh-read RMW which sets only
    /// `BTLC_TIMER_CONTROL.LATCH_REQUEST`, followed by a device fence. It
    /// does not wait for hardware.
    #[doc(hidden)]
    pub fn publish_controller_time_latch_request(&mut self) {
        self.bluetooth
            .bluetooth_controller_core
            .request_bluetooth_controller_time_latch();
        crate::device_fence();
    }

    /// Observe the latch request bit with exactly one control-register read.
    #[doc(hidden)]
    pub fn controller_time_latch_request_pending(&mut self) -> bool {
        self.bluetooth
            .bluetooth_controller_core
            .read_btlc_timer_control()
            .latch_request()
    }

    /// Fence after the observed self-clear edge and read the first latched
    /// time word exactly once.
    #[doc(hidden)]
    pub fn read_controller_latched_time(&mut self) -> BluetoothControllerLatchedTime {
        crate::device_fence();
        BluetoothControllerLatchedTime::from_bits(
            self.bluetooth
                .bluetooth_controller_core
                .read_btlc_timer_capture_0()
                .value(),
        )
    }
}
