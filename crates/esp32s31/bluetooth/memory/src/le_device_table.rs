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
//! [`LeDeviceTablePublication`] after every change.

#![forbid(unsafe_code)]

use core::pin::Pin;

use oer_esp32s31_hal::bluetooth::BluetoothDeviceTableEntryCount;
use oer_esp32s31_hal::types::{
    BluetoothControllerSramAddress, BluetoothControllerSramAddressError,
};
use vcell::VolatileCell;

use crate::sram_link::{
    BLUETOOTH_CONTROLLER_PHYSICAL_SRAM_HIGH, BLUETOOTH_CONTROLLER_PHYSICAL_SRAM_LOW,
};

/// Filter accept list entries the Controller keeps, as the S31 build's LE
/// Read Filter Accept List Size reports.
pub const BLUETOOTH_FILTER_ACCEPT_LIST_CAPACITY: usize = 12;

const ENTRY_WORDS: usize = 2;
const ACCEPT_LIST_MEMBER: u32 = 1 << 1;
const VALID: u32 = 1 << 15;
const RANDOM: u32 = 1 << 14;

/// One filter accept list device: its address type and address.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LeFilterAcceptListDevice {
    /// The address is random rather than public.
    pub random: bool,
    /// The address, least significant octet first.
    pub address: [u8; 6],
}

impl LeFilterAcceptListDevice {
    fn words(self) -> [u32; ENTRY_WORDS] {
        let [a0, a1, a2, a3, a4, a5] = self.address;
        let flags = ACCEPT_LIST_MEMBER | VALID | if self.random { RANDOM } else { 0 };
        [
            flags | u32::from(a0) << 16 | u32::from(a1) << 24,
            u32::from_le_bytes([a2, a3, a4, a5]),
        ]
    }
}

/// Pinned storage of the device table.
#[repr(C, align(4))]
pub struct LeDeviceTableStorage {
    words: [VolatileCell<u32>; BLUETOOTH_FILTER_ACCEPT_LIST_CAPACITY * ENTRY_WORDS],
}

impl LeDeviceTableStorage {
    pub const fn new() -> Self {
        Self {
            words: [const { VolatileCell::new(0) };
                BLUETOOTH_FILTER_ACCEPT_LIST_CAPACITY * ENTRY_WORDS],
        }
    }
}

impl Default for LeDeviceTableStorage {
    fn default() -> Self {
        Self::new()
    }
}

/// Why the table was not bound.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LeDeviceTableBindError {
    /// The storage address is not a 32-bit address.
    AddressWidth,
    /// The storage is not representable as a Controller-SRAM address.
    InvalidAddress(BluetoothControllerSramAddressError),
    /// The storage extends beyond the physical Controller SRAM.
    ExtentOutsidePhysicalSram,
}

/// Why a filter accept list change was refused.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LeDeviceTableError {
    /// Every entry is in use.
    Full,
    /// The device is not in the list.
    NotFound,
}

/// What hardware walks: the first entry and the number of entries.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LeDeviceTablePublication {
    /// Address of the first entry.
    pub first_entry: BluetoothControllerSramAddress,
    /// Number of entries.
    pub count: BluetoothDeviceTableEntryCount,
}

/// The device table bound at its Controller-SRAM address.
pub struct LeDeviceTable {
    storage: Pin<&'static mut LeDeviceTableStorage>,
    first_entry: BluetoothControllerSramAddress,
    members: usize,
}

impl LeDeviceTable {
    /// Bind static storage at its linked address.
    #[cfg(target_arch = "riscv32")]
    pub fn bind(
        storage: &'static mut LeDeviceTableStorage,
    ) -> Result<Self, LeDeviceTableBindError> {
        let base = u32::try_from(core::ptr::addr_of!(*storage).addr())
            .map_err(|_| LeDeviceTableBindError::AddressWidth)?;
        Self::bind_at(storage, base)
    }

    /// Bind static storage at a synthetic Controller-SRAM address.
    #[cfg(not(target_arch = "riscv32"))]
    pub fn bind_model(
        storage: &'static mut LeDeviceTableStorage,
        base: u32,
    ) -> Result<Self, LeDeviceTableBindError> {
        Self::bind_at(storage, base)
    }

    fn bind_at(
        storage: &'static mut LeDeviceTableStorage,
        base: u32,
    ) -> Result<Self, LeDeviceTableBindError> {
        let bytes = core::mem::size_of::<LeDeviceTableStorage>() as u32;
        if base < BLUETOOTH_CONTROLLER_PHYSICAL_SRAM_LOW
            || bytes > BLUETOOTH_CONTROLLER_PHYSICAL_SRAM_HIGH.saturating_sub(base)
        {
            return Err(LeDeviceTableBindError::ExtentOutsidePhysicalSram);
        }
        let first_entry = BluetoothControllerSramAddress::new(base)
            .map_err(LeDeviceTableBindError::InvalidAddress)?;
        let mut table = Self {
            storage: Pin::static_mut(storage),
            first_entry,
            members: 0,
        };
        table.clear();
        Ok(table)
    }

    /// Hardware's view of the table after the last change.
    pub fn publication(&self) -> LeDeviceTablePublication {
        LeDeviceTablePublication {
            first_entry: self.first_entry,
            count: BluetoothDeviceTableEntryCount::new(self.members as u32)
                .expect("the capacity fits the count field"),
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
        self.write(self.members, device.words());
        self.members += 1;
        Ok(())
    }

    /// Remove `device`; the last entry moves into its slot.
    pub fn remove(&mut self, device: LeFilterAcceptListDevice) -> Result<(), LeDeviceTableError> {
        let index = self.position(device).ok_or(LeDeviceTableError::NotFound)?;
        let last = self.members - 1;
        let moved = self.read(last);
        self.write(index, moved);
        self.write(last, [0; ENTRY_WORDS]);
        self.members = last;
        Ok(())
    }

    /// Remove every device.
    pub fn clear(&mut self) {
        for word in &self.storage.words {
            word.set(0);
        }
        self.members = 0;
    }

    fn position(&self, device: LeFilterAcceptListDevice) -> Option<usize> {
        let words = device.words();
        (0..self.members).find(|&index| self.read(index) == words)
    }

    fn read(&self, index: usize) -> [u32; ENTRY_WORDS] {
        core::array::from_fn(|word| self.storage.words[index * ENTRY_WORDS + word].get())
    }

    fn write(&mut self, index: usize, words: [u32; ENTRY_WORDS]) {
        for (word, value) in words.into_iter().enumerate() {
            self.storage.words[index * ENTRY_WORDS + word].set(value);
        }
    }
}

#[cfg(test)]
mod tests;
