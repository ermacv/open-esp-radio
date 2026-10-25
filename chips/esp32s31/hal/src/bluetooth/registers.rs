//! The Bluetooth LE controller register owners and their operations.
//!
//! The PAC root hands the Bluetooth partitions over as their generated
//! parts: the controller parts with modem ETM channel two
//! ([`BluetoothControllerParts`]), the modem low-power timer with ETM
//! channels four through seven ([`BluetoothModemLpTimerParts`]) and the
//! interrupt bank with the scheduler's diagnostic pair
//! ([`BluetoothInterruptPeripherals`]). The owners here hold those parts and
//! perform each vendor operation over their field accessors; only what the
//! accessors cannot express (a whole-word store, an acknowledgement of a
//! write-one-to-clear bank) is a generated method of the part.
//!
//! Every owner is generic over where its blocks lie: production uses the
//! chip's addresses ([`Chip`]), host tests host memory.

#![deny(unsafe_code)]
#![cfg_attr(
    not(feature = "bluetooth-engine"),
    allow(
        dead_code,
        reason = "the controller-memory engine is the only caller of some operations"
    )
)]

use oer_esp32s31_pac::{
    ble_accelerator, bt_bb, bt_mac, btdm, instances,
    modem_etm::{self, ModemEtmBluetooth, ModemEtmBluetoothPhy},
    modem_pwr,
    ownership::{
        BluetoothControllerPeripherals, BluetoothInterruptPeripherals,
        BluetoothModemLpTimerPeripherals,
    },
};
use oer_mmio::{Fixed, Placement};

use crate::modem::{
    BluetoothLowPowerClockObservation, ModemSysconBluetoothObservation,
    SharedModemClockObservation, SharedRadioRegisters,
};

pub(crate) mod baseband;
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

/// Where each instance the Bluetooth owners reach lies: every part of one
/// instance lies at that instance's base.
pub trait BluetoothPlacements {
    /// `BT_MAC`.
    type BtMac: Placement;
    /// `BLE_ACCELERATOR`.
    type Accelerator: Placement;
    /// `BTDM`.
    type Btdm: Placement;
    /// `MODEM_ETM`.
    type Etm: Placement;
    /// `MODEM_PWR`.
    type ModemPwr: Placement;
    /// The controller parts no operation reaches, held for the partition.
    type Held;
}

/// The chip's addresses.
pub enum Chip {}

impl BluetoothPlacements for Chip {
    type BtMac = Fixed<{ instances::bt_mac::BASE }>;
    type Accelerator = Fixed<{ instances::ble_accelerator::BASE }>;
    type Btdm = Fixed<{ instances::btdm::BASE }>;
    type Etm = Fixed<{ instances::modem_etm::BASE }>;
    type ModemPwr = Fixed<{ instances::modem_pwr::BASE }>;
    type Held = HeldControllerBlocks;
}

/// The controller parts of the partition that no operation reaches.
pub struct HeldControllerBlocks {
    iso_scheduler_notify: bt_mac::BleIsoSchedulerNotify,
    sync_publication: bt_mac::BleSyncPublication,
    reset_idle_status: bt_mac::BluetoothResetIdleStatus0,
    cte: bt_bb::BtV32Cte,
}

/// The controller parts the task owner drives.
pub(crate) struct ControllerBlocks<P: BluetoothPlacements> {
    pub(crate) ble_controller_address_slots: bt_mac::BleControllerAddressSlots<P::BtMac>,
    pub(crate) ble_hw_accelerator: ble_accelerator::BleHwAccelerator<P::Accelerator>,
    pub(crate) ble_hw_cte_ring_control: bt_mac::BleHwCteRingControl<P::BtMac>,
    pub(crate) ble_hw_resolving_list: bt_mac::BleHwResolvingList<P::BtMac>,
    pub(crate) ble_hw_runtime_control: bt_mac::BleHwRuntimeControl<P::BtMac>,
    pub(crate) ble_hw_positional_word_1ff0: bt_mac::BleHwPositionalWord1ff0<P::BtMac>,
    pub(crate) ble_hw_positional_word_891c: modem_etm::BleHwPositionalWord891c<P::Etm>,
    pub(crate) ble_scan_backoff: bt_mac::BleScanBackoff<P::BtMac>,
    pub(crate) bluetooth_controller_core: bt_mac::BluetoothControllerCore<P::BtMac>,
    pub(crate) btdm_scheduler_table: btdm::BtdmSchedulerTable<P::Btdm>,
    pub(crate) btmac_ble_phy_init: bt_mac::BtmacBlePhyInit<P::BtMac>,
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
    pub(crate) etm: ModemEtmBluetoothPhy<P::Etm>,
}

