//! The Bluetooth interrupt bank's transactions: output preparation and
//! release, the scheduler-run sources and the source-124 and source-133
//! samples and acknowledgements.

#![deny(unsafe_code)]

pub(crate) mod retirement;

use crate::radio::bluetooth::registers::{
    BluetoothInterruptRegisters, BluetoothInterruptSetup, BluetoothPlacements, Chip,
    InterruptBlocks, device_fence,
};
use crate::set::Set;
use oer_chips_esp32s31_pac::bt_mac as bank;

trait BluetoothSchedulerRunInterruptControl {
    fn clear_scheduler_run_bank_0(&mut self);
    fn clear_scheduler_run_bank_1(&mut self);
    fn enable_scheduler_run_bank_0(&mut self);
    fn enable_scheduler_run_bank_1(&mut self);
}

impl<B: oer_portable_mmio::Placement> BluetoothSchedulerRunInterruptControl
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
            w.set_sources_27_28(oer_chips_esp32s31_pac::Bits::truncate(
                w.sources_27_28().get() | 0x3,
            ));
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
    fn silence_controller_time_overflow(&mut self);
}

struct HardwareInterruptControl<'a, B: oer_portable_mmio::Placement> {
    bank: &'a mut oer_chips_esp32s31_pac::bt_mac::BluetoothInterruptBank<B>,
}

impl<B: oer_portable_mmio::Placement> BluetoothInterruptControl
    for HardwareInterruptControl<'_, B>
{
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
            w.set_controller_time_overflow(true);
            w.set_source_8(true);
            w.set_source_9(true);
            w.set_source_12(true);
        });
    }

    fn prepare_output(&mut self) {
        self.bank.write_irq_control_0(|w| {
            w.set_command(oer_chips_esp32s31_pac::bt_mac::irq_control_0::Command::Command1.bits());
        });
    }

    fn release_output_0(&mut self) {
        self.bank.write_irq_control_1(|w| {
            w.set_command(oer_chips_esp32s31_pac::bt_mac::irq_control_1::Command::Command1.bits());
        });
    }

    fn release_output_1(&mut self) {
        self.bank.write_irq_control_2(|w| {
            w.set_command(oer_chips_esp32s31_pac::bt_mac::irq_control_2::Command::Command1.bits());
        });
    }

    fn mask_primary_baseline_bank_0(&mut self) {
        self.bank.modify_irq_enable_0(|w| {
            w.set_source_15(false);
        });
    }

    fn mask_primary_baseline_bank_1(&mut self) {
        self.bank.modify_irq_enable_1(|w| {
            w.set_controller_time_overflow(false);
            w.set_source_8(false);
            w.set_source_9(false);
            w.set_source_12(false);
        });
    }

    fn silence_controller_time_overflow(&mut self) {
        // SOURCE: HW12_S31_BLE_OVERFLOW_ACK_20261026 and
        // HW9_S31_MAC_MASK_ACK_20261027: masking retains pending. This
        // generated transaction masks only overflow, then clears it.
        self.bank.clear_bluetooth_controller_time_overflow();
    }
}

crate::set::members! {
    /// The sources of the primary interrupt's two status banks, by their
    /// positions: current evidence proves the control flow, not stable
    /// Link-Layer names for these undocumented sources.
    pub enum BluetoothPrimarySource {
        /// Bank one: the BTLC first-word wrap.
        ControllerTimeOverflow,
        Bank0Source15,
        Bank0Source21,
        /// Bank zero: source 27 or 28.
        Bank0Sources27Or28,
        Bank1Source3,
        Bank1Source8,
        Bank1Source9,
        Bank1Source12,
        /// A pending bit of either bank without reviewed handler semantics.
        Unclassified,
    }
}

/// The pending sources of a sampled status bank.
type BluetoothPrimaryStatus = Set<BluetoothPrimarySource>;

/// The sources of a fault: reviewed fault sources and unclassified ones.
const FAULT_SOURCES: BluetoothPrimaryStatus = BluetoothPrimarySource::Bank0Source15
    .set()
    .union(BluetoothPrimarySource::Bank1Source8.set())
    .union(BluetoothPrimarySource::Bank1Source9.set())
    .union(BluetoothPrimarySource::Bank1Source12.set())
    .union(BluetoothPrimarySource::Unclassified.set());

/// The sources an epoch reports beside its fault sources.
const DYNAMIC_SOURCES: BluetoothPrimaryStatus = BluetoothPrimarySource::Bank0Source21
    .set()
    .union(BluetoothPrimarySource::Bank0Sources27Or28.set())
    .union(BluetoothPrimarySource::Bank1Source3.set());

