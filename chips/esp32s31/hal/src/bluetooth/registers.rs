//! The Bluetooth LE controller register owners and their operations.
//!
//! The PAC hands the Bluetooth partition over as its generated parts: the
//! controller blocks with modem ETM channel two
//! ([`BluetoothControllerPartition`]), the modem low-power timer with ETM
//! channels four through seven ([`BluetoothModemLpTimerPartition`]) and the
//! interrupt bank with the scheduler's diagnostic pair
//! ([`BluetoothInterruptPeripherals`]). The owners here hold those parts and
//! perform each vendor operation over their field accessors; a word the
//! accessors cannot write as the vendor does (a whole-word store, a store
//! from zero of part of a word, an acknowledgement of a write-one-to-clear
//! bank) stays a named transaction of the PAC policy.
//!
//! Every owner is generic over where its blocks lie: production uses the
//! chip's addresses ([`Chip`]), host tests host memory.

#![deny(unsafe_code)]

use oer_esp32s31_pac::{
    BluetoothControllerPartition, BluetoothControllerPeripherals, BluetoothInterruptPeripherals,
    BluetoothLowPowerClockObservation, BluetoothModemLpTimerPartition,
    ModemSysconBluetoothObservation, SharedModemClockObservation, SharedRadioRegisters,
    ble_controller_address_slots, ble_hw_accelerator, ble_hw_cte_ring_control,
    ble_hw_positional_word_1ff0, ble_hw_positional_word_891c, ble_hw_resolving_list,
    ble_hw_runtime_control, ble_iso_scheduler_notify, ble_scan_backoff, ble_sync_publication,
    bluetooth_controller_core, bluetooth_interrupt_bank, bluetooth_reset_idle_status_0,
    bluetooth_scheduler_interrupt_runtime, bt_v3_2_cte, btdm_runtime_control, btdm_scheduler_table,
    btmac_ble_phy_init, modem_etm,
};
use oer_mmio::{Fixed, Placement};

mod connection_control;
pub(crate) mod controller;
pub(crate) mod device_table;
pub(crate) mod direction_finding;
pub(crate) mod interrupt;
pub(crate) mod memory_lists;
pub(crate) mod modem_timer;
pub(crate) mod phy;
pub(crate) mod scan;
pub(crate) mod scheduler;

#[cfg(test)]
pub(crate) mod host;

pub use controller::{
    init::{
        BluetoothControllerHalInitConfig, BluetoothControllerTimeScale, BluetoothHalInitPeriod,
        BluetoothHalInitScale, BluetoothMicrosecondDeltaProjection,
        BluetoothRawTickDeltaProjection,
    },
    time::BluetoothControllerLatchedTime,
};
pub use device_table::BluetoothDeviceTablePublished;
pub use direction_finding::BluetoothDirectionFindingDisabledBaselinePrepared;
pub use interrupt::{
    BluetoothInterruptOutputPrepared, BluetoothNrtInterruptAcknowledged,
    BluetoothPrimaryFaultSources, BluetoothPrimaryInterruptEpoch,
    BluetoothSchedulerRunInterruptsPrepared, retirement::BluetoothControllerOutputReleaseError,
};
pub use memory_lists::{
    BluetoothMemoryListPointerImage, BluetoothMemoryListSelector, BluetoothMemoryListSlot,
};
pub use modem_timer::{
    BluetoothLowPowerRuntimeControlObservation, BluetoothModemLpTimerCompareDisposition,
    BluetoothModemLpTimerCounterObservation, BluetoothModemLpTimerEpoch,
    BluetoothModemLpTimerHandlerRegisterObservation, BluetoothModemLpTimerInstant,
    BluetoothModemLpTimerInterruptObservation,
};
pub use phy::{BlePhyEtmRouteDisabled, BluetoothPhyRegisterInitInputs};
pub use scan::BluetoothScanStartPublished;
pub use scheduler::{
    BluetoothSchedulerHardwareListHead, BluetoothSchedulerHardwareListHeadEmptyObserved,
    BluetoothSchedulerHardwareListHeadPublished,
    BluetoothSchedulerHardwareListHeadRetirementObservation,
    BluetoothSchedulerHardwareListsCleared, BluetoothSchedulerHardwareRunCommandPublished,
    BluetoothSchedulerInsertionCommand, BluetoothSchedulerInsertionCommandStartCleared,
    BluetoothSchedulerRunEventPublished,
    cancellation::{
        BluetoothSchedulerCancellationDisposition, BluetoothSchedulerCancellationIndexed,
        BluetoothSchedulerCancellationReleased, BluetoothSchedulerCancellationRequested,
        BluetoothSchedulerCancellationSourceAcknowledged, BluetoothSchedulerSkipCleared,
        BluetoothSchedulerSkipDisposition, BluetoothSchedulerSkipPublished,
        BluetoothSchedulerSkipRequest, BluetoothSchedulerSkipResult,
    },
    insertion::{
        BluetoothSchedulerExecutionLockDisposition, BluetoothSchedulerExecutionLockPublished,
        BluetoothSchedulerExecutionLockRequest, BluetoothSchedulerExecutionModifyDisposition,
        BluetoothSchedulerExecutionModifyPublished,
    },
    lock_modify::{
        BluetoothSchedulerLockModifyInterruptObservation, BluetoothSchedulerLockModifyObservation,
        BluetoothSchedulerLockModifyPublished, BluetoothSchedulerLockModifyRequest,
        BluetoothSchedulerLockModifyTaskObservation,
    },
    runtime::{
        BluetoothSchedulerBusyObservation, BluetoothSchedulerFinishedHardwareListObserved,
        BluetoothSchedulerFinishedListObservation, BluetoothSchedulerFinishedListPop,
        BluetoothSchedulerHardwareListIndex, BluetoothSchedulerReferenceCleared,
        BluetoothSchedulerReferenceGateObservation, BluetoothSchedulerSoftwareListRemovalIdle,
        BluetoothSchedulerSoftwareListRemovalInterruptStep,
        BluetoothSchedulerSoftwareListRemovalJoin, BluetoothSchedulerWorkObservation,
    },
    stop::{BluetoothSchedulerStopped, BluetoothSchedulerStoppedHeadRetirement},
};

