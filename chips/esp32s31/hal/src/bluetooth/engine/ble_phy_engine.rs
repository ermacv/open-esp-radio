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

#![deny(unsafe_code, clippy::undocumented_unsafe_blocks)]

use core::{marker::PhantomPinned, pin::Pin};

use crate::bluetooth::BluetoothControllerReset;
use crate::layouts::*;

use crate::bluetooth::engine::{ControllerDmaSpace, Placement};
use crate::dma_static::Placed;

/// Bytes in the complete recovered BLE PHY environment allocation.
pub const BLUETOOTH_BLE_PHY_ENVIRONMENT_BYTES: usize = BlePhyEnvironment::SIZE;
/// Bytes in one recovered resolving-list hardware allocation.
pub const BLUETOOTH_BLE_PHY_RESOLVING_LIST_BYTES: usize = BleResolvingList::SIZE;

const TX_POWER_LEVELS: [i8; 16] = oer_esp32s31_knowledge::tx_power::TX_POWER_LEVELS_DBM;
const TX_POWER_LEVELS_BYTES: usize = TX_POWER_LEVELS.len();
/// SOURCE: pinned BLE PHY allocation, resolving-list initial opaque head.
const RESOLVING_LIST_INITIAL_HEAD: BleResolvingListWord0000Opaque =
    BleResolvingListWord0000Opaque::ZERO.with_bit_31_opaque(true);

#[derive(Clone, Copy)]
enum BlePhyPacketMode {
    LeCodedS2,
    Le1M,
    Le2M,
    LeCodedS8,
}

