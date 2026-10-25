//! The Bluetooth interrupt bank's transactions: output preparation and
//! release, the scheduler-run sources and the source-124 and source-133
//! samples and acknowledgements.

#![deny(unsafe_code)]

pub(crate) mod retirement;

use crate::bluetooth::registers::{
    BluetoothInterruptRegisters, BluetoothInterruptSetup, BluetoothPlacements, Chip,
    InterruptBlocks, device_fence,
};
use oer_esp32s31_pac::bt_mac as bank;

trait BluetoothSchedulerRunInterruptControl {
    fn clear_scheduler_run_bank_0(&mut self);
    fn clear_scheduler_run_bank_1(&mut self);
    fn enable_scheduler_run_bank_0(&mut self);
    fn enable_scheduler_run_bank_1(&mut self);
}

impl<B: oer_mmio::Placement> BluetoothSchedulerRunInterruptControl
    for HardwareInterruptControl<'_, B>
{
    fn clear_scheduler_run_bank_0(&mut self) {
        self.bank.clear_bluetooth_scheduler_run_interrupts_bank_0();
    }

    fn clear_scheduler_run_bank_1(&mut self) {
        self.bank.clear_bluetooth_scheduler_run_interrupts_bank_1();
    }

    fn enable_scheduler_run_bank_0(&mut self) {
        self.bank.modify_irq_enable_0(|w| {
            w.set_source_21(true);
            w.set_sources_27_28(oer_mmio::Bits::truncate(w.sources_27_28().get() | 0x3));
        });
    }

    fn enable_scheduler_run_bank_1(&mut self) {
        self.bank.modify_irq_enable_1(|w| {
            w.set_source_3(true);
        });
    }
}

fn execute_scheduler_run_interrupt_prepare(
    control: &mut impl BluetoothSchedulerRunInterruptControl,
) {
    control.clear_scheduler_run_bank_0();
    control.clear_scheduler_run_bank_1();
    control.enable_scheduler_run_bank_0();
    control.enable_scheduler_run_bank_1();
}

/// Affine evidence that stale scheduler-run sources were acknowledged, both
/// dynamic enable groups were published and the trailing device fence
/// completed.
#[derive(Debug, Eq, PartialEq)]
#[must_use = "prepared scheduler-run interrupts must feed broker and hardware command publication"]
pub struct BluetoothSchedulerRunInterruptsPrepared {
    _private: (),
}

trait BluetoothInterruptControl {
    fn clear_primary_baseline_bank_0(&mut self);
    fn clear_primary_baseline_bank_1(&mut self);
    fn enable_primary_baseline_bank_0(&mut self);
    fn enable_primary_baseline_bank_1(&mut self);
    fn prepare_output(&mut self);
    fn release_output_0(&mut self);
    fn release_output_1(&mut self);
    fn mask_primary_baseline_bank_0(&mut self);
    fn mask_primary_baseline_bank_1(&mut self);
}

struct HardwareInterruptControl<'a, B: oer_mmio::Placement> {
    bank: &'a mut oer_esp32s31_pac::bt_mac::BluetoothInterruptBank<B>,
}

impl<B: oer_mmio::Placement> BluetoothInterruptControl for HardwareInterruptControl<'_, B> {
    fn clear_primary_baseline_bank_0(&mut self) {
        self.bank.clear_bluetooth_primary_baseline_bank_0();
    }

    fn clear_primary_baseline_bank_1(&mut self) {
        self.bank.clear_bluetooth_primary_baseline_bank_1();
    }

    fn enable_primary_baseline_bank_0(&mut self) {
        self.bank.modify_irq_enable_0(|w| {
            w.set_source_15(true);
        });
    }

    fn enable_primary_baseline_bank_1(&mut self) {
        self.bank.modify_irq_enable_1(|w| {
            w.set_source_8(true);
            w.set_source_9(true);
            w.set_source_12(true);
        });
    }

    fn prepare_output(&mut self) {
        self.bank.write_irq_control_0(|w| {
            w.set_command(0x1);
        });
    }

    fn release_output_0(&mut self) {
        self.bank.write_irq_control_1(|w| {
            w.set_command(0x1);
        });
    }

    fn release_output_1(&mut self) {
        self.bank.write_irq_control_2(|w| {
            w.set_command(0x1);
        });
    }

    fn mask_primary_baseline_bank_0(&mut self) {
        self.bank.modify_irq_enable_0(|w| {
            w.set_source_15(false);
        });
    }

    fn mask_primary_baseline_bank_1(&mut self) {
        self.bank.modify_irq_enable_1(|w| {
            w.set_source_8(false);
            w.set_source_9(false);
            w.set_source_12(false);
        });
    }
}

