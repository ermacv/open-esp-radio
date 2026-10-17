//! Restricted ownership for the reviewed Bluetooth interrupt transaction.

#![deny(unsafe_code)]

pub(crate) mod retirement;

use crate::{
    BluetoothInterruptRegisters, BluetoothInterruptSetup, device_fence,
    svd::{
        field_or_modify, field_read, field_replace_modify, fixed_register_image, interrupt_snapshot,
    },
};

trait BluetoothSchedulerRunInterruptControl {
    fn clear_scheduler_run_bank_0(&mut self);
    fn clear_scheduler_run_bank_1(&mut self);
    fn enable_scheduler_run_bank_0(&mut self);
    fn enable_scheduler_run_bank_1(&mut self);
}

impl BluetoothSchedulerRunInterruptControl for HardwareInterruptControl<'_> {
    fn clear_scheduler_run_bank_0(&mut self) {
        fixed_register_image::clear_bluetooth_scheduler_run_interrupts_bank_0(self.bank);
    }

    fn clear_scheduler_run_bank_1(&mut self) {
        fixed_register_image::clear_bluetooth_scheduler_run_interrupts_bank_1(self.bank);
    }

    fn enable_scheduler_run_bank_0(&mut self) {
        field_or_modify::enable_bluetooth_scheduler_run_interrupts_bank_0(self.bank);
    }

    fn enable_scheduler_run_bank_1(&mut self) {
        field_or_modify::enable_bluetooth_scheduler_run_interrupts_bank_1(self.bank);
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

struct HardwareInterruptControl<'a> {
    bank: &'a crate::svd::BluetoothInterruptBank,
}

impl BluetoothInterruptControl for HardwareInterruptControl<'_> {
    fn clear_primary_baseline_bank_0(&mut self) {
        fixed_register_image::clear_bluetooth_primary_baseline_bank_0(self.bank);
    }

    fn clear_primary_baseline_bank_1(&mut self) {
        fixed_register_image::clear_bluetooth_primary_baseline_bank_1(self.bank);
    }

    fn enable_primary_baseline_bank_0(&mut self) {
        field_or_modify::enable_bluetooth_primary_baseline_bank_0(self.bank);
    }

    fn enable_primary_baseline_bank_1(&mut self) {
        field_or_modify::enable_bluetooth_primary_baseline_bank_1(self.bank);
    }

    fn prepare_output(&mut self) {
        fixed_register_image::prepare_bluetooth_interrupt_output(self.bank);
    }

    fn release_output_0(&mut self) {
        fixed_register_image::release_bluetooth_interrupt_output_0(self.bank);
    }

    fn release_output_1(&mut self) {
        fixed_register_image::release_bluetooth_interrupt_output_1(self.bank);
    }

    fn mask_primary_baseline_bank_0(&mut self) {
        field_replace_modify::mask_bluetooth_primary_baseline_bank_0(self.bank);
    }

    fn mask_primary_baseline_bank_1(&mut self) {
        field_replace_modify::mask_bluetooth_primary_baseline_bank_1(self.bank);
    }
}

#[derive(Clone, Copy)]
struct BluetoothPrimaryBank0Status {
    source_15_pending: bool,
    source_21_pending: bool,
    sources_27_or_28_pending: bool,
    unclassified_pending: bool,
}

#[derive(Clone, Copy)]
struct BluetoothPrimaryBank1Status {
    source_3_pending: bool,
    source_8_pending: bool,
    source_9_pending: bool,
    source_12_pending: bool,
    unclassified_pending: bool,
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

impl BluetoothPrimaryInterruptControl for HardwareInterruptControl<'_> {
    type Bank0Snapshot = interrupt_snapshot::BluetoothPrimaryInterruptBank0Snapshot;
    type Bank1Snapshot = interrupt_snapshot::BluetoothPrimaryInterruptBank1Snapshot;

