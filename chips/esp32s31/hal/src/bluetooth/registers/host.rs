//! The Bluetooth owners over host memory, for the tests of their operations.
//!
//! A test fills each block's memory, runs owner operations and compares the
//! memory before and after, or reads it back through the generated parts'
//! typed readers, so no test names a register address, mask or shift.

use std::{vec, vec::Vec};

use oer_esp32s31_pac::{
    ble_controller_address_slots::{self, BleControllerAddressSlots},
    ble_hw_accelerator::{self, BleHwAccelerator},
    ble_hw_cte_ring_control::{self, BleHwCteRingControl},
    ble_hw_positional_word_1ff0::{self, BleHwPositionalWord1ff0},
    ble_hw_positional_word_891c::{self, BleHwPositionalWord891c},
    ble_hw_resolving_list::{self, BleHwResolvingList},
    ble_hw_runtime_control::{self, BleHwRuntimeControl},
    ble_scan_backoff::{self, BleScanBackoff},
    bluetooth_controller_core::{self, BluetoothControllerCore},
    bluetooth_interrupt_bank::{self, BluetoothInterruptBank},
    bluetooth_scheduler_interrupt_runtime::{self, BluetoothSchedulerInterruptRuntime},
    btdm_runtime_control::{self, BtdmRuntimeControl},
    btdm_scheduler_table::{self, BtdmSchedulerTable},
    btmac_ble_phy_init::{self, BtmacBlePhyInit},
    modem_etm::{self, ModemEtm},
};
use oer_mmio::Host;

use super::{
    BluetoothInterruptRegisters, BluetoothModemLpTimerRegisters, BluetoothPlacements,
    BluetoothTaskRegisters, ControllerBlocks, InterruptBlocks,
};

/// Every block in host memory.
pub(crate) struct HostPlacements<'a>(core::marker::PhantomData<&'a ()>);

impl<'a> BluetoothPlacements for HostPlacements<'a> {
    type AddressSlots = Host<'a>;
    type Accelerator = Host<'a>;
    type CteRing = Host<'a>;
    type ResolvingList = Host<'a>;
    type RuntimeControl = Host<'a>;
    type Word1ff0 = Host<'a>;
    type Word891c = Host<'a>;
    type ScanBackoff = Host<'a>;
    type Core = Host<'a>;
    type SchedulerTable = Host<'a>;
    type PhyInit = Host<'a>;
    type Etm = Host<'a>;
    type Timer = Host<'a>;
    type InterruptBank = Host<'a>;
    type SchedulerRuntime = Host<'a>;
    type Held = ();
}

/// The memory of every block the Bluetooth owners reach.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct HostMemory {
    pub(crate) address_slots: Vec<u32>,
    pub(crate) accelerator: Vec<u32>,
    pub(crate) cte_ring: Vec<u32>,
    pub(crate) resolving_list: Vec<u32>,
    pub(crate) runtime_control: Vec<u32>,
    pub(crate) word_1ff0: Vec<u32>,
    pub(crate) word_891c: Vec<u32>,
    pub(crate) scan_backoff: Vec<u32>,
    pub(crate) core: Vec<u32>,
    pub(crate) scheduler_table: Vec<u32>,
    pub(crate) phy_init: Vec<u32>,
    pub(crate) etm: Vec<u32>,
    pub(crate) timer: Vec<u32>,
    pub(crate) interrupt_bank: Vec<u32>,
    pub(crate) scheduler_runtime: Vec<u32>,
}

fn words(size: usize, fill: u32) -> Vec<u32> {
    vec![fill; size.div_ceil(4)]
}

fn host<'a, T>(
    memory: &'a mut [u32],
    make: impl FnOnce(&'a mut [u32]) -> Result<T, oer_mmio::HostTooSmall>,
) -> T {
    make(memory).expect("memory holds the block")
}

