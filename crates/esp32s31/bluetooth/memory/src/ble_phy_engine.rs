//! Static storage retained by the ESP32-S31 BLE PHY engine.
//!
//! The recovered register-initialization transaction publishes two addresses:
//! a `0x80`-byte BLE environment, three tables referenced by its positional
//! pointer fields, and a controller-SRAM resolving-list object allocated in
//! `0x40`-byte units. The BLE PHY module initialization copies the reviewed
//! channel-frequency and receive packet-start calibration tables into those
//! allocations and records the BTBB transmit-power table: its address at
//! `+0x48`, its length at `+0x4c` and its first and last levels at `+0x78` and
//! `+0x7c`. The register transaction publishes the environment members at
//! `+0x2c` and `+0x4d`. This module owns the complete recovered allocation
//! graph and its stable location. It
//! deliberately does not assign semantics to still-unrecovered words or grant
//! MMIO publication authority.

#![forbid(unsafe_code)]

use core::{marker::PhantomPinned, pin::Pin};

use oer_esp32s31_hal::bluetooth::BluetoothControllerReset;
use vcell::VolatileCell;

use crate::{BLUETOOTH_CONTROLLER_PHYSICAL_SRAM_HIGH, BLUETOOTH_CONTROLLER_PHYSICAL_SRAM_LOW};

use oer_esp32s31_hal::types::{
    BluetoothControllerSramAddress, BluetoothControllerSramAddressError,
    BluetoothPhyEnvironmentAddress, BluetoothPhyEnvironmentAddressError,
};

/// Bytes in the complete recovered BLE PHY environment allocation.
pub const BLUETOOTH_BLE_PHY_ENVIRONMENT_BYTES: usize = 0x80;
/// Bytes in one recovered resolving-list hardware allocation.
pub const BLUETOOTH_BLE_PHY_RESOLVING_LIST_BYTES: usize = 0x40;

const CHANNEL_FREQUENCY_OFFSETS_BYTES: usize = 0x28;
const PACKET_START_OFFSETS_BYTES: usize = 0x08;
const RX_ADDRESS_DELAYS_BYTES: usize = 0x04;
const CHANNEL_FREQUENCY_OFFSETS_POINTER_OFFSET: usize = 0x30;
const PACKET_START_OFFSETS_POINTER_OFFSET: usize = 0x34;
const RX_ADDRESS_DELAYS_POINTER_OFFSET: usize = 0x38;
const TX_POWER_LEVELS_POINTER_OFFSET: usize = 0x48;
const TX_POWER_LEVEL_COUNT_OFFSET: usize = 0x4c;
const TX_POWER_FIRST_LEVEL_OFFSET: usize = 0x78;
const TX_POWER_LAST_LEVEL_OFFSET: usize = 0x7c;
const TX_POWER_LEVELS: [i8; 16] = oer_esp32s31_hal::phy::baseband::TX_POWER_LEVELS_DBM;
const TX_POWER_LEVELS_BYTES: usize = TX_POWER_LEVELS.len();
const RESOLVING_LIST_INITIAL_HEAD_IMAGE: u32 = 0x8000_0000;

const LE_1M_PHY_MODE_INDEX: usize = 1;

#[derive(Clone, Copy)]
enum BlePhyPacketMode {
    LeCodedS2 = 0,
    Le1M = 1,
    Le2M = 2,
    LeCodedS8 = 3,
}

const fn channel_frequency_offsets_mhz() -> [u8; 40] {
    let mut offsets = [0; 40];
    let mut channel = 0;
    while channel < offsets.len() {
        offsets[channel] = match channel {
            0..=10 => 2 + 2 * channel as u8,
            11..=36 => 4 + 2 * channel as u8,
            37 => 0,
            38 => 24,
            39 => 78,
            _ => unreachable!(),
        };
        channel += 1;
    }
    offsets
}

const fn preamble_and_access_address_airtime_micros(mode: BlePhyPacketMode) -> u16 {
    match mode {
        BlePhyPacketMode::Le1M => 40,
        BlePhyPacketMode::Le2M => 24,
        BlePhyPacketMode::LeCodedS2 | BlePhyPacketMode::LeCodedS8 => 336,
    }
}

