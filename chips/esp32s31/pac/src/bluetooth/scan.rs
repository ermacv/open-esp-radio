//! Restricted BLE scanner start: the scan-backoff state publication.

#![deny(unsafe_code)]

use crate::{BluetoothTaskRegisters, device_fence};

/// Affine proof that one complete scanner command sequence was published.
#[must_use = "the scanner command publication belongs to a live controller epoch"]
pub struct BluetoothScanStartPublished {
    _private: (),
}

/// The scan-backoff start sequence: both backoff words start at one before
/// the maximum upper limit is published.
trait BluetoothScanStartTransaction {
    fn initialize_backoff_state_1(&mut self);
    fn initialize_backoff_state_0(&mut self);
    fn publish_standard_upper_limit_max(&mut self);
}

fn execute_scan_start_transaction(transaction: &mut impl BluetoothScanStartTransaction) {
    transaction.initialize_backoff_state_1();
    transaction.initialize_backoff_state_0();
    transaction.publish_standard_upper_limit_max();
}

struct PacBluetoothScanStartTransaction<'registers> {
    registers: &'registers crate::svd::BleScanBackoff,
}

impl BluetoothScanStartTransaction for PacBluetoothScanStartTransaction<'_> {
    fn initialize_backoff_state_1(&mut self) {
        crate::svd::fixed_register_image::initialize_bluetooth_scan_backoff_state_1(self.registers);
    }

    fn initialize_backoff_state_0(&mut self) {
        crate::svd::fixed_register_image::initialize_bluetooth_scan_backoff_state_0(self.registers);
    }

    fn publish_standard_upper_limit_max(&mut self) {
        crate::svd::fixed_register_image::publish_bluetooth_scan_standard_upper_limit_max(
            self.registers,
        );
    }
}

impl BluetoothTaskRegisters {
    /// Publish the complete reviewed scanner start: initialize both
    /// scan-backoff words to one, then publish the maximum upper limit (32)
    /// of the S31 Controller configuration.
    ///
    /// Descriptor and list writes are ordered before the first write.
    ///
    /// # Safety
    ///
    /// The caller must own a powered controller epoch with a fully initialized
    /// scanner link state, scheduler item and RX list, and must serialize this
    /// publication with every task and interrupt owner of scanner hardware.
    #[doc(hidden)]
    #[allow(
        unsafe_code,
        reason = "the signature retains powered scanner-lifecycle and serialization prerequisites"
    )]
    pub unsafe fn publish_scan_start(&mut self) -> BluetoothScanStartPublished {
        device_fence();
        let mut transaction = PacBluetoothScanStartTransaction {
            registers: &self.bluetooth.ble_scan_backoff,
        };
        execute_scan_start_transaction(&mut transaction);
        device_fence();
        BluetoothScanStartPublished { _private: () }
    }
}

#[cfg(test)]
mod tests;