    fn sample_bank_0(&mut self) -> Self::Bank0Snapshot {
        interrupt_snapshot::sample_bluetooth_primary_interrupt_bank_0(self.bank)
    }

    fn sample_bank_1(&mut self) -> Self::Bank1Snapshot {
        interrupt_snapshot::sample_bluetooth_primary_interrupt_bank_1(self.bank)
    }

    fn bank_0_status(&self, snapshot: &Self::Bank0Snapshot) -> BluetoothPrimaryBank0Status {
        BluetoothPrimaryBank0Status {
            source_15_pending: snapshot.source_15(),
            source_21_pending: snapshot.source_21(),
            sources_27_or_28_pending: snapshot.sources_27_28() != 0,
            unclassified_pending: snapshot.bits_0_14_opaque() != 0
                || snapshot.bits_16_20_opaque() != 0
                || snapshot.bits_22_26_opaque() != 0
                || snapshot.bits_29_31_opaque() != 0,
        }
    }

    fn bank_1_status(&self, snapshot: &Self::Bank1Snapshot) -> BluetoothPrimaryBank1Status {
        BluetoothPrimaryBank1Status {
            source_3_pending: snapshot.source_3(),
            source_8_pending: snapshot.source_8(),
            source_9_pending: snapshot.source_9(),
            source_12_pending: snapshot.source_12(),
            unclassified_pending: snapshot.bits_0_2_opaque() != 0
                || snapshot.bits_4_7_opaque() != 0
                || snapshot.bits_10_11_opaque() != 0
                || snapshot.bits_13_31_opaque() != 0,
        }
    }

    fn acknowledge_bank_0(&mut self, snapshot: Self::Bank0Snapshot) {
        interrupt_snapshot::acknowledge_bluetooth_primary_interrupt_bank_0(self.bank, snapshot);
    }

    fn acknowledge_bank_1(&mut self, snapshot: Self::Bank1Snapshot) {
        interrupt_snapshot::acknowledge_bluetooth_primary_interrupt_bank_1(self.bank, snapshot);
    }

    fn capture_diagnostic_detail_0(&mut self) {
        let _ = field_read::capture_bluetooth_irq_diagnostic_detail_0(self.bank);
    }

    fn capture_diagnostic_detail_1(&mut self) {
        let _ = field_read::capture_bluetooth_irq_diagnostic_detail_1(self.bank);
    }