const fn rx_address_capture_delay_micros(mode: BlePhyPacketMode) -> u8 {
    match mode {
        BlePhyPacketMode::Le1M | BlePhyPacketMode::Le2M => 3,
        BlePhyPacketMode::LeCodedS2 | BlePhyPacketMode::LeCodedS8 => 68,
    }
}

const fn packet_start_offsets_micros() -> [u16; 4] {
    [
        preamble_and_access_address_airtime_micros(BlePhyPacketMode::LeCodedS2),
        preamble_and_access_address_airtime_micros(BlePhyPacketMode::Le1M),
        preamble_and_access_address_airtime_micros(BlePhyPacketMode::Le2M),
        preamble_and_access_address_airtime_micros(BlePhyPacketMode::LeCodedS8),
    ]
}

const fn rx_address_delays_micros() -> [u8; 4] {
    [
        rx_address_capture_delay_micros(BlePhyPacketMode::LeCodedS2),
        rx_address_capture_delay_micros(BlePhyPacketMode::Le1M),
        rx_address_capture_delay_micros(BlePhyPacketMode::Le2M),
        rx_address_capture_delay_micros(BlePhyPacketMode::LeCodedS8),
    ]
}

/// Complete static backing storage required by BLE PHY register publication.
///
/// Unknown environment fields remain opaque until individual controller
/// consumers establish their semantics. Construction installs the reviewed
/// immutable PHY lookup data, but does not claim Link Layer, privacy,
/// advertising, scanning, or connection readiness.
#[repr(C, align(4))]
pub struct BlePhyEngineStorage {
    environment: [VolatileCell<u8>; BLUETOOTH_BLE_PHY_ENVIRONMENT_BYTES],
    channel_frequency_offsets_mhz: [u8; CHANNEL_FREQUENCY_OFFSETS_BYTES],
    packet_start_offsets_micros: [u16; 4],
    rx_address_delays_micros: [u8; RX_ADDRESS_DELAYS_BYTES],
    tx_power_levels_dbm: [i8; TX_POWER_LEVELS_BYTES],
    resolving_list: [VolatileCell<u8>; BLUETOOTH_BLE_PHY_RESOLVING_LIST_BYTES],
    _pin: PhantomPinned,
}

const _: () = {
    assert!(core::mem::align_of::<BlePhyEngineStorage>() == 4);
    assert!(
        core::mem::offset_of!(BlePhyEngineStorage, channel_frequency_offsets_mhz)
            == BLUETOOTH_BLE_PHY_ENVIRONMENT_BYTES
    );
    assert!(
        core::mem::offset_of!(BlePhyEngineStorage, packet_start_offsets_micros)
            == BLUETOOTH_BLE_PHY_ENVIRONMENT_BYTES + CHANNEL_FREQUENCY_OFFSETS_BYTES
    );
    assert!(
        core::mem::offset_of!(BlePhyEngineStorage, rx_address_delays_micros)
            == BLUETOOTH_BLE_PHY_ENVIRONMENT_BYTES
                + CHANNEL_FREQUENCY_OFFSETS_BYTES
                + PACKET_START_OFFSETS_BYTES
    );
    assert!(
        core::mem::offset_of!(BlePhyEngineStorage, tx_power_levels_dbm)
            == BLUETOOTH_BLE_PHY_ENVIRONMENT_BYTES
                + CHANNEL_FREQUENCY_OFFSETS_BYTES
                + PACKET_START_OFFSETS_BYTES
                + RX_ADDRESS_DELAYS_BYTES
    );
    assert!(
        core::mem::offset_of!(BlePhyEngineStorage, resolving_list)
            == BLUETOOTH_BLE_PHY_ENVIRONMENT_BYTES
                + CHANNEL_FREQUENCY_OFFSETS_BYTES
                + PACKET_START_OFFSETS_BYTES
                + RX_ADDRESS_DELAYS_BYTES
                + TX_POWER_LEVELS_BYTES
    );
};

/// Source-owned calibration for one received LE 1M packet timestamp.
///
/// The terms remain private to the BLE PHY SRAM codec. Consumers can normalize
/// a controller-microsecond observation but cannot address or reinterpret the
/// underlying hardware tables.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BlePhyLe1MPacketStartCalibration {
    packet_start_offset_micros: u16,
    rx_address_delay_micros: u8,
}

