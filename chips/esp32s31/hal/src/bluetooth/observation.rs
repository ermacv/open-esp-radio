//! Diagnostic value observations borrowed from each register's owner.
use super::*;
use oer_esp32s31_pac as pac;

/// Complete modeled primary enables and pending observations, granting no MMIO authority.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BluetoothInterruptObservation {
    pub enable_0: u32,
    pub enable_1: u32,
    pub status_0: u32,
    pub status_1: u32,
    pub nrt_0: u32,
    pub nrt_1: u32,
}
/// Task-bank enable and complete opaque +0x4bc word.
/// Diagnostic-only: read side effects of +0x4bc are not established.
/// No cause or CPU-output equation is implied.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BluetoothTaskInterruptObservation {
    pub enable: u32,
    pub opaque_state: u32,
}
fn irq_enable_0(v: pac::bt_mac::irq_enable_0::R) -> u32 {
    use pac::bt_mac::irq_enable_0 as r;
    (u32::from(v.source_0()) << r::SOURCE_0_MASK.trailing_zeros())
        | (u32::from(v.source_1()) << r::SOURCE_1_MASK.trailing_zeros())
        | (u32::from(v.source_8()) << r::SOURCE_8_MASK.trailing_zeros())
        | (u32::from(v.source_13()) << r::SOURCE_13_MASK.trailing_zeros())
        | (u32::from(v.source_15()) << r::SOURCE_15_MASK.trailing_zeros())
        | (u32::from(v.source_16()) << r::SOURCE_16_MASK.trailing_zeros())
        | (u32::from(v.source_21()) << r::SOURCE_21_MASK.trailing_zeros())
        | (v.sources_27_28().get() << r::SOURCES_27_28_MASK.trailing_zeros())
        | (u32::from(v.iso_cig_reset_source()) << r::ISO_CIG_RESET_SOURCE_MASK.trailing_zeros())
        | (v.bits_3_7_opaque().get() << r::BITS_3_7_OPAQUE_MASK.trailing_zeros())
        | (v.bits_9_12_opaque().get() << r::BITS_9_12_OPAQUE_MASK.trailing_zeros())
        | (u32::from(v.bit_14_opaque()) << r::BIT_14_OPAQUE_MASK.trailing_zeros())
        | (v.bits_17_20_opaque().get() << r::BITS_17_20_OPAQUE_MASK.trailing_zeros())
        | (v.bits_22_26_opaque().get() << r::BITS_22_26_OPAQUE_MASK.trailing_zeros())
        | (v.bits_29_31_opaque().get() << r::BITS_29_31_OPAQUE_MASK.trailing_zeros())
}
fn irq_enable_1(v: pac::bt_mac::irq_enable_1::R) -> u32 {
    use pac::bt_mac::irq_enable_1 as r;
    (u32::from(v.controller_time_overflow()) << r::CONTROLLER_TIME_OVERFLOW_MASK.trailing_zeros())
        | (u32::from(v.source_3()) << r::SOURCE_3_MASK.trailing_zeros())
        | (u32::from(v.source_4()) << r::SOURCE_4_MASK.trailing_zeros())
        | (u32::from(v.source_5()) << r::SOURCE_5_MASK.trailing_zeros())
        | (u32::from(v.source_6()) << r::SOURCE_6_MASK.trailing_zeros())
        | (u32::from(v.source_8()) << r::SOURCE_8_MASK.trailing_zeros())
        | (u32::from(v.source_9()) << r::SOURCE_9_MASK.trailing_zeros())
        | (u32::from(v.source_12()) << r::SOURCE_12_MASK.trailing_zeros())
        | (u32::from(v.source_13()) << r::SOURCE_13_MASK.trailing_zeros())
        | (u32::from(v.bit_0_opaque()) << r::BIT_0_OPAQUE_MASK.trailing_zeros())
        | (u32::from(v.bit_2_opaque()) << r::BIT_2_OPAQUE_MASK.trailing_zeros())
        | (u32::from(v.bit_7_opaque()) << r::BIT_7_OPAQUE_MASK.trailing_zeros())
        | (v.bits_10_11_opaque().get() << r::BITS_10_11_OPAQUE_MASK.trailing_zeros())
        | (v.bits_14_31_opaque().get() << r::BITS_14_31_OPAQUE_MASK.trailing_zeros())
}
fn interrupt_enable(v: pac::bt_mac::interrupt_enable::R) -> u32 {
    use pac::bt_mac::interrupt_enable as r;
    (u32::from(v.scheduler_run()) << r::SCHEDULER_RUN_MASK.trailing_zeros())
        | (u32::from(v.source_17()) << r::SOURCE_17_MASK.trailing_zeros())
        | (v.bits_0_13_opaque().get() << r::BITS_0_13_OPAQUE_MASK.trailing_zeros())
        | (v.bits_15_16_opaque().get() << r::BITS_15_16_OPAQUE_MASK.trailing_zeros())
        | (v.bits_18_31_opaque().get() << r::BITS_18_31_OPAQUE_MASK.trailing_zeros())
}
pub(super) fn primary<B: oer_mmio::Placement>(
    bank: &pac::bt_mac::BluetoothInterruptBank<B>,
) -> BluetoothInterruptObservation {
    // SOURCE: BT_MAC IRQ_ENABLE/STATUS and NRT snapshot fields in the register
    // model; reads have no acknowledgement side effect. This is sequential,
    // not an atomic cross-bank snapshot or a clear authority.
    BluetoothInterruptObservation {
        enable_0: irq_enable_0(bank.read_irq_enable_0()),
        enable_1: irq_enable_1(bank.read_irq_enable_1()),
        status_0: bank.sample_bluetooth_primary_interrupt_bank_0().bits(),
        status_1: bank.sample_bluetooth_primary_interrupt_bank_1().bits(),
        nrt_0: bank.read_irq_status_snapshot_0().pending_bits(),
        nrt_1: bank.read_irq_status_snapshot_1().pending_bits(),
    }
}
impl InterruptSetupOwner {
    /// Read the complete modeled enable and pending banks without acknowledgement.
    /// The returned values grant neither register access nor release authority.
    pub fn interrupt_observation(&self) -> BluetoothInterruptObservation {
        primary(&self.registers.peripherals.bluetooth_interrupt_bank)
    }
}
impl InterruptOutputPreparedOwner {
    /// Read the complete modeled enable and pending banks without acknowledgement.
    /// The returned values grant neither register access nor release authority.
    pub fn interrupt_observation(&self) -> BluetoothInterruptObservation {
        primary(&self.registers.peripherals.bluetooth_interrupt_bank)
    }
}
impl InterruptRegistersOwner {
    /// Read the complete modeled enable and pending banks without acknowledgement.
    /// The returned values grant neither register access nor release authority.
    pub fn interrupt_observation(&self) -> BluetoothInterruptObservation {
        primary(&self.registers.peripherals.bluetooth_interrupt_bank)
    }
}
impl InterruptOutputReleasedOwner {
    /// Read the complete modeled enable and pending banks without acknowledgement.
    /// The returned values grant neither register access nor release authority.
    pub fn interrupt_observation(&self) -> BluetoothInterruptObservation {
        primary(&self.registers.peripherals.bluetooth_interrupt_bank)
    }
}
impl TaskOwner {
    /// Read all modeled task enables without accessing the opaque +0x4bc word.
    pub fn interrupt_enable_image(&self) -> u32 {
        task_enable(&self.registers.bluetooth.btmac_ble_phy_init)
    }

