//! Restricted BLE scanner start: the scan-backoff state publication.

#![deny(unsafe_code)]

use crate::bluetooth::registers::{BluetoothPlacements, BluetoothTaskRegisters, device_fence};

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

struct PacBluetoothScanStartTransaction<'registers, B: oer_mmio::Placement> {
    registers: &'registers mut oer_esp32s31_pac::ble_scan_backoff::BleScanBackoff<B>,
}

impl<B: oer_mmio::Placement> BluetoothScanStartTransaction
    for PacBluetoothScanStartTransaction<'_, B>
{
    fn initialize_backoff_state_1(&mut self) {
        self.registers.initialize_bluetooth_scan_backoff_state_1();
    }

    fn initialize_backoff_state_0(&mut self) {
        self.registers.initialize_bluetooth_scan_backoff_state_0();
    }

    fn publish_standard_upper_limit_max(&mut self) {
        self.registers.write_upper_limit_max(|w| {
            w.set_value(oer_mmio::Bits::constant::<0x20>());
        });
    }
}

impl<P: BluetoothPlacements> BluetoothTaskRegisters<P> {
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
            registers: &mut self.bluetooth.ble_scan_backoff,
        };
        execute_scan_start_transaction(&mut transaction);
        device_fence();
        BluetoothScanStartPublished { _private: () }
    }
}

#[cfg(test)]
mod tests;
