//! Hardware publication of the Controller-SRAM device table.
//!
//! The filter accept list and the resolving list share one table of 8-byte
//! entries in Controller SRAM. Hardware walks it from the published first
//! entry for the published number of entries; every table change republishes
//! both, the count first, as the vendor's `ble_hw.c` table writers do.

#![deny(unsafe_code)]

use crate::{
    BluetoothControllerSramAddress, BluetoothDeviceTableEntryCount, BluetoothTaskRegisters,
    device_fence,
};

/// Affine proof that one device-table count and first-entry address were
/// published.
#[must_use = "the published device table belongs to a live controller epoch"]
pub struct BluetoothDeviceTablePublished {
    _private: (),
}

trait BluetoothDeviceTableTransaction {
    fn publish_entry_count(&mut self, count: BluetoothDeviceTableEntryCount);
    fn publish_first_entry(&mut self, first_entry: BluetoothControllerSramAddress);
}

fn execute_device_table_publication(
    transaction: &mut impl BluetoothDeviceTableTransaction,
    first_entry: BluetoothControllerSramAddress,
    count: BluetoothDeviceTableEntryCount,
) {
    transaction.publish_entry_count(count);
    transaction.publish_first_entry(first_entry);
}

struct PacBluetoothDeviceTableTransaction<'registers> {
    registers: &'registers crate::svd::BtmacBlePhyInit,
}

impl BluetoothDeviceTableTransaction for PacBluetoothDeviceTableTransaction<'_> {
    fn publish_entry_count(&mut self, count: BluetoothDeviceTableEntryCount) {
        crate::generated::publish_bluetooth_device_table_entry_count(self.registers, count);
    }

    fn publish_first_entry(&mut self, first_entry: BluetoothControllerSramAddress) {
        crate::svd::zero_based_field_write::publish_bluetooth_device_table_first_entry(
            self.registers,
            first_entry.compressed_image(),
        );
    }
}

impl BluetoothTaskRegisters {
    /// Publish the device table: its entry count, then the address of its
    /// first entry. Table writes are ordered before the first register write.
    ///
    /// # Safety
    ///
    /// The caller must own a powered controller epoch and `count` initialized
    /// entries starting at `first_entry` that stay pinned while hardware may
    /// walk them, and must serialize this publication with every scanner,
    /// advertiser and initiator that filters against the table.
    #[doc(hidden)]
    #[allow(
        unsafe_code,
        reason = "the signature retains the table lifetime and serialization prerequisites"
    )]
    pub unsafe fn publish_device_table(
        &mut self,
        first_entry: BluetoothControllerSramAddress,
        count: BluetoothDeviceTableEntryCount,
    ) -> BluetoothDeviceTablePublished {
        device_fence();
        let mut transaction = PacBluetoothDeviceTableTransaction {
            registers: &self.bluetooth.btmac_ble_phy_init,
        };
        execute_device_table_publication(&mut transaction, first_entry, count);
        device_fence();
        BluetoothDeviceTablePublished { _private: () }
    }
}

#[cfg(test)]
mod tests;