const fn channel_frequency_offset_mhz(channel: usize) -> u8 {
    match channel {
        0..=10 => 2 + 2 * channel as u8,
        11..=36 => 4 + 2 * channel as u8,
        37 => 0,
        38 => 24,
        39 => 78,
        _ => panic!("a generated channel field names one of the forty LE channels"),
    }
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

/// Complete static backing storage required by BLE PHY register publication.
///
/// Unknown environment fields remain opaque until individual controller
/// consumers establish their semantics. Placing installs the reviewed
/// immutable PHY lookup data, but does not claim Link Layer, privacy,
/// advertising, scanning, or connection readiness.
#[repr(C, align(4))]
#[derive(bytemuck::Zeroable)]
pub struct BlePhyEngineStorage {
    environment: BlePhyEnvironment,
    channel_frequency_offsets_mhz: [u8; BlePhyChannelFrequencyOffsets::SIZE],
    packet_start_offsets_micros: [u8; BlePhyPacketStartOffsets::SIZE],
    rx_address_delays_micros: [u8; BlePhyRxAddressDelays::SIZE],
    tx_power_levels_dbm: [i8; TX_POWER_LEVELS_BYTES],
    resolving_list: BleResolvingList,
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
            == BLUETOOTH_BLE_PHY_ENVIRONMENT_BYTES + BlePhyChannelFrequencyOffsets::SIZE
    );
    assert!(
        core::mem::offset_of!(BlePhyEngineStorage, rx_address_delays_micros)
            == BLUETOOTH_BLE_PHY_ENVIRONMENT_BYTES
                + BlePhyChannelFrequencyOffsets::SIZE
                + BlePhyPacketStartOffsets::SIZE
    );
    assert!(
        core::mem::offset_of!(BlePhyEngineStorage, tx_power_levels_dbm)
            == BLUETOOTH_BLE_PHY_ENVIRONMENT_BYTES
                + BlePhyChannelFrequencyOffsets::SIZE
                + BlePhyPacketStartOffsets::SIZE
                + BlePhyRxAddressDelays::SIZE
    );
    assert!(
        core::mem::offset_of!(BlePhyEngineStorage, resolving_list)
            == BLUETOOTH_BLE_PHY_ENVIRONMENT_BYTES
                + BlePhyChannelFrequencyOffsets::SIZE
                + BlePhyPacketStartOffsets::SIZE
                + BlePhyRxAddressDelays::SIZE
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
            packet_start_offset_micros: preamble_and_access_address_airtime_micros(
                BlePhyPacketMode::Le1M,
            ),
            rx_address_delay_micros: rx_address_capture_delay_micros(BlePhyPacketMode::Le1M),
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

/// Address proof for the complete contiguous allocation graph.
pub(crate) struct BlePhyEngineBinding {
    environment: DmaRef<BlePhyEnvironment>,
    channel_frequency_offsets: DmaWindowAddress,
    packet_start_offsets: DmaWindowAddress,
    rx_address_delays: DmaWindowAddress,
    tx_power_levels: DmaWindowAddress,
    resolving_list: DmaRef<BleResolvingList>,
}

impl BlePhyEngineBinding {
    fn new<S: ControllerDmaSpace>(storage: &BlePhyEngineStorage, placement: &Placement<S>) -> Self {
        Self {
            environment: placement.reference(&storage.environment),
            channel_frequency_offsets: placement.address(&storage.channel_frequency_offsets_mhz),
            packet_start_offsets: placement.address(&storage.packet_start_offsets_micros),
            rx_address_delays: placement.address(&storage.rx_address_delays_micros),
            tx_power_levels: placement.address(&storage.tx_power_levels_dbm),
            resolving_list: placement.reference(&storage.resolving_list),
        }
    }
    /// The environment layout in this retained placed allocation.
    pub const fn environment_address(&self) -> DmaRef<BlePhyEnvironment> {
        self.environment
    }
    /// The resolving list in the same allocation.
    pub const fn resolving_list_address(&self) -> DmaRef<BleResolvingList> {
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
    /// Publish only this owner's retained PHY graph through the powered task.
    #[cfg(target_arch = "riscv32")]
    pub fn initialize_registers<B: crate::power::PlatformClockBackend, T>(
        &self,
        task: &mut crate::bluetooth::TaskOwner,
        radio: &mut crate::domain::RadioDomain<B, T>,
    ) {
        task.enable_ble_base_stack_hardware(radio, &self.hardware_objects());
    }

    /// Borrow the stable address proof without changing ownership.
    #[cfg(test)]
    pub(crate) const fn binding(&self) -> &BlePhyEngineBinding {
        &self.binding
    }

    /// The environment and resolving list of this storage, as the base-stack
    /// enable points the hardware at them under the normal Controller
    /// profile.
    pub(crate) fn hardware_objects(&self) -> crate::bluetooth::BlePhyHardwareObjects {
        #[allow(unsafe_code, reason = "the owner of the storage vouches for it")]
        // SAFETY: this owner holds the storage `Pin<&'static mut>` for as long
        // as it lives, its binding names that storage's environment and
        // resolving list, and both are `VolatileCell` arrays the CPU reaches
        // only through volatile accesses.
        unsafe {
            let inputs = crate::types::BluetoothPhyRegisterInitInputs::normal_controller_profile(
                self.binding.environment_address(),
                self.binding.resolving_list_address(),
            );
            crate::bluetooth::BlePhyHardwareObjects::bind(inputs)
        }
    }

    /// Borrow the initialized LE 1M packet-start calibration by value.
    pub fn le_1m_packet_start_calibration(&self) -> BlePhyLe1MPacketStartCalibration {
        let storage = self.storage.as_ref().get_ref();
        BlePhyLe1MPacketStartCalibration {
            packet_start_offset_micros: BlePhyPacketStartOffsets::new(
                &storage.packet_start_offsets_micros,
            )
            .le_1m()
            .micros(),
            rx_address_delay_micros: BlePhyRxAddressDelays::new(&storage.rx_address_delays_micros)
                .le_1m()
                .micros(),
        }
    }

    /// The environment and resolving-list bytes, for host models.
    #[cfg(test)]
    fn image(&self) -> std::vec::Vec<u8> {
        let storage = self.storage.as_ref().get_ref();
        let mut bytes = std::vec::Vec::new();
        bytes.extend(
            storage
                .environment
                .word_0000_opaque()
                .read()
                .bits()
                .to_le_bytes(),
        );
        bytes.extend(
            storage
                .environment
                .word_0004_opaque()
                .read()
                .bits()
                .to_le_bytes(),
        );
        bytes.extend(
            storage
                .environment
                .word_0008_opaque()
                .read()
                .bits()
                .to_le_bytes(),
        );
        bytes.extend(
            storage
                .environment
                .word_000c_opaque()
                .read()
                .bits()
                .to_le_bytes(),
        );
        bytes.extend(
            storage
                .environment
                .word_0010_opaque()
                .read()
                .bits()
                .to_le_bytes(),
        );
        bytes.extend(
            storage
                .environment
                .word_0014_opaque()
                .read()
                .bits()
                .to_le_bytes(),
        );
        bytes.extend(
            storage
                .environment
                .word_0018_opaque()
                .read()
                .bits()
                .to_le_bytes(),
        );
        bytes.extend(
            storage
                .environment
                .word_001c_opaque()
                .read()
                .bits()
                .to_le_bytes(),
        );
        bytes.extend(
            storage
                .environment
                .word_0020_opaque()
                .read()
                .bits()
                .to_le_bytes(),
        );
        bytes.extend(
            storage
                .environment
                .word_0024_opaque()
                .read()
                .bits()
                .to_le_bytes(),
        );
        bytes.extend(
            storage
                .environment
                .word_0028_opaque()
                .read()
                .bits()
                .to_le_bytes(),
        );
        bytes.extend(
            storage
                .environment
                .word_002c_opaque()
                .read()
                .bits()
                .to_le_bytes(),
        );
        bytes.extend(
            storage
                .environment
                .channel_frequency_offsets()
                .read()
                .bits()
                .to_le_bytes(),
        );
        bytes.extend(
            storage
                .environment
                .packet_start_offsets()
                .read()
                .bits()
                .to_le_bytes(),
        );
        bytes.extend(
            storage
                .environment
                .rx_address_delays()
                .read()
                .bits()
                .to_le_bytes(),
        );
        bytes.extend(
            storage
                .environment
                .word_003c_opaque()
                .read()
                .bits()
                .to_le_bytes(),
        );
        bytes.extend(
            storage
                .environment
                .word_0040_opaque()
                .read()
                .bits()
                .to_le_bytes(),
        );
        bytes.extend(
            storage
                .environment
                .word_0044_opaque()
                .read()
                .bits()
                .to_le_bytes(),
        );
        bytes.extend(
            storage
                .environment
                .tx_power_levels()
                .read()
                .bits()
                .to_le_bytes(),
        );
        bytes.extend(
            storage
                .environment
                .tx_power_level_count()
                .read()
                .bits()
                .to_le_bytes(),
        );
        bytes.extend(
            storage
                .environment
                .word_0050_opaque()
                .read()
                .bits()
                .to_le_bytes(),
        );
        bytes.extend(
            storage
                .environment
                .word_0054_opaque()
                .read()
                .bits()
                .to_le_bytes(),
        );
        bytes.extend(
            storage
                .environment
                .word_0058_opaque()
                .read()
                .bits()
                .to_le_bytes(),
        );
        bytes.extend(
            storage
                .environment
                .word_005c_opaque()
                .read()
                .bits()
                .to_le_bytes(),
        );
        bytes.extend(
            storage
                .environment
                .word_0060_opaque()
                .read()
                .bits()
                .to_le_bytes(),
        );
        bytes.extend(
            storage
                .environment
                .word_0064_opaque()
                .read()
                .bits()
                .to_le_bytes(),
        );
        bytes.extend(
            storage
                .environment
                .word_0068_opaque()
                .read()
                .bits()
                .to_le_bytes(),
        );
        bytes.extend(
            storage
                .environment
                .word_006c_opaque()
                .read()
                .bits()
                .to_le_bytes(),
        );
        bytes.extend(
            storage
                .environment
                .word_0070_opaque()
                .read()
                .bits()
                .to_le_bytes(),
        );
        bytes.extend(
            storage
                .environment
                .word_0074_opaque()
                .read()
                .bits()
                .to_le_bytes(),
        );
        bytes.extend(
            storage
                .environment
                .tx_power_first_level()
                .read()
                .bits()
                .to_le_bytes(),
        );
        bytes.extend(
            storage
                .environment
                .tx_power_last_level()
                .read()
                .bits()
                .to_le_bytes(),
        );
        bytes.extend(
            storage
                .resolving_list
                .word_0000_opaque()
                .read()
                .bits()
                .to_le_bytes(),
        );
        bytes.extend(
            storage
                .resolving_list
                .word_0004_opaque()
                .read()
                .bits()
                .to_le_bytes(),
        );
        bytes.extend(
            storage
                .resolving_list
                .word_0008_opaque()
                .read()
                .bits()
                .to_le_bytes(),
        );
        bytes.extend(
            storage
                .resolving_list
                .word_000c_opaque()
                .read()
                .bits()
                .to_le_bytes(),
        );
        bytes.extend(
            storage
                .resolving_list
                .word_0010_opaque()
                .read()
                .bits()
                .to_le_bytes(),
        );
        bytes.extend(
            storage
                .resolving_list
                .word_0014_opaque()
                .read()
                .bits()
                .to_le_bytes(),
        );
        bytes.extend(
            storage
                .resolving_list
                .word_0018_opaque()
                .read()
                .bits()
                .to_le_bytes(),
        );
        bytes.extend(
            storage
                .resolving_list
                .word_001c_opaque()
                .read()
                .bits()
                .to_le_bytes(),
        );
        bytes.extend(
            storage
                .resolving_list
                .word_0020_opaque()
                .read()
                .bits()
                .to_le_bytes(),
        );
        bytes.extend(
            storage
                .resolving_list
                .word_0024_opaque()
                .read()
                .bits()
                .to_le_bytes(),
        );
        bytes.extend(
            storage
                .resolving_list
                .word_0028_opaque()
                .read()
                .bits()
                .to_le_bytes(),
        );
        bytes.extend(
            storage
                .resolving_list
                .word_002c_opaque()
                .read()
                .bits()
                .to_le_bytes(),
        );
        bytes.extend(
            storage
                .resolving_list
                .word_0030_opaque()
                .read()
                .bits()
                .to_le_bytes(),
        );
        bytes.extend(
            storage
                .resolving_list
                .word_0034_opaque()
                .read()
                .bits()
                .to_le_bytes(),
        );
        bytes.extend(
            storage
                .resolving_list
                .word_0038_opaque()
                .read()
                .bits()
                .to_le_bytes(),
        );
        bytes.extend(
            storage
                .resolving_list
                .word_003c_opaque()
                .read()
                .bits()
                .to_le_bytes(),
        );
        bytes
    }

    /// Model hardware writing the environment during an epoch.
    #[cfg(test)]
    fn emulate_hardware_writes(&self) {
        let storage = self.storage.as_ref().get_ref();
        storage
            .environment
            .word_0000_opaque()
            .write(BlePhyEnvironmentWord0000Opaque::from_bits(0xa5a5_a5a5));
        storage
            .environment
            .word_0004_opaque()
            .write(BlePhyEnvironmentWord0004Opaque::from_bits(0xa5a5_a5a5));
        storage
            .environment
            .word_0008_opaque()
            .write(BlePhyEnvironmentWord0008Opaque::from_bits(0xa5a5_a5a5));
        storage
            .environment
            .word_000c_opaque()
            .write(BlePhyEnvironmentWord000cOpaque::from_bits(0xa5a5_a5a5));
        storage
            .environment
            .word_0010_opaque()
            .write(BlePhyEnvironmentWord0010Opaque::from_bits(0xa5a5_a5a5));
        storage
            .environment
            .word_0014_opaque()
            .write(BlePhyEnvironmentWord0014Opaque::from_bits(0xa5a5_a5a5));
        storage
            .environment
            .word_0018_opaque()
            .write(BlePhyEnvironmentWord0018Opaque::from_bits(0xa5a5_a5a5));
        storage
            .environment
            .word_001c_opaque()
            .write(BlePhyEnvironmentWord001cOpaque::from_bits(0xa5a5_a5a5));
        storage
            .environment
            .word_0020_opaque()
            .write(BlePhyEnvironmentWord0020Opaque::from_bits(0xa5a5_a5a5));
        storage
            .environment
            .word_0024_opaque()
            .write(BlePhyEnvironmentWord0024Opaque::from_bits(0xa5a5_a5a5));
        storage
            .environment
            .word_0028_opaque()
            .write(BlePhyEnvironmentWord0028Opaque::from_bits(0xa5a5_a5a5));
        storage
            .environment
            .word_002c_opaque()
            .write(BlePhyEnvironmentWord002cOpaque::from_bits(0xa5a5_a5a5));
        storage.environment.channel_frequency_offsets().write(
            BlePhyEnvironmentChannelFrequencyOffsets::from_bits(0xa5a5_a5a5),
        );
        storage
            .environment
            .packet_start_offsets()
            .write(BlePhyEnvironmentPacketStartOffsets::from_bits(0xa5a5_a5a5));
        storage
            .environment
            .rx_address_delays()
            .write(BlePhyEnvironmentRxAddressDelays::from_bits(0xa5a5_a5a5));
        storage
            .environment
            .word_003c_opaque()
            .write(BlePhyEnvironmentWord003cOpaque::from_bits(0xa5a5_a5a5));
        storage
            .environment
            .word_0040_opaque()
            .write(BlePhyEnvironmentWord0040Opaque::from_bits(0xa5a5_a5a5));
        storage
            .environment
            .word_0044_opaque()
            .write(BlePhyEnvironmentWord0044Opaque::from_bits(0xa5a5_a5a5));
        storage
            .environment
            .tx_power_levels()
            .write(BlePhyEnvironmentTxPowerLevels::from_bits(0xa5a5_a5a5));
        storage
            .environment
            .tx_power_level_count()
            .write(BlePhyEnvironmentTxPowerLevelCount::from_bits(0xa5a5_a5a5));
        storage
            .environment
            .word_0050_opaque()
            .write(BlePhyEnvironmentWord0050Opaque::from_bits(0xa5a5_a5a5));
        storage
            .environment
            .word_0054_opaque()
            .write(BlePhyEnvironmentWord0054Opaque::from_bits(0xa5a5_a5a5));
        storage
            .environment
            .word_0058_opaque()
            .write(BlePhyEnvironmentWord0058Opaque::from_bits(0xa5a5_a5a5));
        storage
            .environment
            .word_005c_opaque()
            .write(BlePhyEnvironmentWord005cOpaque::from_bits(0xa5a5_a5a5));
        storage
            .environment
            .word_0060_opaque()
            .write(BlePhyEnvironmentWord0060Opaque::from_bits(0xa5a5_a5a5));
        storage
            .environment
            .word_0064_opaque()
            .write(BlePhyEnvironmentWord0064Opaque::from_bits(0xa5a5_a5a5));
        storage
            .environment
            .word_0068_opaque()
            .write(BlePhyEnvironmentWord0068Opaque::from_bits(0xa5a5_a5a5));
        storage
            .environment
            .word_006c_opaque()
            .write(BlePhyEnvironmentWord006cOpaque::from_bits(0xa5a5_a5a5));
        storage
            .environment
            .word_0070_opaque()
            .write(BlePhyEnvironmentWord0070Opaque::from_bits(0xa5a5_a5a5));
        storage
            .environment
            .word_0074_opaque()
            .write(BlePhyEnvironmentWord0074Opaque::from_bits(0xa5a5_a5a5));
        storage
            .environment
            .tx_power_first_level()
            .write(BlePhyEnvironmentTxPowerFirstLevel::from_bits(0xa5a5_a5a5));
        storage
            .environment
            .tx_power_last_level()
            .write(BlePhyEnvironmentTxPowerLastLevel::from_bits(0xa5a5_a5a5));
        storage
            .resolving_list
            .word_0000_opaque()
            .write(BleResolvingListWord0000Opaque::from_bits(0xa5a5_a5a5));
        storage
            .resolving_list
            .word_0004_opaque()
            .write(BleResolvingListWord0004Opaque::from_bits(0xa5a5_a5a5));
        storage
            .resolving_list
            .word_0008_opaque()
            .write(BleResolvingListWord0008Opaque::from_bits(0xa5a5_a5a5));
        storage
            .resolving_list
            .word_000c_opaque()
            .write(BleResolvingListWord000cOpaque::from_bits(0xa5a5_a5a5));
        storage
            .resolving_list
            .word_0010_opaque()
            .write(BleResolvingListWord0010Opaque::from_bits(0xa5a5_a5a5));
        storage
            .resolving_list
            .word_0014_opaque()
            .write(BleResolvingListWord0014Opaque::from_bits(0xa5a5_a5a5));
        storage
            .resolving_list
            .word_0018_opaque()
            .write(BleResolvingListWord0018Opaque::from_bits(0xa5a5_a5a5));
        storage
            .resolving_list
            .word_001c_opaque()
            .write(BleResolvingListWord001cOpaque::from_bits(0xa5a5_a5a5));
        storage
            .resolving_list
            .word_0020_opaque()
            .write(BleResolvingListWord0020Opaque::from_bits(0xa5a5_a5a5));
        storage
            .resolving_list
            .word_0024_opaque()
            .write(BleResolvingListWord0024Opaque::from_bits(0xa5a5_a5a5));
        storage
            .resolving_list
            .word_0028_opaque()
            .write(BleResolvingListWord0028Opaque::from_bits(0xa5a5_a5a5));
        storage
            .resolving_list
            .word_002c_opaque()
            .write(BleResolvingListWord002cOpaque::from_bits(0xa5a5_a5a5));
        storage
            .resolving_list
            .word_0030_opaque()
            .write(BleResolvingListWord0030Opaque::from_bits(0xa5a5_a5a5));
        storage
            .resolving_list
            .word_0034_opaque()
            .write(BleResolvingListWord0034Opaque::from_bits(0xa5a5_a5a5));
        storage
            .resolving_list
            .word_0038_opaque()
            .write(BleResolvingListWord0038Opaque::from_bits(0xa5a5_a5a5));
        storage
            .resolving_list
            .word_003c_opaque()
            .write(BleResolvingListWord003cOpaque::from_bits(0xa5a5_a5a5));
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
            environment: BlePhyEnvironment::zeroed(),
            channel_frequency_offsets_mhz: [0; BlePhyChannelFrequencyOffsets::SIZE],
            packet_start_offsets_micros: [0; BlePhyPacketStartOffsets::SIZE],
            rx_address_delays_micros: [0; BlePhyRxAddressDelays::SIZE],
            tx_power_levels_dbm: TX_POWER_LEVELS,
            resolving_list: BleResolvingList::zeroed(),
            _pin: PhantomPinned,
        }
    }

    /// Claim the linker-proven allocation and initialize its retained hardware objects.
    ///
    /// Raw static storage does not provide placement proof:
    ///
    /// ```compile_fail,E0308
    /// use oer_esp32s31_hal::bluetooth::engine::{BlePhyEngineStorage, ControllerDmaSpace};
    /// fn from_raw<S: ControllerDmaSpace>(storage: &'static mut BlePhyEngineStorage) {
    ///     BlePhyEngineStorage::place::<S>(storage);
    /// }
    /// ```
    pub fn place<S: ControllerDmaSpace>(placed: Placed<Self>) -> BlePhyEngineCpuOwned {
        let (storage, placement) = Placement::<S>::take(placed);
        Self::from_parts::<S>(storage, &placement)
    }

    pub(super) fn from_parts<S: ControllerDmaSpace>(
        storage: &'static mut Self,
        placement: &Placement<S>,
    ) -> BlePhyEngineCpuOwned {
        storage.initialize_lookup_tables();
        let binding = BlePhyEngineBinding::new(storage, placement);
        storage.initialize_reviewed_allocation(&binding);
        BlePhyEngineCpuOwned {
            storage: Pin::static_mut(storage),
            binding,
        }
    }

    fn initialize_lookup_tables(&mut self) {
        // SOURCE: BLOB_LIBBLE_MEMORY_PHY_LOOKUPS, bt-lib pin 10c50778;
        // generated BLE_PHY_* byte layouts and notes/bluetooth-controller-boundary.md.
        let mut frequencies =
            BlePhyChannelFrequencyOffsetsMut::new(&mut self.channel_frequency_offsets_mhz);
        for channel in 0..BlePhyChannelFrequencyOffsets::COUNT {
            frequencies
                .entry(channel)
                .expect("the internal LE channel index fits its generated lookup")
                .set_value(
                    BlePhyChannelFrequencyOffsetValue::from_bits(0)
                        .with_offset_mhz(channel_frequency_offset_mhz(channel)),
                );
        }
        let mut airtime = BlePhyPacketStartOffsetsMut::new(&mut self.packet_start_offsets_micros);
        airtime.set_le_coded_s2(BlePhyPacketStartOffsetsLeCodedS2::from_bits(0).with_micros(
            preamble_and_access_address_airtime_micros(BlePhyPacketMode::LeCodedS2),
        ));
        airtime.set_le_1m(BlePhyPacketStartOffsetsLe1m::from_bits(0).with_micros(
            preamble_and_access_address_airtime_micros(BlePhyPacketMode::Le1M),
        ));
        airtime.set_le_2m(BlePhyPacketStartOffsetsLe2m::from_bits(0).with_micros(
            preamble_and_access_address_airtime_micros(BlePhyPacketMode::Le2M),
        ));
        airtime.set_le_coded_s8(BlePhyPacketStartOffsetsLeCodedS8::from_bits(0).with_micros(
            preamble_and_access_address_airtime_micros(BlePhyPacketMode::LeCodedS8),
        ));
        let mut delays = BlePhyRxAddressDelaysMut::new(&mut self.rx_address_delays_micros);
        delays.set_le_coded_s2(
            BlePhyRxAddressDelaysLeCodedS2::from_bits(0)
                .with_micros(rx_address_capture_delay_micros(BlePhyPacketMode::LeCodedS2)),
        );
        delays.set_le_1m(
            BlePhyRxAddressDelaysLe1m::from_bits(0)
                .with_micros(rx_address_capture_delay_micros(BlePhyPacketMode::Le1M)),
        );
        delays.set_le_2m(
            BlePhyRxAddressDelaysLe2m::from_bits(0)
                .with_micros(rx_address_capture_delay_micros(BlePhyPacketMode::Le2M)),
        );
        delays.set_le_coded_s8(
            BlePhyRxAddressDelaysLeCodedS8::from_bits(0)
                .with_micros(rx_address_capture_delay_micros(BlePhyPacketMode::LeCodedS8)),
        );
        self.tx_power_levels_dbm = TX_POWER_LEVELS;
    }

    /// Write the allocation-time image: a zeroed environment and resolving
    /// list with the reviewed table pointers, the transmit-power table record
    /// and the resolving-list head.
    fn initialize_reviewed_allocation(&self, binding: &BlePhyEngineBinding) {
        crate::bluetooth::engine::shared_layout::clear_phy_environment(&self.environment);
        crate::bluetooth::engine::shared_layout::clear_resolving_list(&self.resolving_list);
        self.environment.channel_frequency_offsets().write(
            BlePhyEnvironmentChannelFrequencyOffsets::ZERO
                .with_address(binding.channel_frequency_offsets),
        );
        self.environment.packet_start_offsets().write(
            BlePhyEnvironmentPacketStartOffsets::ZERO.with_address(binding.packet_start_offsets),
        );
        self.environment
            .rx_address_delays()
            .write(BlePhyEnvironmentRxAddressDelays::ZERO.with_address(binding.rx_address_delays));
        self.environment
            .tx_power_levels()
            .write(BlePhyEnvironmentTxPowerLevels::ZERO.with_address(binding.tx_power_levels));
        self.environment.tx_power_level_count().write(
            BlePhyEnvironmentTxPowerLevelCount::ZERO.with_value(TX_POWER_LEVELS_BYTES as u8),
        );
        self.environment.tx_power_first_level().write(
            BlePhyEnvironmentTxPowerFirstLevel::ZERO
                .with_dbm_sign_extended(TX_POWER_LEVELS[0] as i32 as u32),
        );
        self.environment.tx_power_last_level().write(
            BlePhyEnvironmentTxPowerLastLevel::ZERO
                .with_dbm_sign_extended(TX_POWER_LEVELS[TX_POWER_LEVELS_BYTES - 1] as i32 as u32),
        );
        self.resolving_list
            .word_0000_opaque()
            .write(RESOLVING_LIST_INITIAL_HEAD);
    }
}

impl Default for BlePhyEngineStorage {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests;