/// Where each block the Bluetooth owners drive lies.
pub trait BluetoothPlacements {
    /// `BLE_CONTROLLER_ADDRESS_SLOTS`.
    type AddressSlots: Placement;
    /// `BLE_HW_ACCELERATOR`.
    type Accelerator: Placement;
    /// `BLE_HW_CTE_RING_CONTROL`.
    type CteRing: Placement;
    /// `BLE_HW_RESOLVING_LIST`.
    type ResolvingList: Placement;
    /// `BLE_HW_RUNTIME_CONTROL`.
    type RuntimeControl: Placement;
    /// `BLE_HW_POSITIONAL_WORD_1FF0`.
    type Word1ff0: Placement;
    /// `BLE_HW_POSITIONAL_WORD_891C`.
    type Word891c: Placement;
    /// `BLE_SCAN_BACKOFF`.
    type ScanBackoff: Placement;
    /// `BLUETOOTH_CONTROLLER_CORE`.
    type Core: Placement;
    /// `BTDM_SCHEDULER_TABLE`.
    type SchedulerTable: Placement;
    /// `BTMAC_BLE_PHY_INIT`.
    type PhyInit: Placement;
    /// `MODEM_ETM`.
    type Etm: Placement;
    /// `BTDM_RUNTIME_CONTROL`.
    type Timer: Placement;
    /// `BLUETOOTH_INTERRUPT_BANK`.
    type InterruptBank: Placement;
    /// `BLUETOOTH_SCHEDULER_INTERRUPT_RUNTIME`.
    type SchedulerRuntime: Placement;
    /// The controller blocks no operation reaches, held for the partition.
    type Held;
}

/// The chip's addresses.
pub enum Chip {}

