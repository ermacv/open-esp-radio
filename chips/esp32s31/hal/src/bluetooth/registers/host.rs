//! The Bluetooth owners over host memory, for the tests of their operations.
//!
//! Each part gets the memory of its whole instance, from the instance's
//! base, so a test fills that memory, runs owner operations and compares the
//! memory before and after, or reads it back through the generated parts'
//! typed readers; no test names a register mask or shift.

use std::{vec, vec::Vec};

use oer_esp32s31_pac::{
    ble_accelerator::BleHwAccelerator,
    bt_mac::{
        BleControllerAddressSlots, BleHwCteRingControl, BleHwPositionalWord1ff0,
        BleHwResolvingList, BleHwRuntimeControl, BleScanBackoff, BluetoothControllerCore,
        BluetoothInterruptBank, BluetoothSchedulerInterruptRuntime, BtmacBlePhyInit,
    },
    btdm::BtdmSchedulerTable,
    instances,
    modem_etm::{self, BleHwPositionalWord891c},
    modem_pwr::{BtdmLpTimerStatus, BtdmRuntimeControl},
};
use oer_mmio::Host;

use super::{
    BluetoothInterruptRegisters, BluetoothModemLpTimerRegisters, BluetoothPlacements,
    BluetoothTaskRegisters, ControllerBlocks, InterruptBlocks,
};

/// Every instance in host memory.
pub(crate) struct HostPlacements<'a>(core::marker::PhantomData<&'a ()>);

impl<'a> BluetoothPlacements for HostPlacements<'a> {
    type BtMac = Host<'a>;
    type Accelerator = Host<'a>;
    type Btdm = Host<'a>;
    type Etm = Host<'a>;
    type ModemPwr = Host<'a>;
    type Held = ();
}

/// The memory of every part the Bluetooth owners reach, each the whole
/// window of its instance.
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
    pub(crate) timer_status: Vec<u32>,
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
    make(memory).expect("memory holds the instance")
}

impl HostMemory {
    /// Every word of every instance `fill`.
    pub(crate) fn filled(fill: u32) -> Self {
        let bt_mac = || words(instances::bt_mac::SIZE, fill);
        Self {
            address_slots: bt_mac(),
            accelerator: words(instances::ble_accelerator::SIZE, fill),
            cte_ring: bt_mac(),
            resolving_list: bt_mac(),
            runtime_control: bt_mac(),
            word_1ff0: bt_mac(),
            word_891c: words(instances::modem_etm::SIZE, fill),
            scan_backoff: bt_mac(),
            core: bt_mac(),
            scheduler_table: words(instances::btdm::SIZE, fill),
            phy_init: bt_mac(),
            etm: words(instances::modem_etm::SIZE, fill),
            timer: words(instances::modem_pwr::SIZE, fill),
            timer_status: words(instances::modem_pwr::SIZE, fill),
            interrupt_bank: bt_mac(),
            scheduler_runtime: bt_mac(),
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
        let (_, phy_etm, timer_etm) = host(&mut self.etm, modem_etm::modem_etm_host);
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
            status: host(&mut self.timer_status, BtdmLpTimerStatus::host),
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