trait BluetoothPrimaryInterruptControl {
    type Bank0Snapshot;
    type Bank1Snapshot;

    fn sample_bank_0(&mut self) -> Self::Bank0Snapshot;
    fn sample_bank_1(&mut self) -> Self::Bank1Snapshot;
    fn bank_0_status(&self, snapshot: &Self::Bank0Snapshot) -> BluetoothPrimaryStatus;
    fn bank_1_status(&self, snapshot: &Self::Bank1Snapshot) -> BluetoothPrimaryStatus;
    fn acknowledge_bank_0(&mut self, snapshot: Self::Bank0Snapshot);
    fn acknowledge_bank_1(&mut self, snapshot: Self::Bank1Snapshot);
    fn capture_diagnostic_detail_0(&mut self);
    fn capture_diagnostic_detail_1(&mut self);
    fn capture_diagnostic_state(&mut self);
}

impl<B: oer_portable_mmio::Placement> BluetoothPrimaryInterruptControl
    for HardwareInterruptControl<'_, B>
{
    type Bank0Snapshot = bank::BluetoothPrimaryInterruptBank0Snapshot;
    type Bank1Snapshot = bank::BluetoothPrimaryInterruptBank1Snapshot;

    fn sample_bank_0(&mut self) -> Self::Bank0Snapshot {
        self.bank.sample_bluetooth_primary_interrupt_bank_0()
    }

    fn sample_bank_1(&mut self) -> Self::Bank1Snapshot {
        self.bank.sample_bluetooth_primary_interrupt_bank_1()
    }

    fn bank_0_status(&self, snapshot: &Self::Bank0Snapshot) -> BluetoothPrimaryStatus {
        let unclassified = snapshot.bits_0_14_opaque().get() != 0
            || snapshot.bits_16_20_opaque().get() != 0
            || snapshot.bits_22_26_opaque().get() != 0
            || snapshot.bits_29_31_opaque().get() != 0;
        Set::of_bit(snapshot.source_15(), BluetoothPrimarySource::Bank0Source15)
            .union(Set::of_bit(
                snapshot.source_21(),
                BluetoothPrimarySource::Bank0Source21,
            ))
            .union(Set::of_bit(
                snapshot.sources_27_28().get() != 0,
                BluetoothPrimarySource::Bank0Sources27Or28,
            ))
            .union(Set::of_bit(
                unclassified,
                BluetoothPrimarySource::Unclassified,
            ))
    }

    fn bank_1_status(&self, snapshot: &Self::Bank1Snapshot) -> BluetoothPrimaryStatus {
        let unclassified = snapshot.bit_0_opaque()
            || snapshot.bit_2_opaque()
            || snapshot.bits_4_7_opaque().get() != 0
            || snapshot.bits_10_11_opaque().get() != 0
            || snapshot.bits_13_31_opaque().get() != 0;
        Set::of_bit(
            snapshot.controller_time_overflow(),
            BluetoothPrimarySource::ControllerTimeOverflow,
        )
        .union(Set::of_bit(
            snapshot.source_3(),
            BluetoothPrimarySource::Bank1Source3,
        ))
        .union(Set::of_bit(
            snapshot.source_8(),
            BluetoothPrimarySource::Bank1Source8,
        ))
        .union(Set::of_bit(
            snapshot.source_9(),
            BluetoothPrimarySource::Bank1Source9,
        ))
        .union(Set::of_bit(
            snapshot.source_12(),
            BluetoothPrimarySource::Bank1Source12,
        ))
        .union(Set::of_bit(
            unclassified,
            BluetoothPrimarySource::Unclassified,
        ))
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
    // Known release limitation: these baseline clears silently discard a
    // retained fault that arrived after the previous epoch's last visible
    // release check. Masked pending is unobservable; release never reopens
    // enables to inspect it. SOURCE: HW32_S31_BT_MASKED_FIXED_W1C_20261027,
    // HW32_S31_BT_OUTPUT_OBSERVER_20261027 (PR #757; measured timer lanes,
    // not a positive fault-lane qualification).
    control.clear_primary_baseline_bank_0();
    control.clear_primary_baseline_bank_1();
    control.enable_primary_baseline_bank_0();
    control.enable_primary_baseline_bank_1();
    control.prepare_output();
}

fn execute_primary_release(control: &mut impl BluetoothInterruptControl) {
    control.silence_controller_time_overflow();
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
    /// SOURCE: BLOB_LIBBTDM_CONTROLLER_IRQ: reviewed ESP32-S31 baseline
    /// helpers. The composite setup clears bank images `0x0000_8000` and
    /// `0x0000_1302`, ORs those same baseline sources into the two enable banks,
    /// writes one
    /// to `0x2010_100c`, then the outer path calls the platform interrupt
    /// allocator for source 124. The earlier HAL-init part of that composite
    /// remains a separate lifecycle prerequisite. Each clear first masks its
    /// baseline sources in their enable bank. Bank-one bit 1 is controller-time
    /// overflow (SOURCE: HW12_S31_BLE_OVERFLOW_ACK_20261026;
    /// BLOB_LIBBTDM_BASELINE_IRQ_CURRENT, N5). CONTROL0 starts BTLC; it is
    /// not merely an interrupt-route command (HW9_S31_BTLC_START_20261027).
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
    /// First mask and clear controller-time overflow, then write image one to
    /// CONTROL1 and CONTROL2 and mask the remaining baseline groups. CONTROL1/2
    /// do not lower the output of an enabled pending cause (SOURCE:
    /// HW9_S31_MAC_RELEASE_20261027). Dynamic Link-Layer sources must already
    /// have been quiesced by their own owners. The bank remains in its bound
    /// handler throughout this transaction.
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
pub struct BluetoothPrimaryFaultSources(BluetoothPrimaryStatus);

impl BluetoothPrimaryFaultSources {
    /// No fault source pending, the base of the host-test epochs.
    #[cfg(any(test, feature = "test-support"))]
    const NONE_FOR_HOST_TEST: Self = Self(Set::empty());

    const fn from_status(status: BluetoothPrimaryStatus) -> Self {
        Self(status.intersection(FAULT_SOURCES))
    }

    /// The pending fault sources: each is one of the reviewed fault
    /// sources or [`BluetoothPrimarySource::Unclassified`].
    pub const fn sources(self) -> Set<BluetoothPrimarySource> {
        self.0
    }

    /// Whether a reviewed fault source or an unclassified source was pending.
    pub const fn is_fault(self) -> bool {
        !self.0.is_empty()
    }

    /// Whether positional bank-zero source 15 was pending.
    pub const fn bank_0_source_15_pending(self) -> bool {
        self.0.includes(BluetoothPrimarySource::Bank0Source15.set())
    }

    /// Whether positional bank-one source 8 was pending.
    pub const fn bank_1_source_8_pending(self) -> bool {
        self.0.includes(BluetoothPrimarySource::Bank1Source8.set())
    }

    /// Whether positional bank-one source 9 was pending.
    pub const fn bank_1_source_9_pending(self) -> bool {
        self.0.includes(BluetoothPrimarySource::Bank1Source9.set())
    }

    /// Whether positional bank-one source 12 was pending.
    pub const fn bank_1_source_12_pending(self) -> bool {
        self.0.includes(BluetoothPrimarySource::Bank1Source12.set())
    }

    /// Whether at least one status source without reviewed handler semantics
    /// was pending in either bank.
    pub const fn unclassified_pending(self) -> bool {
        self.0.includes(BluetoothPrimarySource::Unclassified.set())
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

/// NRT unexpectedly exposed a primary-owned overflow or fault clear lane.
/// Neither bank of this exact sampled pair was acknowledged.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BluetoothNrtInterruptFault {
    /// Complete NRT observations, preserved before any acknowledgement.
    pub snapshots: [u32; 2],
}

/// Admission for a sampled NRT pair. Geometry comes from the shared clear
/// register: an opaque NRT word must never consume primary overflow or a
/// primary fault unseen. New arrivals absent from this affine sample are not
/// included in its acknowledgement.
fn acknowledge_nrt_if_admitted<T>(
    snapshots: [u32; 2],
    acknowledge: impl FnOnce() -> T,
) -> Result<T, BluetoothNrtInterruptFault> {
    if crate::radio::bluetooth::interrupt_policy::nrt_fault_visible(snapshots) {
        Err(BluetoothNrtInterruptFault { snapshots })
    } else {
        Ok(acknowledge())
    }
}

/// The same affine sample/admit/acknowledge transaction serves runtime and
/// terminal release. Diagnostic observations cannot supply its clear images.
fn capture_nrt_and_acknowledge<B: oer_portable_mmio::Placement>(
    bank: &mut bank::BluetoothInterruptBank<B>,
) -> Result<Option<BluetoothNrtInterruptAcknowledged>, BluetoothNrtInterruptFault> {
    let bank_0 = bank.sample_bluetooth_interrupt_bank_0();
    let bank_1 = bank.sample_bluetooth_interrupt_bank_1();
    acknowledge_nrt_if_admitted([bank_0.pending_bits(), bank_1.pending_bits()], || {
        if bank_0.is_clear() && bank_1.is_clear() {
            return None;
        }
        bank.acknowledge_bluetooth_interrupt_bank_0(bank_0);
        bank.acknowledge_bluetooth_interrupt_bank_1(bank_1);
        device_fence();
        Some(BluetoothNrtInterruptAcknowledged { _private: () })
    })
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
    /// The controller time overflow and the dynamic sources pending.
    sources: BluetoothPrimaryStatus,
    fault_sources: BluetoothPrimaryFaultSources,
}

impl BluetoothPrimaryInterruptEpoch {
    /// Construct one fault-free epoch from positional source presence for
    /// host tests, without exposing interrupt-bank bit geometry above this
    /// module.
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub const fn for_dynamic_host_test(sources: Set<BluetoothPrimarySource>) -> Self {
        Self {
            // A fault-free epoch: only the dynamic sources are taken.
            sources: sources.intersection(DYNAMIC_SOURCES),
            fault_sources: BluetoothPrimaryFaultSources::NONE_FOR_HOST_TEST,
        }
    }

    /// Construct one representative baseline-fault epoch (bank-one source 8)
    /// for upper-layer fail-stop host tests.
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub const fn for_fault_host_test() -> Self {
        Self {
            sources: Set::empty(),
            fault_sources: BluetoothPrimaryFaultSources(BluetoothPrimarySource::Bank1Source8.set()),
        }
    }

    /// Construct one epoch with only a source whose handler semantics remain
    /// unclassified, for upper-layer fail-closed host tests.
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub const fn for_unclassified_host_test() -> Self {
        Self {
            sources: Set::empty(),
            fault_sources: BluetoothPrimaryFaultSources(BluetoothPrimarySource::Unclassified.set()),
        }
    }

    /// Whether positional dynamic bank-zero source 21 was pending.
    pub const fn bank_0_source_21_pending(&self) -> bool {
        self.sources
            .includes(BluetoothPrimarySource::Bank0Source21.set())
    }

    /// Whether this acknowledged primary epoch captured a BTLC first-word wrap.
    pub const fn controller_time_overflow(&self) -> bool {
        self.sources
            .includes(BluetoothPrimarySource::ControllerTimeOverflow.set())
    }

    /// Add an overflow observation to a synthetic epoch for host tests.
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub const fn with_controller_time_overflow_for_host_test(mut self) -> Self {
        self.sources = self
            .sources
            .union(BluetoothPrimarySource::ControllerTimeOverflow.set());
        self
    }

    /// Whether positional dynamic bank-zero source 27 or 28 was pending.
    pub const fn bank_0_sources_27_or_28_pending(&self) -> bool {
        self.sources
            .includes(BluetoothPrimarySource::Bank0Sources27Or28.set())
    }

    /// Whether positional dynamic bank-one source 3 was pending.
    pub const fn bank_1_source_3_pending(&self) -> bool {
        self.sources
            .includes(BluetoothPrimarySource::Bank1Source3.set())
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
    let status = bank_0_status.union(bank_1_status);
    if status.is_empty() {
        return None;
    }
    control.acknowledge_bank_0(bank_0);
    control.acknowledge_bank_1(bank_1);

    let fault_sources = BluetoothPrimaryFaultSources::from_status(status);
    if fault_sources.bank_1_source_9_pending() {
        control.capture_diagnostic_detail_0();
        control.capture_diagnostic_detail_1();
    }
    if fault_sources.bank_1_source_12_pending() {
        control.capture_diagnostic_state();
    }

    Some(BluetoothPrimaryInterruptEpoch {
        sources: status.intersection(
            DYNAMIC_SOURCES.union(BluetoothPrimarySource::ControllerTimeOverflow.set()),
        ),
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

    /// Read both NRT cause words for a fatal diagnostic; acknowledge nothing.
    pub fn nrt_status(&self) -> [u32; 2] {
        let bank = &self.peripherals.bluetooth_interrupt_bank;
        [
            bank.read_irq_status_snapshot_0().pending_bits(),
            bank.read_irq_status_snapshot_1().pending_bits(),
        ]
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
    /// Only this transaction acknowledges. Diagnostic reads grant no
    /// acknowledgement or release authority.
    ///
    /// The bank shares its handler's line with other sources, so an entry
    /// may find both snapshots clear: it then writes nothing and returns
    /// `Ok(None)`.
    /// If the exact NRT sample includes a shared primary fault or overflow
    /// clear lane, refuse both acknowledgements and retain both observations.
    pub fn capture_nrt_and_acknowledge(
        &mut self,
    ) -> Result<Option<BluetoothNrtInterruptAcknowledged>, BluetoothNrtInterruptFault> {
        capture_nrt_and_acknowledge(&mut self.peripherals.bluetooth_interrupt_bank)
    }
}

#[cfg(test)]
mod tests;