#[derive(Clone, Copy)]
struct BluetoothPrimaryBank0Status {
    source_15_pending: bool,
    source_21_pending: bool,
    sources_27_or_28_pending: bool,
    unclassified_pending: bool,
}

impl BluetoothPrimaryBank0Status {
    /// Whether no bit of the bank was pending: the four cover all of them.
    const fn is_clear(self) -> bool {
        !(self.source_15_pending
            || self.source_21_pending
            || self.sources_27_or_28_pending
            || self.unclassified_pending)
    }
}

#[derive(Clone, Copy)]
struct BluetoothPrimaryBank1Status {
    source_3_pending: bool,
    source_8_pending: bool,
    source_9_pending: bool,
    source_12_pending: bool,
    unclassified_pending: bool,
}

impl BluetoothPrimaryBank1Status {
    /// Whether no bit of the bank was pending: the five cover all of them.
    const fn is_clear(self) -> bool {
        !(self.source_3_pending
            || self.source_8_pending
            || self.source_9_pending
            || self.source_12_pending
            || self.unclassified_pending)
    }
}

trait BluetoothPrimaryInterruptControl {
    type Bank0Snapshot;
    type Bank1Snapshot;

    fn sample_bank_0(&mut self) -> Self::Bank0Snapshot;
    fn sample_bank_1(&mut self) -> Self::Bank1Snapshot;
    fn bank_0_status(&self, snapshot: &Self::Bank0Snapshot) -> BluetoothPrimaryBank0Status;
    fn bank_1_status(&self, snapshot: &Self::Bank1Snapshot) -> BluetoothPrimaryBank1Status;
    fn acknowledge_bank_0(&mut self, snapshot: Self::Bank0Snapshot);
    fn acknowledge_bank_1(&mut self, snapshot: Self::Bank1Snapshot);
    fn capture_diagnostic_detail_0(&mut self);
    fn capture_diagnostic_detail_1(&mut self);
    fn capture_diagnostic_state(&mut self);
}

impl<B: oer_mmio::Placement> BluetoothPrimaryInterruptControl for HardwareInterruptControl<'_, B> {
    type Bank0Snapshot = bank::BluetoothPrimaryInterruptBank0Snapshot;
    type Bank1Snapshot = bank::BluetoothPrimaryInterruptBank1Snapshot;

    fn sample_bank_0(&mut self) -> Self::Bank0Snapshot {
        self.bank.sample_bluetooth_primary_interrupt_bank_0()
    }

    fn sample_bank_1(&mut self) -> Self::Bank1Snapshot {
        self.bank.sample_bluetooth_primary_interrupt_bank_1()
    }

    fn bank_0_status(&self, snapshot: &Self::Bank0Snapshot) -> BluetoothPrimaryBank0Status {
        BluetoothPrimaryBank0Status {
            source_15_pending: snapshot.source_15(),
            source_21_pending: snapshot.source_21(),
            sources_27_or_28_pending: snapshot.sources_27_28().get() != 0,
            unclassified_pending: snapshot.bits_0_14_opaque().get() != 0
                || snapshot.bits_16_20_opaque().get() != 0
                || snapshot.bits_22_26_opaque().get() != 0
                || snapshot.bits_29_31_opaque().get() != 0,
        }
    }

    fn bank_1_status(&self, snapshot: &Self::Bank1Snapshot) -> BluetoothPrimaryBank1Status {
        BluetoothPrimaryBank1Status {
            source_3_pending: snapshot.source_3(),
            source_8_pending: snapshot.source_8(),
            source_9_pending: snapshot.source_9(),
            source_12_pending: snapshot.source_12(),
            unclassified_pending: snapshot.bits_0_2_opaque().get() != 0
                || snapshot.bits_4_7_opaque().get() != 0
                || snapshot.bits_10_11_opaque().get() != 0
                || snapshot.bits_13_31_opaque().get() != 0,
        }
    }

    fn acknowledge_bank_0(&mut self, snapshot: Self::Bank0Snapshot) {
        self.bank
            .acknowledge_bluetooth_primary_interrupt_bank_0(snapshot);
    }

    fn acknowledge_bank_1(&mut self, snapshot: Self::Bank1Snapshot) {
        self.bank
            .acknowledge_bluetooth_primary_interrupt_bank_1(snapshot);
    }

    fn capture_diagnostic_detail_0(&mut self) {
        let _ = self.bank.read_irq_diagnostic_detail_0();
    }

    fn capture_diagnostic_detail_1(&mut self) {
        let _ = self.bank.read_irq_diagnostic_detail_1();
    }

    fn capture_diagnostic_state(&mut self) {
        let _ = self.bank.read_irq_diagnostic_state();
    }
}