impl BluetoothPlacements for Chip {
    type AddressSlots = Fixed<{ ble_controller_address_slots::BASE }>;
    type Accelerator = Fixed<{ ble_hw_accelerator::BASE }>;
    type CteRing = Fixed<{ ble_hw_cte_ring_control::BASE }>;
    type ResolvingList = Fixed<{ ble_hw_resolving_list::BASE }>;
    type RuntimeControl = Fixed<{ ble_hw_runtime_control::BASE }>;
    type Word1ff0 = Fixed<{ ble_hw_positional_word_1ff0::BASE }>;
    type Word891c = Fixed<{ ble_hw_positional_word_891c::BASE }>;
    type ScanBackoff = Fixed<{ ble_scan_backoff::BASE }>;
    type Core = Fixed<{ bluetooth_controller_core::BASE }>;
    type SchedulerTable = Fixed<{ btdm_scheduler_table::BASE }>;
    type PhyInit = Fixed<{ btmac_ble_phy_init::BASE }>;
    type Etm = Fixed<{ modem_etm::BASE }>;
    type Timer = Fixed<{ btdm_runtime_control::BASE }>;
    type InterruptBank = Fixed<{ bluetooth_interrupt_bank::BASE }>;
    type SchedulerRuntime = Fixed<{ bluetooth_scheduler_interrupt_runtime::BASE }>;
    type Held = HeldControllerBlocks;
}

/// The controller blocks of the partition that no operation reaches.
pub struct HeldControllerBlocks {
    iso_scheduler_notify: ble_iso_scheduler_notify::BleIsoSchedulerNotify,
    sync_publication: ble_sync_publication::BleSyncPublication,
    reset_idle_status: bluetooth_reset_idle_status_0::BluetoothResetIdleStatus0,
    cte: bt_v3_2_cte::BtV3_2Cte,
}

/// The controller blocks the task owner drives.
pub(crate) struct ControllerBlocks<P: BluetoothPlacements> {
    pub(crate) ble_controller_address_slots:
        ble_controller_address_slots::BleControllerAddressSlots<P::AddressSlots>,
    pub(crate) ble_hw_accelerator: ble_hw_accelerator::BleHwAccelerator<P::Accelerator>,
    pub(crate) ble_hw_cte_ring_control: ble_hw_cte_ring_control::BleHwCteRingControl<P::CteRing>,
    pub(crate) ble_hw_resolving_list: ble_hw_resolving_list::BleHwResolvingList<P::ResolvingList>,
    pub(crate) ble_hw_runtime_control:
        ble_hw_runtime_control::BleHwRuntimeControl<P::RuntimeControl>,
    pub(crate) ble_hw_positional_word_1ff0:
        ble_hw_positional_word_1ff0::BleHwPositionalWord1ff0<P::Word1ff0>,
    pub(crate) ble_hw_positional_word_891c:
        ble_hw_positional_word_891c::BleHwPositionalWord891c<P::Word891c>,
    pub(crate) ble_scan_backoff: ble_scan_backoff::BleScanBackoff<P::ScanBackoff>,
    pub(crate) bluetooth_controller_core:
        bluetooth_controller_core::BluetoothControllerCore<P::Core>,
    pub(crate) btdm_scheduler_table: btdm_scheduler_table::BtdmSchedulerTable<P::SchedulerTable>,
    pub(crate) btmac_ble_phy_init: btmac_ble_phy_init::BtmacBlePhyInit<P::PhyInit>,
    held: P::Held,
}

/// Ordinary task-side owner of the Bluetooth controller partition.
///
/// It contains no shared radio partition: a transaction that also touches the
/// radio PHY, coexistence or shared baseband borrows the shared radio owner
/// explicitly. Possessing it does not itself prove that common PHY, BTBB or
/// controller lifecycle prerequisites have run.
#[must_use = "the Bluetooth task owner must be reunited before release"]
pub struct BluetoothTaskRegisters<P: BluetoothPlacements = Chip> {
    pub(crate) bluetooth: ControllerBlocks<P>,
    /// Modem ETM channel two, which BLE PHY initialization programs.
    ///
    /// The pinned BLE PHY initialization routes modem event 8 to task 20 on
    /// a channel it reserves. The vendor library uses channel zero, which
    /// IEEE 802.15.4 owns here; the same route runs on this channel, and
    /// Direct Test Mode disables and restores it as the vendor does its
    /// channel zero.
    pub(crate) etm: modem_etm::ModemEtmBluetoothPhy<P::Etm>,
}