/// The Bluetooth controller partition with the modem ETM channel its PHY
/// initialization programs.
pub struct BluetoothControllerParts {
    /// The controller partition of the PAC root.
    pub controller: BluetoothControllerPeripherals,
    /// Modem ETM channel two.
    pub phy_etm: ModemEtmBluetoothPhy,
}

impl BluetoothTaskRegisters {
    /// Assemble the task owner from the partition. This performs no MMIO.
    pub fn new(parts: BluetoothControllerParts) -> Self {
        let BluetoothControllerParts {
            controller:
                BluetoothControllerPeripherals {
                    ble_controller_address_slots,
                    ble_hw_accelerator,
                    ble_hw_cte_ring_control,
                    ble_hw_resolving_list,
                    ble_hw_runtime_control,
                    ble_iso_scheduler_notify,
                    ble_hw_positional_word1ff0,
                    ble_hw_positional_word891c,
                    ble_scan_backoff,
                    ble_sync_publication,
                    bluetooth_controller_core,
                    bluetooth_reset_idle_status0,
                    btdm_scheduler_table,
                    btmac_ble_phy_init,
                    bt_v32_cte,
                },
            phy_etm,
        } = parts;
        Self {
            bluetooth: ControllerBlocks {
                ble_controller_address_slots,
                ble_hw_accelerator,
                ble_hw_cte_ring_control,
                ble_hw_resolving_list,
                ble_hw_runtime_control,
                ble_hw_positional_word_1ff0: ble_hw_positional_word1ff0,
                ble_hw_positional_word_891c: ble_hw_positional_word891c,
                ble_scan_backoff,
                bluetooth_controller_core,
                btdm_scheduler_table,
                btmac_ble_phy_init,
                held: HeldControllerBlocks {
                    iso_scheduler_notify: ble_iso_scheduler_notify,
                    sync_publication: ble_sync_publication,
                    reset_idle_status: bluetooth_reset_idle_status0,
                    cte: bt_v32_cte,
                },
            },
            etm: phy_etm,
        }
    }

    /// Return the partition. This performs no MMIO.
    pub fn into_parts(self) -> BluetoothControllerParts {
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
        BluetoothControllerParts {
            controller: BluetoothControllerPeripherals {
                ble_controller_address_slots,
                ble_hw_accelerator,
                ble_hw_cte_ring_control,
                ble_hw_resolving_list,
                ble_hw_runtime_control,
                ble_iso_scheduler_notify: iso_scheduler_notify,
                ble_hw_positional_word1ff0: ble_hw_positional_word_1ff0,
                ble_hw_positional_word891c: ble_hw_positional_word_891c,
                ble_scan_backoff,
                ble_sync_publication: sync_publication,
                bluetooth_controller_core,
                bluetooth_reset_idle_status0: reset_idle_status,
                btdm_scheduler_table,
                btmac_ble_phy_init,
                bt_v32_cte: cte,
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
    pub(crate) bluetooth_interrupt_bank: bt_mac::BluetoothInterruptBank<P::BtMac>,
    pub(crate) bluetooth_scheduler_interrupt_runtime:
        bt_mac::BluetoothSchedulerInterruptRuntime<P::BtMac>,
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
    pub(crate) runtime_control: modem_pwr::BtdmRuntimeControl<P::ModemPwr>,
    /// Modem ETM channels four through seven, programmed by the low-power
    /// component.
    pub(crate) etm: ModemEtmBluetooth<P::Etm>,
}

/// The modem low-power timer partition with the modem ETM channels its
/// low-power component programs.
pub struct BluetoothModemLpTimerParts {
    /// The timer partition of the PAC root.
    pub timer: BluetoothModemLpTimerPeripherals,
    /// Modem ETM channels four through seven.
    pub etm: ModemEtmBluetooth,
}

impl BluetoothModemLpTimerRegisters {
    /// Take the timer partition. This performs no MMIO.
    pub fn new(parts: BluetoothModemLpTimerParts) -> Self {
        let BluetoothModemLpTimerParts {
            timer:
                BluetoothModemLpTimerPeripherals {
                    btdm_runtime_control,
                },
            etm,
        } = parts;
        Self {
            runtime_control: btdm_runtime_control,
            etm,
        }
    }

    /// Return the timer partition. This performs no MMIO.
    pub fn into_parts(self) -> BluetoothModemLpTimerParts {
        BluetoothModemLpTimerParts {
            timer: BluetoothModemLpTimerPeripherals {
                btdm_runtime_control: self.runtime_control,
            },
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
mod tests;