fn execute_primary_prepare(control: &mut impl BluetoothInterruptControl) {
    control.clear_primary_baseline_bank_0();
    control.clear_primary_baseline_bank_1();
    control.enable_primary_baseline_bank_0();
    control.enable_primary_baseline_bank_1();
    control.prepare_output();
}

fn execute_primary_release(control: &mut impl BluetoothInterruptControl) {
    control.release_output_0();
    control.release_output_1();
    control.mask_primary_baseline_bank_0();
    control.mask_primary_baseline_bank_1();
}

/// Controller-side interrupt output prepared by the bank's interrupt handler
/// when it opens the epoch.
///
/// This state owns the exact baseline clear/enable transaction followed by
/// `IRQ_CONTROL_0 = 1`, which the vendor runs immediately before it allocates
/// the primary CPU interrupt. It still exposes no status capture: the handler
/// stages it at once for sources 124 and 133.
#[must_use = "the prepared Bluetooth interrupt output must be staged or released"]
pub struct BluetoothInterruptOutputPrepared<P: BluetoothPlacements = Chip> {
    pub(crate) peripherals: InterruptBlocks<P>,
}

impl<P: BluetoothPlacements> BluetoothInterruptSetup<P> {
    /// Prepare the controller-side interrupt output in the bank's handler.
    ///
    /// SOURCE: complete ESP32-S31 `libbtdm_common.a` `btdm_hal.c` helpers. The
    /// composite setup clears bank images `0x0000_8000` and `0x0000_1300`,
    /// ORs those same baseline sources into the two enable banks, writes one
    /// to `0x2010_100c`, then the outer path calls the platform interrupt
    /// allocator for source 124. The earlier HAL-init part of that composite
    /// remains a separate lifecycle prerequisite. Each clear first masks its
    /// baseline sources in their enable bank.
    pub fn prepare_controller_output(mut self) -> BluetoothInterruptOutputPrepared<P> {
        let mut control = HardwareInterruptControl {
            bank: &mut self.peripherals.bluetooth_interrupt_bank,
        };
        execute_primary_prepare(&mut control);
        device_fence();
        BluetoothInterruptOutputPrepared {
            peripherals: self.peripherals,
        }
    }
}

impl<P: BluetoothPlacements> BluetoothInterruptOutputPrepared<P> {
    /// Release a prepared controller output after any CPU route has been
    /// removed.
    ///
    /// The complete teardown leaf frees the CPU route before this transaction,
    /// writes image one to `0x2010_1010` and then `0x2010_1014`, and finally
    /// clears the same baseline enable groups. Dynamic Link-Layer sources must
    /// already have been quiesced by their own owners.
    pub fn release_controller_output(mut self) -> BluetoothInterruptSetup<P> {
        let mut control = HardwareInterruptControl {
            bank: &mut self.peripherals.bluetooth_interrupt_bank,
        };
        execute_primary_release(&mut control);
        device_fence();
        BluetoothInterruptSetup {
            peripherals: self.peripherals,
        }
    }