impl BluetoothTaskRegisters {
    /// Assemble the task owner from the partition. This performs no MMIO.
    pub fn new(partition: BluetoothControllerPartition) -> Self {
        let BluetoothControllerPartition {
            controller:
                BluetoothControllerPeripherals {
                    ble_controller_address_slots,
                    ble_hw_accelerator,
                    ble_hw_cte_ring_control,
                    ble_hw_resolving_list,
                    ble_hw_runtime_control,
                    ble_iso_scheduler_notify,
                    ble_hw_positional_word_1ff0,
                    ble_hw_positional_word_891c,
                    ble_scan_backoff,
                    ble_sync_publication,
                    bluetooth_controller_core,
                    bluetooth_reset_idle_status_0,
                    btdm_scheduler_table,
                    btmac_ble_phy_init,
                    bt_v3_2_cte,
                },
            phy_etm,
        } = partition;
        Self {
            bluetooth: ControllerBlocks {
                ble_controller_address_slots,
                ble_hw_accelerator,
                ble_hw_cte_ring_control,
                ble_hw_resolving_list,
                ble_hw_runtime_control,
                ble_hw_positional_word_1ff0,
                ble_hw_positional_word_891c,
                ble_scan_backoff,
                bluetooth_controller_core,
                btdm_scheduler_table,
                btmac_ble_phy_init,
                held: HeldControllerBlocks {
                    iso_scheduler_notify: ble_iso_scheduler_notify,
                    sync_publication: ble_sync_publication,
                    reset_idle_status: bluetooth_reset_idle_status_0,
                    cte: bt_v3_2_cte,
                },
            },
            etm: phy_etm,
        }
    }

    /// Return the partition. This performs no MMIO.
    pub fn into_partition(self) -> BluetoothControllerPartition {
        let ControllerBlocks {
            ble_controller_address_slots,
            ble_hw_accelerator,
            ble_hw_cte_ring_control,
            ble_hw_resolving_list,
            ble_hw_runtime_control,
            ble_hw_positional_word_1ff0,
            ble_hw_positional_word_891c,
            ble_scan_backoff,
            bluetooth_controller_core,
            btdm_scheduler_table,
            btmac_ble_phy_init,
            held:
                HeldControllerBlocks {
                    iso_scheduler_notify,
                    sync_publication,
                    reset_idle_status,
                    cte,
                },
        } = self.bluetooth;
        BluetoothControllerPartition {
            controller: BluetoothControllerPeripherals {
                ble_controller_address_slots,
                ble_hw_accelerator,
                ble_hw_cte_ring_control,
                ble_hw_resolving_list,
                ble_hw_runtime_control,
                ble_iso_scheduler_notify: iso_scheduler_notify,
                ble_hw_positional_word_1ff0,
                ble_hw_positional_word_891c,
                ble_scan_backoff,
                ble_sync_publication: sync_publication,
                bluetooth_controller_core,
                bluetooth_reset_idle_status_0: reset_idle_status,
                btdm_scheduler_table,
                btmac_ble_phy_init,
                bt_v3_2_cte: cte,
            },
            phy_etm: self.etm,
        }
    }
}

impl<P: BluetoothPlacements> BluetoothTaskRegisters<P> {
    /// Read back the shared modem clocks and the Bluetooth low-power clock.
    #[doc(hidden)]
    pub fn bluetooth_shared_clock_observation(
        &self,
        shared: &SharedRadioRegisters,
    ) -> (
        SharedModemClockObservation,
        BluetoothLowPowerClockObservation,
    ) {
        (
            shared.radio_phy().shared_modem_clock_observation(),
            shared.radio_phy().bluetooth_low_power_clock_observation(),
        )
    }

    /// Reset the Bluetooth controller domains and fence the reset edge.
    #[doc(hidden)]
    pub fn reset_controller_domains(&mut self, shared: &mut SharedRadioRegisters) {
        shared.radio_phy_mut().reset_bluetooth_controller_domains();
        device_fence();
    }

    /// Whether the Bluetooth controller domain resets read back released.
    pub fn controller_resets_released(&self, shared: &SharedRadioRegisters) -> bool {
        self.controller_clock_observation(shared)
            .controller_resets_released
    }

    /// Read back the Bluetooth controller and APB clock sets and resets.
    pub fn controller_clock_observation(
        &self,
        shared: &SharedRadioRegisters,
    ) -> ModemSysconBluetoothObservation {
        shared.radio_phy().bluetooth_clock_observation()
    }
}

/// The interrupt bank and the scheduler's diagnostic pair.
pub(crate) struct InterruptBlocks<P: BluetoothPlacements> {
    pub(crate) bluetooth_interrupt_bank:
        bluetooth_interrupt_bank::BluetoothInterruptBank<P::InterruptBank>,
    pub(crate) bluetooth_scheduler_interrupt_runtime:
        bluetooth_scheduler_interrupt_runtime::BluetoothSchedulerInterruptRuntime<
            P::SchedulerRuntime,
        >,
}