impl HostMemory {
    /// Every word of every block `fill`.
    pub(crate) fn filled(fill: u32) -> Self {
        Self {
            address_slots: words(ble_controller_address_slots::SIZE, fill),
            accelerator: words(ble_hw_accelerator::SIZE, fill),
            cte_ring: words(ble_hw_cte_ring_control::SIZE, fill),
            resolving_list: words(ble_hw_resolving_list::SIZE, fill),
            runtime_control: words(ble_hw_runtime_control::SIZE, fill),
            word_1ff0: words(ble_hw_positional_word_1ff0::SIZE, fill),
            word_891c: words(ble_hw_positional_word_891c::SIZE, fill),
            scan_backoff: words(ble_scan_backoff::SIZE, fill),
            core: words(bluetooth_controller_core::SIZE, fill),
            scheduler_table: words(btdm_scheduler_table::SIZE, fill),
            phy_init: words(btmac_ble_phy_init::SIZE, fill),
            etm: words(modem_etm::SIZE, fill),
            timer: words(btdm_runtime_control::SIZE, fill),
            interrupt_bank: words(bluetooth_interrupt_bank::SIZE, fill),
            scheduler_runtime: words(bluetooth_scheduler_interrupt_runtime::SIZE, fill),
        }
    }

    /// The task, interrupt and timer owners over this memory.
    pub(crate) fn owners(
        &mut self,
    ) -> (
        BluetoothTaskRegisters<HostPlacements<'_>>,
        BluetoothInterruptRegisters<HostPlacements<'_>>,
        BluetoothModemLpTimerRegisters<HostPlacements<'_>>,
    ) {
        let (_, phy_etm, timer_etm) = host(&mut self.etm, ModemEtm::host).divide();
        let task = BluetoothTaskRegisters {
            bluetooth: ControllerBlocks {
                ble_controller_address_slots: host(
                    &mut self.address_slots,
                    BleControllerAddressSlots::host,
                ),
                ble_hw_accelerator: host(&mut self.accelerator, BleHwAccelerator::host),
                ble_hw_cte_ring_control: host(&mut self.cte_ring, BleHwCteRingControl::host),
                ble_hw_resolving_list: host(&mut self.resolving_list, BleHwResolvingList::host),
                ble_hw_runtime_control: host(&mut self.runtime_control, BleHwRuntimeControl::host),
                ble_hw_positional_word_1ff0: host(
                    &mut self.word_1ff0,
                    BleHwPositionalWord1ff0::host,
                ),
                ble_hw_positional_word_891c: host(
                    &mut self.word_891c,
                    BleHwPositionalWord891c::host,
                ),
                ble_scan_backoff: host(&mut self.scan_backoff, BleScanBackoff::host),
                bluetooth_controller_core: host(&mut self.core, BluetoothControllerCore::host),
                btdm_scheduler_table: host(&mut self.scheduler_table, BtdmSchedulerTable::host),
                btmac_ble_phy_init: host(&mut self.phy_init, BtmacBlePhyInit::host),
                held: (),
            },
            etm: phy_etm,
        };
        let interrupts = BluetoothInterruptRegisters {
            peripherals: InterruptBlocks {
                bluetooth_interrupt_bank: host(
                    &mut self.interrupt_bank,
                    BluetoothInterruptBank::host,
                ),
                bluetooth_scheduler_interrupt_runtime: host(
                    &mut self.scheduler_runtime,
                    BluetoothSchedulerInterruptRuntime::host,
                ),
            },
        };
        let timer = BluetoothModemLpTimerRegisters {
            runtime_control: host(&mut self.timer, BtdmRuntimeControl::host),
            etm: timer_etm,
        };
        (task, interrupts, timer)
    }
}

/// Each word that differs between two images of a block, as its word index
/// and new value.
pub(crate) fn changed(before: &[u32], after: &[u32]) -> Vec<(usize, u32)> {
    before
        .iter()
        .zip(after)
        .enumerate()
        .filter(|(_, (before, after))| before != after)
        .map(|(index, (_, after))| (index, *after))
        .collect()
}