    fn capture_diagnostic_state(&mut self) {
        let _ = field_read::capture_bluetooth_irq_diagnostic_state(self.bank);
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

/// Controller-side interrupt output prepared before a CPU route is installed.
///
/// This state owns the exact baseline clear/enable transaction followed by
/// `IRQ_CONTROL_0 = 1`, immediately before the vendor platform allocates the
/// primary CPU interrupt. It still exposes no status capture: both CPU routes
/// must share one staged ISR owner before either route is enabled.
#[must_use = "the prepared Bluetooth interrupt output must be routed or released"]
pub struct BluetoothInterruptOutputPrepared {
    peripherals: crate::svd::peripheral_ownership::BluetoothInterruptPeripherals,
}

impl BluetoothInterruptSetup {
    /// Prepare the controller-side interrupt output before installing a CPU
    /// route.
    ///
    /// SOURCE: complete ESP32-S31 `libbtdm_common.a` `btdm_hal.c` helpers. The
    /// composite setup clears bank images `0x0000_8000` and `0x0000_1300`,
    /// ORs those same baseline sources into the two enable banks, writes one
    /// to `0x2010_100c`, then the outer path calls the platform interrupt
    /// allocator for source 124. The earlier HAL-init part of that composite
    /// remains a separate lifecycle prerequisite.
    pub fn prepare_controller_output(self) -> BluetoothInterruptOutputPrepared {
        let mut control = HardwareInterruptControl {
            bank: &self.peripherals.bluetooth_interrupt_bank,
        };
        execute_primary_prepare(&mut control);
        device_fence();
        BluetoothInterruptOutputPrepared {
            peripherals: self.peripherals,
        }
    }
}

impl BluetoothInterruptOutputPrepared {
    /// Release a prepared controller output after any CPU route has been
    /// removed.
    ///
    /// The complete teardown leaf frees the CPU route before this transaction,
    /// writes image one to `0x2010_1010` and then `0x2010_1014`, and finally
    /// clears the same baseline enable groups. Dynamic Link-Layer sources must
    /// already have been quiesced by their own owners.
    pub fn release_controller_output(self) -> BluetoothInterruptSetup {
        let mut control = HardwareInterruptControl {
            bank: &self.peripherals.bluetooth_interrupt_bank,
        };
        execute_primary_release(&mut control);
        device_fence();
        BluetoothInterruptSetup {
            peripherals: self.peripherals,
        }
    }

    /// Transfer the prepared bank into stable storage shared by both hard
    /// handlers before either CPU route is enabled.
    ///
    /// This conversion performs no MMIO and does not itself prove that source
    /// 124 or source 133 has been routed. A platform adapter must retain the
    /// returned value in interrupt-safe storage, bind both routes on one core,
    /// and recover it only after both routes have been disabled.
    pub fn stage_for_cpu_routes(self) -> BluetoothInterruptRegisters {
        BluetoothInterruptRegisters {
            peripherals: self.peripherals,
        }
    }
}

impl BluetoothInterruptRegisters {
    /// Return the interrupt partition to controller-output-only ownership.
    ///
    /// The caller must mask events and disable the CPU route first. This
    /// method performs no controller transaction; the separate output-release
    /// edge publishes the reviewed teardown strobes.
    pub fn deactivate(self) -> BluetoothInterruptOutputPrepared {
        BluetoothInterruptOutputPrepared {
            peripherals: self.peripherals,
        }
    }
}

/// Semantic source presence requiring fail-closed primary IRQ handling.
///
/// Names remain positional because current evidence proves the control flow,
/// but not stable Link-Layer names for these undocumented sources. Raw status
/// and diagnostic register images remain private to the PAC.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BluetoothPrimaryFaultSources {
    bank_0_source_15_pending: bool,
    bank_1_source_8_pending: bool,
    bank_1_source_9_pending: bool,
    bank_1_source_12_pending: bool,
    unclassified_pending: bool,
}

impl BluetoothPrimaryFaultSources {
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

#[cfg(feature = "validation-probes")]
impl BluetoothNrtInterruptAcknowledged {
    /// Construct an acknowledged token for host-only controller tests.
    #[doc(hidden)]
    pub const fn for_validation() -> Self {
        Self { _private: () }
    }
}

/// One complete primary source-124 sample, acknowledgement and fault capture.
///
/// The PAC projects raw status into positional source facts before this value
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

    /// Construct one fault-free epoch from positional source presence without
    /// exposing interrupt-bank bit geometry above the PAC.
    #[cfg(feature = "validation-probes")]
    #[doc(hidden)]
    pub const fn for_dynamic_validation(
        bank_0_source_21_pending: bool,
        bank_0_sources_27_or_28_pending: bool,
        bank_1_source_3_pending: bool,
    ) -> Self {
        Self {
            bank_0_source_21_pending,
            bank_0_sources_27_or_28_pending,
            bank_1_source_3_pending,
            fault_sources: BluetoothPrimaryFaultSources {
                bank_0_source_15_pending: false,
                bank_1_source_8_pending: false,
                bank_1_source_9_pending: false,
                bank_1_source_12_pending: false,
                unclassified_pending: false,
            },
        }
    }