impl InterruptBlocks<Chip> {
    fn new(peripherals: BluetoothInterruptPeripherals) -> Self {
        let BluetoothInterruptPeripherals {
            bluetooth_interrupt_bank,
            bluetooth_scheduler_interrupt_runtime,
        } = peripherals;
        Self {
            bluetooth_interrupt_bank,
            bluetooth_scheduler_interrupt_runtime,
        }
    }

    fn into_peripherals(self) -> BluetoothInterruptPeripherals {
        BluetoothInterruptPeripherals {
            bluetooth_interrupt_bank: self.bluetooth_interrupt_bank,
            bluetooth_scheduler_interrupt_runtime: self.bluetooth_scheduler_interrupt_runtime,
        }
    }
}

/// Inactive owner of the Bluetooth interrupt partition.
#[must_use = "the Bluetooth interrupt setup must remain paired with its task owner"]
pub struct BluetoothInterruptSetup<P: BluetoothPlacements = Chip> {
    pub(crate) peripherals: InterruptBlocks<P>,
}

impl BluetoothInterruptSetup {
    /// Take the interrupt partition. This performs no MMIO.
    pub fn new(peripherals: BluetoothInterruptPeripherals) -> Self {
        Self {
            peripherals: InterruptBlocks::new(peripherals),
        }
    }

    /// Return the interrupt partition. This performs no MMIO.
    pub fn into_partition(self) -> BluetoothInterruptPeripherals {
        self.peripherals.into_peripherals()
    }
}

/// Bluetooth interrupt-bank capability staged for a powered ISR epoch.
///
/// [`BluetoothInterruptOutputPrepared::stage_for_cpu_routes`] constructs this
/// value only after the baseline masks and controller output have been
/// prepared. The platform retains it in stable storage shared by the primary
/// and NRT handlers before enabling either CPU route: sources 124
/// (`MODEM_BT_MAC`) and 133 (`MODEM_BT_MAC_INT1`) sample and acknowledge the
/// same bank, so this one owner serves both.
#[must_use = "the Bluetooth interrupt owner must be deactivated and reunited"]
pub struct BluetoothInterruptRegisters<P: BluetoothPlacements = Chip> {
    pub(crate) peripherals: InterruptBlocks<P>,
}

/// Disjoint register owner for the Bluetooth modem low-power timer.
///
/// It holds only `BTDM_RUNTIME_CONTROL` and the modem ETM channels four
/// through seven, so source 127 keeps its hardware authority while the task
/// owner keeps the scheduler, baseband and Link-Layer registers.
#[must_use = "the modem LP-timer owner must remain paired with the Bluetooth lifecycle"]
pub struct BluetoothModemLpTimerRegisters<P: BluetoothPlacements = Chip> {
    pub(crate) runtime_control: btdm_runtime_control::BtdmRuntimeControl<P::Timer>,
    /// Modem ETM channels four through seven, programmed by the low-power
    /// component.
    pub(crate) etm: modem_etm::ModemEtmBluetooth<P::Etm>,
}

impl BluetoothModemLpTimerRegisters {
    /// Take the timer partition. This performs no MMIO.
    pub fn new(partition: BluetoothModemLpTimerPartition) -> Self {
        let BluetoothModemLpTimerPartition {
            runtime_control,
            etm,
        } = partition;
        Self {
            runtime_control,
            etm,
        }
    }

    /// Return the timer partition. This performs no MMIO.
    pub fn into_partition(self) -> BluetoothModemLpTimerPartition {
        BluetoothModemLpTimerPartition {
            runtime_control: self.runtime_control,
            etm: self.etm,
        }
    }
}

/// Order SRAM descriptor writes and MMIO at a hardware ownership boundary.
pub(crate) fn device_fence() {
    oer_mmio::fence();
}

/// A field of at most eight bits read back, as a byte.
pub(crate) const fn field_byte<const WIDTH: u32>(bits: oer_mmio::Bits<WIDTH>) -> u8 {
    const { assert!(WIDTH <= 8, "the field fits a byte") };
    bits.get().to_le_bytes()[0]
}

#[cfg(test)]
mod equivalence_tests;
