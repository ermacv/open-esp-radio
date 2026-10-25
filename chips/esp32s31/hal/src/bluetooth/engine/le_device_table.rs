//! Controller-SRAM device table of the ESP32-S31 Bluetooth LE Controller.
//!
//! The filter accept list and the resolving list share one table of 8-byte
//! entries that hardware walks when it filters advertising. Byte 0 marks the
//! lists an entry belongs to (bit 1 the filter accept list, bit 0 the
//! resolving list), byte 1 bit 7 a valid entry and bit 6 a random address,
//! and bytes 2 to 7 hold the address, least significant octet first
//! (`ble_hw.c` of the pinned `libble_app`). This owner implements the filter
//! accept list only. Entries stay packed from the first: a removal moves the
//! last entry into the freed slot, so the published count is the number of
//! members. The owner publishes nothing itself; the caller publishes the
//! [`LeDeviceTable::publication`] after every change.

#![deny(unsafe_code, clippy::undocumented_unsafe_blocks)]

use core::pin::Pin;

use crate::bluetooth::DeviceTablePublication;
use crate::layouts::*;

use crate::bluetooth::engine::{ControllerDmaSpace, Placement};
use crate::dma_static::Placed;

/// Filter accept list entries the Controller keeps, as the S31 build's LE
/// Read Filter Accept List Size reports.
pub const BLUETOOTH_FILTER_ACCEPT_LIST_CAPACITY: usize =
    BleDeviceTable::SIZE / BleDeviceTableEntry::SIZE;

/// One filter accept list device: its address type and address.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LeFilterAcceptListDevice {
    /// The address is random rather than public.
    pub random: bool,
    /// The address, least significant octet first.
    pub address: [u8; 6],
}

impl LeFilterAcceptListDevice {
    fn image(self) -> DeviceEntryImage {
        let [a0, a1, a2, a3, a4, a5] = self.address;
        DeviceEntryImage {
            flags_and_address: DEVICE_ENTRY
                .with_random(self.random)
                .with_address_low(u16::from_le_bytes([a0, a1])),
            address_high: BleDeviceTableEntryAddressHigh::ZERO
                .with_value(u32::from_le_bytes([a2, a3, a4, a5])),
        }
    }
}

/// Pinned storage of the device table.
#[repr(C, align(4))]
#[derive(bytemuck::Zeroable)]
pub struct LeDeviceTableStorage {
    layout: BleDeviceTable,
}

impl LeDeviceTableStorage {
    pub const fn new() -> Self {
        Self {
            layout: BleDeviceTable::zeroed(),
        }
    }
}

impl Default for LeDeviceTableStorage {
    fn default() -> Self {
        Self::new()
    }
}

/// Why a filter accept list change was refused.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LeDeviceTableError {
    /// Every entry is in use.
    Full,
    /// The device is not in the list.
    NotFound,
}

/// The device table bound at its Controller-SRAM address.
pub struct LeDeviceTable {
    storage: Pin<&'static mut LeDeviceTableStorage>,
    first_entry: DmaRef<BleDeviceTable>,
    members: usize,
}

impl LeDeviceTable {
    /// Publish this table's retained extent, without exporting its first link.
    pub(crate) fn publish(&self, controller: &mut crate::bluetooth::ControllerHal<'_>) {
        let _ = controller.publish_device_table(&self.publication());
    }

    /// Claim and initialize a linker-proven device-table allocation.
    pub fn place<S: ControllerDmaSpace>(placed: Placed<LeDeviceTableStorage>) -> Self {
        let (storage, placement) = Placement::<S>::take(placed);
        Self::from_parts::<S>(storage, &placement)
    }

    pub(super) fn from_parts<S: ControllerDmaSpace>(
        storage: &'static mut LeDeviceTableStorage,
        placement: &Placement<S>,
    ) -> Self {
        let first_entry = placement.reference(&storage.layout);
        let mut table = Self {
            storage: Pin::static_mut(storage),
            first_entry,
            members: 0,
        };
        table.clear();
        table
    }

    /// Hardware's view of the table after the last change: its first entry
    /// and the number of entries.
    pub(crate) fn publication(&self) -> DeviceTablePublication {
        let count = u8::try_from(self.members).expect("the capacity fits the count field");
        #[allow(unsafe_code, reason = "the table vouches for its storage")]
        // SAFETY: the table holds its storage `Pin<&'static mut>` from `Placed`;
        // the linker proves its physical placement, `members` never
        // exceeds its capacity, and its words are `VolatileCell`s the CPU
        // reaches only through volatile accesses.
        unsafe {
            DeviceTablePublication::new(self.first_entry, count)
        }
    }

    /// Number of devices in the filter accept list.
    pub const fn len(&self) -> usize {
        self.members
    }

    /// Whether the filter accept list is empty.
    pub const fn is_empty(&self) -> bool {
        self.members == 0
    }

    /// Add `device`. A device already in the list is accepted unchanged.
    pub fn add(&mut self, device: LeFilterAcceptListDevice) -> Result<(), LeDeviceTableError> {
        if self.position(device).is_some() {
            return Ok(());
        }
        if self.members == BLUETOOTH_FILTER_ACCEPT_LIST_CAPACITY {
            return Err(LeDeviceTableError::Full);
        }
        self.write(self.members, device.image());
        self.members += 1;
        Ok(())
    }

    /// Remove `device`; the last entry moves into its slot.
    pub fn remove(&mut self, device: LeFilterAcceptListDevice) -> Result<(), LeDeviceTableError> {
        let index = self.position(device).ok_or(LeDeviceTableError::NotFound)?;
        let last = self.members - 1;
        let moved = self.read(last);
        self.write(index, moved);
        self.write(last, DeviceEntryImage::ZERO);
        self.members = last;
        Ok(())
    }

    /// Remove every device.
    pub fn clear(&mut self) {
        for index in 0..BLUETOOTH_FILTER_ACCEPT_LIST_CAPACITY {
            self.write(index, DeviceEntryImage::ZERO);
        }
        self.members = 0;
    }

    fn position(&self, device: LeFilterAcceptListDevice) -> Option<usize> {
        let words = device.image();
        (0..self.members).find(|&index| self.read(index) == words)
    }

    fn read(&self, index: usize) -> DeviceEntryImage {
        let entry = self
            .storage
            .layout
            .entry(index)
            .expect("member index is inside the table");
        DeviceEntryImage {
            flags_and_address: entry.flags_and_address().read(),
            address_high: entry.address_high().read(),
        }
    }
    fn write(&mut self, index: usize, image: DeviceEntryImage) {
        let entry = self
            .storage
            .layout
            .entry(index)
            .expect("member index is inside the table");
        entry.flags_and_address().write(image.flags_and_address);
        entry.address_high().write(image.address_high);
    }
}

#[cfg(test)]
mod tests;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct DeviceEntryImage {
    flags_and_address: BleDeviceTableEntryFlagsAndAddress,
    address_high: BleDeviceTableEntryAddressHigh,
}
impl DeviceEntryImage {
    const ZERO: Self = Self {
        flags_and_address: BleDeviceTableEntryFlagsAndAddress::ZERO,
        address_high: BleDeviceTableEntryAddressHigh::ZERO,
    };
}
/// SOURCE: pinned device-table writer: an active filter-accept-list entry.
const DEVICE_ENTRY: BleDeviceTableEntryFlagsAndAddress = BleDeviceTableEntryFlagsAndAddress::ZERO
    .with_accept_list_member(true)
    .with_valid(true);