impl BlePhyLe1MPacketStartCalibration {
    /// The reviewed LE 1M terms that BLE PHY initialization publishes.
    pub const fn le_1m() -> Self {
        Self {
            packet_start_offset_micros: packet_start_offsets_micros()[LE_1M_PHY_MODE_INDEX],
            rx_address_delay_micros: rx_address_delays_micros()[LE_1M_PHY_MODE_INDEX],
        }
    }

    /// Time from the on-air packet start to its receive timestamp.
    pub const fn capture_delay_micros(self) -> u32 {
        self.packet_start_offset_micros as u32 + self.rx_address_delay_micros as u32
    }

    /// Recover the on-air packet-start time from a converted receive timestamp.
    pub const fn normalize_controller_micros(self, captured_micros: u32) -> u32 {
        captured_micros.wrapping_sub(
            self.packet_start_offset_micros as u32 + self.rx_address_delay_micros as u32,
        )
    }
}

/// Why static BLE PHY storage could not acquire a physical address binding.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BlePhyEngineBindError {
    /// A target pointer cannot be represented by the 32-bit S31 address space.
    AddressWidth,
    /// The environment address cannot represent every published member.
    InvalidEnvironment(BluetoothPhyEnvironmentAddressError),
    /// The resolving-list base is outside the controller-pointer domain.
    InvalidResolvingList(BluetoothControllerSramAddressError),
    /// An environment-owned auxiliary base is outside the controller domain.
    InvalidAuxiliary(BluetoothControllerSramAddressError),
    /// Some byte of either object lies outside physical internal SRAM.
    ExtentOutsidePhysicalSram,
}

/// Failed storage binding that returns the exact unchanged allocation.
#[must_use = "failed BLE PHY storage remains available for corrected placement"]
pub struct BlePhyEngineBindFailure {
    storage: &'static mut BlePhyEngineStorage,
    error: BlePhyEngineBindError,
}

impl BlePhyEngineBindFailure {
    fn new(storage: &'static mut BlePhyEngineStorage, error: BlePhyEngineBindError) -> Self {
        Self { storage, error }
    }

    /// Inspect the finite binding failure.
    pub const fn error(&self) -> BlePhyEngineBindError {
        self.error
    }

    /// Recover the unchanged allocation and failure reason.
    pub fn into_parts(self) -> (&'static mut BlePhyEngineStorage, BlePhyEngineBindError) {
        (self.storage, self.error)
    }
}

impl core::fmt::Debug for BlePhyEngineBindFailure {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("BlePhyEngineBindFailure")
            .field("error", &self.error)
            .finish_non_exhaustive()
    }
}

/// Synthetic base accepted only by native ownership models.
#[cfg(not(target_arch = "riscv32"))]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BlePhyEngineModelAddress(BluetoothControllerSramAddress);

#[cfg(not(target_arch = "riscv32"))]
impl BlePhyEngineModelAddress {
    /// Validate one synthetic base without deriving it from a host pointer.
    pub const fn new(address: u32) -> Result<Self, BluetoothControllerSramAddressError> {
        match BluetoothControllerSramAddress::new(address) {
            Ok(address) => Ok(Self(address)),
            Err(error) => Err(error),
        }
    }

    const fn address(self) -> u32 {
        self.0.address()
    }
}

/// Address proof for the complete contiguous allocation graph.
pub struct BlePhyEngineBinding {
    environment: BluetoothPhyEnvironmentAddress,
    channel_frequency_offsets: BluetoothControllerSramAddress,
    packet_start_offsets: BluetoothControllerSramAddress,
    rx_address_delays: BluetoothControllerSramAddress,
    tx_power_levels: BluetoothControllerSramAddress,
    resolving_list: BluetoothControllerSramAddress,
    end_exclusive: u32,
}