    /// The prepared bank as its handler holds it for the epoch. Performs no
    /// MMIO; the handler serving sources 124 and 133 calls it when it opens
    /// the epoch and keeps the bank until the output is released.
    pub fn stage_for_handler(self) -> BluetoothInterruptRegisters<P> {
        BluetoothInterruptRegisters {
            peripherals: self.peripherals,
        }
    }
}

/// Semantic source presence requiring fail-closed primary IRQ handling.
///
/// Names remain positional because current evidence proves the control flow,
/// but not stable Link-Layer names for these undocumented sources. Raw status
/// and diagnostic register images remain private to the register owner.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BluetoothPrimaryFaultSources {
    bank_0_source_15_pending: bool,
    bank_1_source_8_pending: bool,
    bank_1_source_9_pending: bool,
    bank_1_source_12_pending: bool,
    unclassified_pending: bool,
}

impl BluetoothPrimaryFaultSources {
    /// No fault source pending, the base of the host-test epochs.
    #[cfg(any(test, feature = "test-support"))]
    const NONE_FOR_HOST_TEST: Self = Self {
        bank_0_source_15_pending: false,
        bank_1_source_8_pending: false,
        bank_1_source_9_pending: false,
        bank_1_source_12_pending: false,
        unclassified_pending: false,
    };

    const fn from_status(
        bank_0: BluetoothPrimaryBank0Status,
        bank_1: BluetoothPrimaryBank1Status,
    ) -> Self {
        Self {
            bank_0_source_15_pending: bank_0.source_15_pending,
            bank_1_source_8_pending: bank_1.source_8_pending,
            bank_1_source_9_pending: bank_1.source_9_pending,
            bank_1_source_12_pending: bank_1.source_12_pending,
            unclassified_pending: bank_0.unclassified_pending || bank_1.unclassified_pending,
        }
    }

    /// Whether a reviewed fault source or an unclassified source was pending.
    pub const fn is_fault(self) -> bool {
        self.bank_0_source_15_pending
            || self.bank_1_source_8_pending
            || self.bank_1_source_9_pending
            || self.bank_1_source_12_pending
            || self.unclassified_pending
    }

    /// Whether positional bank-zero source 15 was pending.
    pub const fn bank_0_source_15_pending(self) -> bool {
        self.bank_0_source_15_pending
    }

    /// Whether positional bank-one source 8 was pending.
    pub const fn bank_1_source_8_pending(self) -> bool {
        self.bank_1_source_8_pending
    }

    /// Whether positional bank-one source 9 was pending.
    pub const fn bank_1_source_9_pending(self) -> bool {
        self.bank_1_source_9_pending
    }

    /// Whether positional bank-one source 12 was pending.
    pub const fn bank_1_source_12_pending(self) -> bool {
        self.bank_1_source_12_pending
    }

    /// Whether at least one status source without reviewed handler semantics
    /// was pending in either bank.
    pub const fn unclassified_pending(self) -> bool {
        self.unclassified_pending
    }
}

/// Proof that one NRT status pair was sampled and acknowledged in order.
///
/// The default profile has no semantic consumer for either status bank, so the
/// token deliberately carries no register image and exposes no raw diagnostic
/// escape hatch.
#[derive(Debug, Eq, PartialEq)]
#[must_use = "the acknowledged NRT epoch must be retained or explicitly consumed"]
pub struct BluetoothNrtInterruptAcknowledged {
    _private: (),
}

impl BluetoothNrtInterruptAcknowledged {
    /// Construct an acknowledged token for host-only controller tests.
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub const fn for_host_test() -> Self {
        Self { _private: () }
    }
}