    /// Construct one representative baseline-fault epoch for upper-layer
    /// fail-stop validation without exporting the matching hardware mask.
    #[cfg(feature = "validation-probes")]
    #[doc(hidden)]
    pub const fn for_fault_validation() -> Self {
        Self {
            bank_0_source_21_pending: false,
            bank_0_sources_27_or_28_pending: false,
            bank_1_source_3_pending: false,
            fault_sources: BluetoothPrimaryFaultSources {
                bank_0_source_15_pending: false,
                bank_1_source_8_pending: true,
                bank_1_source_9_pending: false,
                bank_1_source_12_pending: false,
                unclassified_pending: false,
            },
        }
    }

    /// Construct one epoch containing only a source whose handler semantics
    /// remain unclassified, for upper-layer fail-closed validation.
    #[cfg(feature = "validation-probes")]
    #[doc(hidden)]
    pub const fn for_unclassified_validation() -> Self {
        Self {
            bank_0_source_21_pending: false,
            bank_0_sources_27_or_28_pending: false,
            bank_1_source_3_pending: false,
            fault_sources: BluetoothPrimaryFaultSources {
                bank_0_source_15_pending: false,
                bank_1_source_8_pending: false,
                bank_1_source_9_pending: false,
                bank_1_source_12_pending: false,
                unclassified_pending: true,
            },
        }
    }
}

fn execute_primary_interrupt_epoch(
    control: &mut impl BluetoothPrimaryInterruptControl,
) -> BluetoothPrimaryInterruptEpoch {
    let bank_0 = control.sample_bank_0();
    let bank_1 = control.sample_bank_1();
    let bank_0_status = control.bank_0_status(&bank_0);
    let bank_1_status = control.bank_1_status(&bank_1);
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

    BluetoothPrimaryInterruptEpoch {
        bank_0_source_21_pending: bank_0_status.source_21_pending,
        bank_0_sources_27_or_28_pending: bank_0_status.sources_27_or_28_pending,
        bank_1_source_3_pending: bank_1_status.source_3_pending,
        fault_sources,
    }
}

impl BluetoothInterruptRegisters {
    /// Clear stale scheduler-run sources and enable their dynamic groups.
    ///
    /// SOURCE: complete current `r_sym_bt_DOVkQWJHjeuid8jcS9Bq` followed by
    /// `r_sym_bt_6lAYUFKOuBLyOZ6Kvsv5`; same-chip named
    /// `r_btdm_hal_link_basic_irq_clear` and
    /// `r_btdm_hal_link_basic_irq_enable`. The exact order is W1C bank zero,
    /// W1C bank one, enable bank zero, enable bank one.
    pub fn prepare_scheduler_run_interrupts(&mut self) -> BluetoothSchedulerRunInterruptsPrepared {
        let mut control = HardwareInterruptControl {
            bank: &self.peripherals.bluetooth_interrupt_bank,
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
    pub fn capture_primary_and_acknowledge(&mut self) -> BluetoothPrimaryInterruptEpoch {
        let mut control = HardwareInterruptControl {
            bank: &self.peripherals.bluetooth_interrupt_bank,
        };
        let epoch = execute_primary_interrupt_epoch(&mut control);
        device_fence();
        epoch
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
    pub fn capture_nrt_and_acknowledge(&mut self) -> BluetoothNrtInterruptAcknowledged {
        let bank_0 = interrupt_snapshot::sample_bluetooth_interrupt_bank_0(
            &self.peripherals.bluetooth_interrupt_bank,
        );
        let bank_1 = interrupt_snapshot::sample_bluetooth_interrupt_bank_1(
            &self.peripherals.bluetooth_interrupt_bank,
        );
        interrupt_snapshot::acknowledge_bluetooth_interrupt_bank_0(
            &self.peripherals.bluetooth_interrupt_bank,
            bank_0,
        );
        interrupt_snapshot::acknowledge_bluetooth_interrupt_bank_1(
            &self.peripherals.bluetooth_interrupt_bank,
            bank_1,
        );
        device_fence();
        BluetoothNrtInterruptAcknowledged { _private: () }
    }
}

#[cfg(test)]
mod tests;