    /// Read task enables and the complete opaque +0x4bc word.
    /// Diagnostic-only: read side effects of +0x4bc are not established.
    /// Production admission and release use only `interrupt_enable_image`.
    /// SOURCE: BLOB_S31_BLE_TASK_IRQ_INHERITANCE_20261027 and
    /// S31_MMIO_USE_201014BC / S31_MMIO_DECLARED_201014BC (use, not read safety).
    /// Unmodeled lanes and event identity stay unknown; the sourced whole-word
    /// read retains those bits without adding field semantics.
    pub fn interrupt_observation(&self) -> BluetoothTaskInterruptObservation {
        task(&self.registers.bluetooth.btmac_ble_phy_init)
    }
}

pub(super) fn task<B: oer_mmio::Placement>(
    b: &pac::bt_mac::BtmacBlePhyInit<B>,
) -> BluetoothTaskInterruptObservation {
    BluetoothTaskInterruptObservation {
        enable: task_enable(b),
        opaque_state: b.read_bluetooth_task_diagnostic_image(),
    }
}

pub(super) fn task_enable<B: oer_mmio::Placement>(b: &pac::bt_mac::BtmacBlePhyInit<B>) -> u32 {
    interrupt_enable(b.read_interrupt_enable())
}

#[cfg(test)]
mod tests;