/// One complete primary source-124 sample, acknowledgement and fault capture.
///
/// The register owner projects raw status into positional source facts before this value
/// crosses the hardware boundary. Conditional diagnostic reads still happen at
/// their reviewed temporal points, but their undocumented register images are
/// not published as Controller API.
#[derive(Debug, Eq, PartialEq)]
#[must_use = "a primary interrupt epoch must be classified before returning from the handler"]
pub struct BluetoothPrimaryInterruptEpoch {
    bank_0_source_21_pending: bool,
    bank_0_sources_27_or_28_pending: bool,
    bank_1_source_3_pending: bool,
    fault_sources: BluetoothPrimaryFaultSources,
}

impl BluetoothPrimaryInterruptEpoch {
    /// Construct one fault-free epoch from positional source presence for
    /// host tests, without exposing interrupt-bank bit geometry above this
    /// module.
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub const fn for_dynamic_host_test(
        bank_0_source_21_pending: bool,
        bank_0_sources_27_or_28_pending: bool,
        bank_1_source_3_pending: bool,
    ) -> Self {
        Self {
            bank_0_source_21_pending,
            bank_0_sources_27_or_28_pending,
            bank_1_source_3_pending,
            fault_sources: BluetoothPrimaryFaultSources::NONE_FOR_HOST_TEST,
        }
    }

    /// Construct one representative baseline-fault epoch (bank-one source 8)
    /// for upper-layer fail-stop host tests.
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub const fn for_fault_host_test() -> Self {
        Self {
            bank_0_source_21_pending: false,
            bank_0_sources_27_or_28_pending: false,
            bank_1_source_3_pending: false,
            fault_sources: BluetoothPrimaryFaultSources {
                bank_1_source_8_pending: true,
                ..BluetoothPrimaryFaultSources::NONE_FOR_HOST_TEST
            },
        }
    }

    /// Construct one epoch with only a source whose handler semantics remain
    /// unclassified, for upper-layer fail-closed host tests.
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub const fn for_unclassified_host_test() -> Self {
        Self {
            bank_0_source_21_pending: false,
            bank_0_sources_27_or_28_pending: false,
            bank_1_source_3_pending: false,
            fault_sources: BluetoothPrimaryFaultSources {
                unclassified_pending: true,
                ..BluetoothPrimaryFaultSources::NONE_FOR_HOST_TEST
            },
        }
    }

    /// Whether positional dynamic bank-zero source 21 was pending.
    pub const fn bank_0_source_21_pending(&self) -> bool {
        self.bank_0_source_21_pending
    }

    /// Whether positional dynamic bank-zero source 27 or 28 was pending.
    pub const fn bank_0_sources_27_or_28_pending(&self) -> bool {
        self.bank_0_sources_27_or_28_pending
    }

    /// Whether positional dynamic bank-one source 3 was pending.
    pub const fn bank_1_source_3_pending(&self) -> bool {
        self.bank_1_source_3_pending
    }

    /// Semantic fault-source presence captured in the acknowledged epoch.
    pub const fn fault_sources(&self) -> BluetoothPrimaryFaultSources {
        self.fault_sources
    }
}

/// One primary epoch, or `None` when neither bank held a pending bit: the
/// handler of the shared line then writes nothing.
fn execute_primary_interrupt_epoch(
    control: &mut impl BluetoothPrimaryInterruptControl,
) -> Option<BluetoothPrimaryInterruptEpoch> {
    let bank_0 = control.sample_bank_0();
    let bank_1 = control.sample_bank_1();
    let bank_0_status = control.bank_0_status(&bank_0);
    let bank_1_status = control.bank_1_status(&bank_1);
    if bank_0_status.is_clear() && bank_1_status.is_clear() {
        return None;
    }
    control.acknowledge_bank_0(bank_0);
    control.acknowledge_bank_1(bank_1);

    let fault_sources = BluetoothPrimaryFaultSources::from_status(bank_0_status, bank_1_status);
    if fault_sources.bank_1_source_9_pending() {
        control.capture_diagnostic_detail_0();
        control.capture_diagnostic_detail_1();
    }
    if fault_sources.bank_1_source_12_pending() {
        control.capture_diagnostic_state();
    }

    Some(BluetoothPrimaryInterruptEpoch {
        bank_0_source_21_pending: bank_0_status.source_21_pending,
        bank_0_sources_27_or_28_pending: bank_0_status.sources_27_or_28_pending,
        bank_1_source_3_pending: bank_1_status.source_3_pending,
        fault_sources,
    })
}