impl BlePhyEngineBinding {
    fn new(
        environment: u32,
        channel_frequency_offsets: u32,
        packet_start_offsets: u32,
        rx_address_delays: u32,
        tx_power_levels: u32,
        resolving_list: u32,
    ) -> Result<Self, BlePhyEngineBindError> {
        let environment = BluetoothPhyEnvironmentAddress::new(environment)
            .map_err(BlePhyEngineBindError::InvalidEnvironment)?;
        let channel_frequency_offsets =
            BluetoothControllerSramAddress::new(channel_frequency_offsets)
                .map_err(BlePhyEngineBindError::InvalidAuxiliary)?;
        let packet_start_offsets = BluetoothControllerSramAddress::new(packet_start_offsets)
            .map_err(BlePhyEngineBindError::InvalidAuxiliary)?;
        let rx_address_delays = BluetoothControllerSramAddress::new(rx_address_delays)
            .map_err(BlePhyEngineBindError::InvalidAuxiliary)?;
        let tx_power_levels = BluetoothControllerSramAddress::new(tx_power_levels)
            .map_err(BlePhyEngineBindError::InvalidAuxiliary)?;
        let resolving_list = BluetoothControllerSramAddress::new(resolving_list)
            .map_err(BlePhyEngineBindError::InvalidResolvingList)?;
        let end_exclusive = resolving_list
            .address()
            .checked_add(BLUETOOTH_BLE_PHY_RESOLVING_LIST_BYTES as u32)
            .ok_or(BlePhyEngineBindError::ExtentOutsidePhysicalSram)?;
        let expected_channel_frequency_offsets = environment
            .address()
            .checked_add(BLUETOOTH_BLE_PHY_ENVIRONMENT_BYTES as u32)
            .ok_or(BlePhyEngineBindError::ExtentOutsidePhysicalSram)?;
        let expected_packet_start_offsets = channel_frequency_offsets
            .address()
            .checked_add(CHANNEL_FREQUENCY_OFFSETS_BYTES as u32)
            .ok_or(BlePhyEngineBindError::ExtentOutsidePhysicalSram)?;
        let expected_rx_address_delays = packet_start_offsets
            .address()
            .checked_add(PACKET_START_OFFSETS_BYTES as u32)
            .ok_or(BlePhyEngineBindError::ExtentOutsidePhysicalSram)?;
        let expected_tx_power_levels = rx_address_delays
            .address()
            .checked_add(RX_ADDRESS_DELAYS_BYTES as u32)
            .ok_or(BlePhyEngineBindError::ExtentOutsidePhysicalSram)?;
        let expected_resolving_list = tx_power_levels
            .address()
            .checked_add(TX_POWER_LEVELS_BYTES as u32)
            .ok_or(BlePhyEngineBindError::ExtentOutsidePhysicalSram)?;
        if environment.address() < BLUETOOTH_CONTROLLER_PHYSICAL_SRAM_LOW
            || channel_frequency_offsets.address() != expected_channel_frequency_offsets
            || packet_start_offsets.address() != expected_packet_start_offsets
            || rx_address_delays.address() != expected_rx_address_delays
            || tx_power_levels.address() != expected_tx_power_levels
            || resolving_list.address() != expected_resolving_list
            || end_exclusive > BLUETOOTH_CONTROLLER_PHYSICAL_SRAM_HIGH
        {
            return Err(BlePhyEngineBindError::ExtentOutsidePhysicalSram);
        }
        Ok(Self {
            environment,
            channel_frequency_offsets,
            packet_start_offsets,
            rx_address_delays,
            tx_power_levels,
            resolving_list,
            end_exclusive,
        })
    }

    /// Return the complete physical SRAM range retained by this owner.
    pub const fn range(&self) -> (u32, u32) {
        (self.environment.address(), self.end_exclusive)
    }

    /// Return the typed environment base without granting publication.
    pub const fn environment_address(&self) -> BluetoothPhyEnvironmentAddress {
        self.environment
    }

    /// Return the typed resolving-list base without granting publication.
    pub const fn resolving_list_address(&self) -> BluetoothControllerSramAddress {
        self.resolving_list
    }
}

/// Unique CPU owner of address-bound BLE PHY engine storage.
///
/// The owner intentionally has no mutable-storage or hardware-publication API.
/// A higher lifecycle must consume and retain it while the PAC transaction
/// makes the environment and resolving-list addresses visible to the
/// controller.
#[must_use = "BLE PHY storage must outlive every controller consumer"]
pub struct BlePhyEngineCpuOwned {
    storage: Pin<&'static mut BlePhyEngineStorage>,
    binding: BlePhyEngineBinding,
}

impl BlePhyEngineCpuOwned {
    /// Borrow the stable address proof without changing ownership.
    pub const fn binding(&self) -> &BlePhyEngineBinding {
        &self.binding
    }

    /// Borrow the initialized LE 1M packet-start calibration by value.
    pub fn le_1m_packet_start_calibration(&self) -> BlePhyLe1MPacketStartCalibration {
        let storage = self.storage.as_ref().get_ref();
        BlePhyLe1MPacketStartCalibration {
            packet_start_offset_micros: storage.packet_start_offsets_micros[LE_1M_PHY_MODE_INDEX],
            rx_address_delay_micros: storage.rx_address_delays_micros[LE_1M_PHY_MODE_INDEX],
        }
    }

    /// The environment and resolving-list bytes, for host models.
    #[cfg(test)]
    fn image(&self) -> std::vec::Vec<u8> {
        let storage = self.storage.as_ref().get_ref();
        storage
            .environment
            .iter()
            .chain(&storage.resolving_list)
            .map(VolatileCell::get)
            .collect()
    }

    /// Model hardware writing the environment during an epoch.
    #[cfg(test)]
    fn emulate_hardware_writes(&self) {
        let storage = self.storage.as_ref().get_ref();
        for cell in storage.environment.iter().chain(&storage.resolving_list) {
            cell.set(0xa5);
        }
    }

    /// Return the environment and resolving list to their allocation-time
    /// image after the Controller that used them was reset.
    pub fn reset_after_controller_reset(&mut self, _reset: &BluetoothControllerReset) {
        self.storage
            .as_ref()
            .get_ref()
            .initialize_reviewed_allocation(&self.binding);
    }
}

impl BlePhyEngineStorage {
    /// Reserve storage for the BLE PHY engine and its reviewed immutable data.
    pub const fn new() -> Self {
        Self {
            environment: [const { VolatileCell::new(0) }; BLUETOOTH_BLE_PHY_ENVIRONMENT_BYTES],
            channel_frequency_offsets_mhz: channel_frequency_offsets_mhz(),
            packet_start_offsets_micros: packet_start_offsets_micros(),
            rx_address_delays_micros: rx_address_delays_micros(),
            tx_power_levels_dbm: TX_POWER_LEVELS,
            resolving_list: [const { VolatileCell::new(0) };
                BLUETOOTH_BLE_PHY_RESOLVING_LIST_BYTES],
            _pin: PhantomPinned,
        }
    }

    /// Bind the real locations of one unique static target allocation.
    #[cfg(target_arch = "riscv32")]
    pub fn pin_static(
        storage: &'static mut Self,
    ) -> Result<BlePhyEngineCpuOwned, BlePhyEngineBindFailure> {
        let environment = match u32::try_from(core::ptr::addr_of!(storage.environment).addr()) {
            Ok(address) => address,
            Err(_) => {
                return Err(BlePhyEngineBindFailure::new(
                    storage,
                    BlePhyEngineBindError::AddressWidth,
                ));
            }
        };
        let resolving_list = match u32::try_from(core::ptr::addr_of!(storage.resolving_list).addr())
        {
            Ok(address) => address,
            Err(_) => {
                return Err(BlePhyEngineBindFailure::new(
                    storage,
                    BlePhyEngineBindError::AddressWidth,
                ));
            }
        };
        let channel_frequency_offsets = match u32::try_from(
            core::ptr::addr_of!(storage.channel_frequency_offsets_mhz).addr(),
        ) {
            Ok(address) => address,
            Err(_) => {
                return Err(BlePhyEngineBindFailure::new(
                    storage,
                    BlePhyEngineBindError::AddressWidth,
                ));
            }
        };
        let packet_start_offsets =
            match u32::try_from(core::ptr::addr_of!(storage.packet_start_offsets_micros).addr()) {
                Ok(address) => address,
                Err(_) => {
                    return Err(BlePhyEngineBindFailure::new(
                        storage,
                        BlePhyEngineBindError::AddressWidth,
                    ));
                }
            };
        let rx_address_delays =
            match u32::try_from(core::ptr::addr_of!(storage.rx_address_delays_micros).addr()) {
                Ok(address) => address,
                Err(_) => {
                    return Err(BlePhyEngineBindFailure::new(
                        storage,
                        BlePhyEngineBindError::AddressWidth,
                    ));
                }
            };
        let tx_power_levels =
            match u32::try_from(core::ptr::addr_of!(storage.tx_power_levels_dbm).addr()) {
                Ok(address) => address,
                Err(_) => {
                    return Err(BlePhyEngineBindFailure::new(
                        storage,
                        BlePhyEngineBindError::AddressWidth,
                    ));
                }
            };
        Self::pin_static_inner(
            storage,
            environment,
            channel_frequency_offsets,
            packet_start_offsets,
            rx_address_delays,
            tx_power_levels,
            resolving_list,
        )
    }