impl<P: BluetoothPlacements> BluetoothInterruptRegisters<P> {
    /// Clear stale scheduler-run sources and enable their dynamic groups.
    ///
    /// SOURCE: complete current `r_sym_bt_DOVkQWJHjeuid8jcS9Bq` followed by
    /// `r_sym_bt_6lAYUFKOuBLyOZ6Kvsv5`; same-chip named
    /// `r_btdm_hal_link_basic_irq_clear` and
    /// `r_btdm_hal_link_basic_irq_enable`. The exact order is W1C bank zero,
    /// W1C bank one, enable bank zero, enable bank one. Each W1C first masks
    /// its sources in its enable bank, so an event of a group still enabled
    /// by an earlier preparation is never acknowledged without a sample.
    pub fn prepare_scheduler_run_interrupts(&mut self) -> BluetoothSchedulerRunInterruptsPrepared {
        let mut control = HardwareInterruptControl {
            bank: &mut self.peripherals.bluetooth_interrupt_bank,
        };
        execute_scheduler_run_interrupt_prepare(&mut control);
        device_fence();
        BluetoothSchedulerRunInterruptsPrepared { _private: () }
    }

    /// Capture and acknowledge one complete primary BT MAC interrupt image.
    ///
    /// This is the exact prefix of the source-124 handler: read masked status
    /// bank zero, read masked status bank one, copy the first image to clear
    /// bank zero, then copy the second image to clear bank one. The transaction
    /// then preserves the complete diagnostic prefix: source 9 captures both
    /// detail words and source 12 captures the state word. Callback dispatch
    /// and the special scheduler-event suffix remain higher-layer work.
    ///
    /// The bank shares its handler's line with other sources, so an entry
    /// may find both status words clear: it then writes nothing and returns
    /// `None`.
    pub fn capture_primary_and_acknowledge(&mut self) -> Option<BluetoothPrimaryInterruptEpoch> {
        let mut control = HardwareInterruptControl {
            bank: &mut self.peripherals.bluetooth_interrupt_bank,
        };
        let epoch = execute_primary_interrupt_epoch(&mut control)?;
        device_fence();
        Some(epoch)
    }

    /// Capture and acknowledge one complete controller interrupt epoch.
    ///
    /// The order is the exact complete ESP32-S31 NRT ISR prefix:
    ///
    /// 1. read first status snapshot;
    /// 2. read second status snapshot;
    /// 3. write the first image to its write-one-to-clear bank;
    /// 4. write the second image to its write-one-to-clear bank.
    ///
    /// Separate sample or acknowledgement methods are deliberately absent:
    /// the reviewed vendor body does not authorize another ordering.
    ///
    /// The bank shares its handler's line with other sources, so an entry
    /// may find both snapshots clear: it then writes nothing and returns
    /// `None`.
    pub fn capture_nrt_and_acknowledge(&mut self) -> Option<BluetoothNrtInterruptAcknowledged> {
        let bank_0 = self
            .peripherals
            .bluetooth_interrupt_bank
            .sample_bluetooth_interrupt_bank_0();
        let bank_1 = self
            .peripherals
            .bluetooth_interrupt_bank
            .sample_bluetooth_interrupt_bank_1();
        if bank_0.is_clear() && bank_1.is_clear() {
            return None;
        }
        self.peripherals
            .bluetooth_interrupt_bank
            .acknowledge_bluetooth_interrupt_bank_0(bank_0);
        self.peripherals
            .bluetooth_interrupt_bank
            .acknowledge_bluetooth_interrupt_bank_1(bank_1);
        device_fence();
        Some(BluetoothNrtInterruptAcknowledged { _private: () })
    }
}

#[cfg(test)]
mod tests;