    /// Bind a deterministic physical-SRAM base to a native ownership model.
    #[cfg(not(target_arch = "riscv32"))]
    pub fn pin_static_model(
        storage: &'static mut Self,
        base: BlePhyEngineModelAddress,
    ) -> Result<BlePhyEngineCpuOwned, BlePhyEngineBindFailure> {
        let environment = base.address();
        let channel_frequency_offsets = environment + BLUETOOTH_BLE_PHY_ENVIRONMENT_BYTES as u32;
        let packet_start_offsets =
            channel_frequency_offsets + CHANNEL_FREQUENCY_OFFSETS_BYTES as u32;
        let rx_address_delays = packet_start_offsets + PACKET_START_OFFSETS_BYTES as u32;
        let tx_power_levels = rx_address_delays + RX_ADDRESS_DELAYS_BYTES as u32;
        let resolving_list = tx_power_levels + TX_POWER_LEVELS_BYTES as u32;
        Self::pin_static_inner(
            storage,
            environment,
            channel_frequency_offsets,
            packet_start_offsets,
            rx_address_delays,
            tx_power_levels,
            resolving_list,
        )
    }

    fn pin_static_inner(
        storage: &'static mut Self,
        environment: u32,
        channel_frequency_offsets: u32,
        packet_start_offsets: u32,
        rx_address_delays: u32,
        tx_power_levels: u32,
        resolving_list: u32,
    ) -> Result<BlePhyEngineCpuOwned, BlePhyEngineBindFailure> {
        let binding = match BlePhyEngineBinding::new(
            environment,
            channel_frequency_offsets,
            packet_start_offsets,
            rx_address_delays,
            tx_power_levels,
            resolving_list,
        ) {
            Ok(binding) => binding,
            Err(error) => return Err(BlePhyEngineBindFailure::new(storage, error)),
        };
        storage.initialize_reviewed_allocation(&binding);
        Ok(BlePhyEngineCpuOwned {
            storage: Pin::static_mut(storage),
            binding,
        })
    }

    /// Write the allocation-time image: a zeroed environment and resolving
    /// list with the reviewed table pointers, the transmit-power table record
    /// and the resolving-list head.
    fn initialize_reviewed_allocation(&self, binding: &BlePhyEngineBinding) {
        let write = |cells: &[VolatileCell<u8>], offset: usize, bytes: [u8; 4]| {
            for (cell, byte) in cells[offset..offset + 4].iter().zip(bytes) {
                cell.set(byte);
            }
        };
        for cell in self.environment.iter().chain(&self.resolving_list) {
            cell.set(0);
        }
        write(
            &self.environment,
            CHANNEL_FREQUENCY_OFFSETS_POINTER_OFFSET,
            binding.channel_frequency_offsets.address().to_le_bytes(),
        );
        write(
            &self.environment,
            PACKET_START_OFFSETS_POINTER_OFFSET,
            binding.packet_start_offsets.address().to_le_bytes(),
        );
        write(
            &self.environment,
            RX_ADDRESS_DELAYS_POINTER_OFFSET,
            binding.rx_address_delays.address().to_le_bytes(),
        );
        write(
            &self.environment,
            TX_POWER_LEVELS_POINTER_OFFSET,
            binding.tx_power_levels.address().to_le_bytes(),
        );
        self.environment[TX_POWER_LEVEL_COUNT_OFFSET].set(TX_POWER_LEVELS_BYTES as u8);
        // The first and last levels are stored sign-extended to words.
        write(
            &self.environment,
            TX_POWER_FIRST_LEVEL_OFFSET,
            i32::from(TX_POWER_LEVELS[0]).to_le_bytes(),
        );
        write(
            &self.environment,
            TX_POWER_LAST_LEVEL_OFFSET,
            i32::from(TX_POWER_LEVELS[TX_POWER_LEVELS_BYTES - 1]).to_le_bytes(),
        );
        write(
            &self.resolving_list,
            0,
            RESOLVING_LIST_INITIAL_HEAD_IMAGE.to_le_bytes(),
        );
    }
}

impl Default for BlePhyEngineStorage {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests;
