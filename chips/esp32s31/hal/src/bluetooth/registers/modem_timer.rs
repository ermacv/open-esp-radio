//! Exact controller transactions for the modem low-power timer.
//!
//! The module contains the one-command runtime-counter start performed by the
//! controller hardware-enable path, the complete low-power configuration
//! component, the MMIO prefix immediately before ESP32-S31 installs interrupt
//! source 127, and the bounded register classifier and hardware-acknowledgement
//! transactions at the start of that source's handler. Each method is one
//! finite transaction over the timer partition; the order in which the
//! lifecycle may call them belongs to the HAL. These transactions do not
//! initialize the vendor software environment, publish ISR storage, install a
//! CPU route, dispatch the software timer queue, or claim that the controller,
//! Link Layer, or HCI is live.

#![deny(unsafe_code)]

use crate::bluetooth::registers::{
    BluetoothModemLpTimerRegisters, BluetoothPlacements, device_fence,
};

/// Hardware branch observed while completing the BTDM low-power component.
///
/// The positional names retain only the independently reviewed register facts;
/// they do not assign an undocumented clock or sleep meaning to `CONTROL_2`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothLowPowerRuntimeControlObservation {
    /// `CONTROL_2` was clear after controls zero and four were cleared.
    Control2Clear,
    /// `CONTROL_2` was set, so the fresh-read `CONTROL_1` publication ran.
    Control2SetControl1Published,
}

/// Exact positional path by which source 127 reached its software handler.
///
/// These names deliberately preserve register positions rather than assign
/// undocumented RTC or timer semantics to the observed hardware values.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothModemLpTimerInterruptObservation {
    /// `WORD_0038_OPAQUE` was nonzero and `WORD_006C_OPAQUE` was zero.
    Value006cZero,
    /// `WORD_0038_OPAQUE` and `WORD_006C_OPAQUE` were nonzero; `WORD_0058_OPAQUE.bit2` was
    /// clear.
    Control2Clear,
    /// `WORD_0058_OPAQUE.bit2` was set and a second fresh read supplied the image
    /// in which `WORD_0058_OPAQUE.bit1` was published.
    Control2SetControl1Published,
}

/// Positional state acknowledged by the common source-127 timer handler.
///
/// The two booleans intentionally retain register positions instead of
/// assigning undocumented timer or RTC meanings. A nonzero `WORD_0024_OPAQUE` low
/// byte requires software timer-queue dispatch. A nonzero `WORD_002C_OPAQUE` low
/// byte requires a separate software environment update before the final
/// hardware read.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BluetoothModemLpTimerHandlerRegisterObservation {
    state_0024_low_byte_nonzero: bool,
    state_002c_low_byte_nonzero: bool,
}

impl BluetoothModemLpTimerHandlerRegisterObservation {
    /// Whether the sampled low byte of `WORD_0024_OPAQUE` was nonzero.
    pub const fn state_0024_low_byte_was_nonzero(self) -> bool {
        self.state_0024_low_byte_nonzero
    }

    /// Whether the sampled low byte of `WORD_002C_OPAQUE` was nonzero.
    pub const fn state_002c_low_byte_was_nonzero(self) -> bool {
        self.state_002c_low_byte_nonzero
    }
}

/// Sampled low bytes retained across preparation or shutdown.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
#[must_use = "pending observations retain state removed at prepare or shutdown"]
pub struct BluetoothModemLpTimerPendingObservation {
    state_0024: u8,
    state_002c: u8,
}

impl BluetoothModemLpTimerPendingObservation {
    /// Sampled software-dispatch byte.
    pub const fn state_0024(self) -> u8 {
        self.state_0024
    }
    /// Sampled rollover byte; no event-count interpretation is implied.
    pub const fn state_002c(self) -> u8 {
        self.state_002c
    }
}

/// Finite acknowledgement while the counter and both measured lanes are stopped.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothModemLpTimerStoppedInterruptObservation {
    /// Neither sampled pending byte requested work.
    Quiet,
    /// The sampled rollover byte was acknowledged without dispatch or rearming.
    RolloverAcknowledged(BluetoothModemLpTimerPendingObservation),
}

/// Stopped state that the gate sends through its existing fault path.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothModemLpTimerStoppedInterruptFault {
    /// Software dispatch is pending after the task drained; bytes remain intact.
    SoftwarePending {
        /// Complete cause word sampled with the pending bytes.
        status: u32,
        /// Both pending bytes before any acknowledgement.
        pending: BluetoothModemLpTimerPendingObservation,
    },
    /// A nonzero status other than the measured rollover-only image; no ack ran.
    UnexpectedStatus {
        /// Complete unrecognized cause word.
        status: u32,
        /// Both pending bytes sampled before any acknowledgement.
        pending: BluetoothModemLpTimerPendingObservation,
    },
}

/// Source-owned high byte paired with the modem LP timer's positional counter.
///
/// The vendor environment advances this byte with wrapping arithmetic whenever
/// the reviewed rollover state is acknowledged. No physical time unit is
/// assigned here.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct BluetoothModemLpTimerEpoch(u8);

impl BluetoothModemLpTimerEpoch {
    /// Begin the source-owned timer environment at epoch zero.
    pub const fn new() -> Self {
        Self(0)
    }

    /// Return the current positional high byte.
    pub const fn high_byte(self) -> u8 {
        self.0
    }

    /// Apply the rollover fact already acknowledged by the common handler's
    /// register phase.
    pub fn advance_for_handler_registers(
        &mut self,
        observation: BluetoothModemLpTimerHandlerRegisterObservation,
    ) {
        if observation.state_002c_low_byte_was_nonzero() {
            self.advance();
        }
    }

    fn advance(&mut self) {
        self.0 = self.0.wrapping_add(1);
    }

    const fn advanced(mut self) -> Self {
        self.0 = self.0.wrapping_add(1);
        self
    }

    const fn combine(self, counter_image: u32) -> BluetoothModemLpTimerInstant {
        BluetoothModemLpTimerInstant(((self.0 as u32) << 24) | counter_image)
    }
}

/// One positional 32-bit modem LP-timer instant.
///
/// The value wraps and deliberately has no claimed physical unit.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BluetoothModemLpTimerInstant(u32);

impl BluetoothModemLpTimerInstant {
    /// Retain one complete positional deadline image.
    pub const fn from_bits(bits: u32) -> Self {
        Self(bits)
    }

    /// Return the complete positional image.
    pub const fn bits(self) -> u32 {
        self.0
    }
}

/// Result of one finite LP-counter sample.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BluetoothModemLpTimerCounterObservation {
    instant: BluetoothModemLpTimerInstant,
    rollover_acknowledged: bool,
}

impl BluetoothModemLpTimerCounterObservation {
    /// Return the sampled positional instant after any epoch advance.
    pub const fn instant(self) -> BluetoothModemLpTimerInstant {
        self.instant
    }

    /// Whether this sample acknowledged a newly observed rollover state.
    pub const fn rollover_was_acknowledged(self) -> bool {
        self.rollover_acknowledged
    }
}

/// Hardware branch selected while programming one positional compare.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothModemLpTimerCompareDisposition {
    /// The deadline delta was at most two and software work was requested now.
    Immediate,
    /// The deadline lies within one low-counter span and its low image was used.
    Deadline,
    /// A half-range checkpoint was used for a more distant deadline.
    HalfRangeCheckpoint,
}

trait ModemLpTimerTransaction {
    /// Initialize controls and disable delivery without clearing pending.
    fn prepare_controls(&mut self);
    fn sample_pending(&mut self) -> BluetoothModemLpTimerPendingObservation;
    /// Apply the vendor all-ones +0x34 command to clear the two tested causes.
    fn clear_pending_with_command(&mut self);
    fn enable_rollover(&mut self);
    fn fence(&mut self);
}

trait ModemLpTimerStopTransaction {
    fn disable_delivery(&mut self);
    fn stop_counter(&mut self);
    fn sample_pending(&mut self) -> BluetoothModemLpTimerPendingObservation;
    fn read_status(&mut self) -> u32;
    fn clear_rollover(&mut self);
    fn clear_pending(&mut self);
    fn fence(&mut self);
}

fn execute_modem_lp_timer_stop(
    transaction: &mut impl ModemLpTimerStopTransaction,
) -> BluetoothModemLpTimerPendingObservation {
    transaction.disable_delivery();
    transaction.stop_counter();
    transaction.fence();
    let pending = transaction.sample_pending();
    // SOURCE: HW6_S31_LP_QUIET_20261026, hw6-20261026-reset-02,
    // tags 0x3260/0x3380-0x33e2 (oer-notes/research/hw-ble-lp-reset.md).
    // Controller reset does not clear these bytes. Preserve their observation
    // before the measured whole-word zeros make the stopped timer quiet.
    transaction.clear_pending();
    transaction.fence();
    pending
}

fn execute_modem_lp_timer_stopped_interrupt(
    transaction: &mut impl ModemLpTimerStopTransaction,
    epoch: &mut BluetoothModemLpTimerEpoch,
) -> Result<
    BluetoothModemLpTimerStoppedInterruptObservation,
    BluetoothModemLpTimerStoppedInterruptFault,
> {
    use oer_esp32s31_pac::modem_pwr::word_0038_opaque::Bits0_31Opaque;
    let status = transaction.read_status();
    let pending = transaction.sample_pending();
    if pending.state_0024() != 0 {
        return Err(
            BluetoothModemLpTimerStoppedInterruptFault::SoftwarePending { status, pending },
        );
    }
    if status != Bits0_31Opaque::Inactive.bits() && status != Bits0_31Opaque::RolloverPending.bits()
    {
        return Err(
            BluetoothModemLpTimerStoppedInterruptFault::UnexpectedStatus { status, pending },
        );
    }
    if pending.state_002c() != 0 {
        transaction.clear_rollover();
        epoch.advance();
        transaction.fence();
        Ok(BluetoothModemLpTimerStoppedInterruptObservation::RolloverAcknowledged(pending))
    } else {
        Ok(BluetoothModemLpTimerStoppedInterruptObservation::Quiet)
    }
}

trait ModemLpTimerLowPowerInitTransaction {
    fn initialize_config(&mut self);
    fn clear_controls_0_4(&mut self);
    fn control_2_is_set(&mut self) -> bool;
    fn publish_control_1(&mut self);
    fn fence(&mut self);
}

trait ModemLpTimerStartTransaction {
    fn start_counter(&mut self);
    fn fence(&mut self);
}

trait ModemLpTimerInterruptTransaction {
    fn read_status_0038(&mut self) -> u32;
    fn read_value_006c(&mut self) -> u32;
    fn control_0058_bit_2_is_set(&mut self) -> bool;
    fn set_control_0058_bit_1_from_fresh_read(&mut self);
    fn fence(&mut self);
}

trait ModemLpTimerHandlerRegisterTransaction {
    fn sample_state_0024_low_byte_nonzero(&mut self) -> bool;
    fn clear_state_0024(&mut self);
    fn sample_state_002c_low_byte_nonzero(&mut self) -> bool;
    fn clear_state_002c(&mut self);
    fn sample_final_state_0024(&mut self);
    fn fence(&mut self);
}

trait ModemLpTimerRuntimeTransaction {
    fn read_counter(&mut self) -> u32;
    fn rollover_low_byte_nonzero(&mut self) -> bool;
    fn clear_rollover(&mut self);
    fn publish_software_pending(&mut self);
    fn disable_compare(&mut self);
    /// Store one compare image, produced only by the deadline projection of
    /// `execute_modem_lp_timer_compare_program`.
    fn write_compare(&mut self, image: u32);
    fn trigger_timer_command(&mut self);
    fn fence(&mut self);
}

const MODEM_LP_TIMER_LOW_SPAN: u32 =
    1 << oer_esp32s31_pac::modem_pwr::word_0048_opaque::COUNTER_LOW_WIDTH;
const MODEM_LP_TIMER_HALF_RANGE: u32 = MODEM_LP_TIMER_LOW_SPAN / 2;
const MODEM_LP_TIMER_IMMEDIATE_TOLERANCE: i32 = 2;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ModemLpTimerInterruptDisposition {
    Spurious,
    HandlerPending(BluetoothModemLpTimerInterruptObservation),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ModemLpTimerHandlerRegisterDisposition {
    Rearmed,
    SoftwarePending(BluetoothModemLpTimerHandlerRegisterObservation),
}

fn execute_modem_lp_timer_interrupt(
    transaction: &mut impl ModemLpTimerInterruptTransaction,
) -> ModemLpTimerInterruptDisposition {
    if transaction.read_status_0038() == 0 {
        return ModemLpTimerInterruptDisposition::Spurious;
    }

    let observation = if transaction.read_value_006c() == 0 {
        BluetoothModemLpTimerInterruptObservation::Value006cZero
    } else if !transaction.control_0058_bit_2_is_set() {
        BluetoothModemLpTimerInterruptObservation::Control2Clear
    } else {
        transaction.set_control_0058_bit_1_from_fresh_read();
        BluetoothModemLpTimerInterruptObservation::Control2SetControl1Published
    };
    transaction.fence();
    ModemLpTimerInterruptDisposition::HandlerPending(observation)
}

fn execute_modem_lp_timer_handler_registers(
    transaction: &mut impl ModemLpTimerHandlerRegisterTransaction,
) -> ModemLpTimerHandlerRegisterDisposition {
    let state_0024_low_byte_nonzero = transaction.sample_state_0024_low_byte_nonzero();
    if state_0024_low_byte_nonzero {
        transaction.clear_state_0024();
    }

    let state_002c_low_byte_nonzero = transaction.sample_state_002c_low_byte_nonzero();
    if state_002c_low_byte_nonzero {
        transaction.clear_state_002c();
    }

    let observation = BluetoothModemLpTimerHandlerRegisterObservation {
        state_0024_low_byte_nonzero,
        state_002c_low_byte_nonzero,
    };
    if state_0024_low_byte_nonzero || state_002c_low_byte_nonzero {
        transaction.fence();
        ModemLpTimerHandlerRegisterDisposition::SoftwarePending(observation)
    } else {
        transaction.sample_final_state_0024();
        ModemLpTimerHandlerRegisterDisposition::Rearmed
    }
}

fn execute_modem_lp_timer_counter_sample(
    transaction: &mut impl ModemLpTimerRuntimeTransaction,
    epoch: &mut BluetoothModemLpTimerEpoch,
) -> BluetoothModemLpTimerCounterObservation {
    let first_counter = transaction.read_counter();
    let rollover_acknowledged = transaction.rollover_low_byte_nonzero();
    let counter = if rollover_acknowledged {
        epoch.advance();
        let fresh_counter = transaction.read_counter();
        transaction.clear_rollover();
        transaction.publish_software_pending();
        transaction.trigger_timer_command();
        transaction.fence();
        fresh_counter
    } else {
        first_counter
    };

    BluetoothModemLpTimerCounterObservation {
        instant: epoch.combine(counter),
        rollover_acknowledged,
    }
}

fn execute_modem_lp_timer_compare_disable(transaction: &mut impl ModemLpTimerRuntimeTransaction) {
    transaction.disable_compare();
    transaction.fence();
}

fn execute_modem_lp_timer_compare_program(
    transaction: &mut impl ModemLpTimerRuntimeTransaction,
    deadline: BluetoothModemLpTimerInstant,
    epoch: BluetoothModemLpTimerEpoch,
) -> BluetoothModemLpTimerCompareDisposition {
    transaction.disable_compare();
    let first_counter = transaction.read_counter();
    let rollover = transaction.rollover_low_byte_nonzero();
    let (effective_epoch, counter) = if rollover {
        (epoch.advanced(), transaction.read_counter())
    } else {
        (epoch, first_counter)
    };
    let current = effective_epoch.combine(counter).bits();
    let delta = deadline.bits().wrapping_sub(current);

    let disposition = if (delta as i32) <= MODEM_LP_TIMER_IMMEDIATE_TOLERANCE {
        transaction.publish_software_pending();
        BluetoothModemLpTimerCompareDisposition::Immediate
    } else if delta < MODEM_LP_TIMER_LOW_SPAN {
        transaction.write_compare(deadline.bits() % MODEM_LP_TIMER_LOW_SPAN);
        BluetoothModemLpTimerCompareDisposition::Deadline
    } else {
        transaction.write_compare(counter.wrapping_add(MODEM_LP_TIMER_HALF_RANGE));
        BluetoothModemLpTimerCompareDisposition::HalfRangeCheckpoint
    };
    transaction.trigger_timer_command();
    transaction.fence();
    disposition
}

fn execute_modem_lp_timer_prepare(
    transaction: &mut impl ModemLpTimerTransaction,
) -> BluetoothModemLpTimerPendingObservation {
    transaction.prepare_controls();
    let stale = transaction.sample_pending();
    // SOURCE: HW9_S31_LP_CLEAR34_20261027, hw9-20261027-lp-06,
    // lp-clear.json, tags 0x9600-0x9608 and 0x9640-0x9650
    // (oer-notes/research/hw-ble-irq-wave2.md). The vendor's whole-word
    // all-ones +0x34 clears both tested pending bytes. Sample before that
    // command so stale state survives in the returned observation; clear
    // before arming rollover to avoid delivery into a vacant gate.
    transaction.clear_pending_with_command();
    transaction.enable_rollover();
    transaction.fence();
    stale
}

fn execute_modem_lp_timer_low_power_init(
    transaction: &mut impl ModemLpTimerLowPowerInitTransaction,
) -> BluetoothLowPowerRuntimeControlObservation {
    transaction.initialize_config();
    transaction.clear_controls_0_4();
    let observation = if transaction.control_2_is_set() {
        transaction.publish_control_1();
        BluetoothLowPowerRuntimeControlObservation::Control2SetControl1Published
    } else {
        BluetoothLowPowerRuntimeControlObservation::Control2Clear
    };
    transaction.fence();
    observation
}

fn execute_modem_lp_timer_start(transaction: &mut impl ModemLpTimerStartTransaction) {
    transaction.start_counter();
    transaction.fence();
}

struct HardwareModemLpTimerTransaction<'registers, B: oer_mmio::Placement> {
    registers: &'registers mut oer_esp32s31_pac::modem_pwr::BtdmRuntimeControl<B>,
    status: oer_esp32s31_pac::modem_pwr::BtdmLpTimerStatus<B>,
}

struct HardwareModemLpTimerLowPowerInitTransaction<'registers, E, T>
where
    E: oer_mmio::Placement,
    T: oer_mmio::Placement,
{
    config: &'registers mut oer_esp32s31_pac::modem_etm::ModemEtmBluetooth<E>,
    runtime_control: &'registers mut oer_esp32s31_pac::modem_pwr::BtdmRuntimeControl<T>,
}

impl<E, T> ModemLpTimerLowPowerInitTransaction
    for HardwareModemLpTimerLowPowerInitTransaction<'_, E, T>
where
    E: oer_mmio::Placement,
    T: oer_mmio::Placement,
{
    fn initialize_config(&mut self) {
        let etm = &mut *self.config;
        etm.trigger_channel_enable_clear_ch5();
        etm.modify_ch5_event(|w| {
            w.set_id(oer_esp32s31_pac::modem_etm::ch_event::Id::BtdmLowPowerEvent43.bits());
        });
        etm.modify_ch5_task(|w| {
            w.set_id(oer_esp32s31_pac::modem_etm::ch_task::Id::BtdmLowPowerTask12.bits());
        });
        etm.trigger_channel_enable_clear_ch4();
        etm.modify_ch4_event(|w| {
            w.set_id(oer_esp32s31_pac::modem_etm::ch_event::Id::BtdmLowPowerEvent45.bits());
        });
        etm.modify_ch4_task(|w| {
            w.set_id(oer_esp32s31_pac::modem_etm::ch_task::Id::BtdmLowPowerTask11.bits());
        });
        etm.trigger_channel_enable_clear_ch6();
        etm.modify_ch6_event(|w| {
            w.set_id(oer_esp32s31_pac::modem_etm::ch_event::Id::BtdmLowPowerEvent46.bits());
        });
        etm.modify_ch6_task(|w| {
            w.set_id(oer_esp32s31_pac::modem_etm::ch_task::Id::BtdmLowPowerTask11.bits());
        });
        etm.trigger_channel_enable_clear_ch7();
        etm.modify_ch7_event(|w| {
            w.set_id(oer_esp32s31_pac::modem_etm::ch_event::Id::BtdmLowPowerEvent46.bits());
        });
        etm.modify_ch7_task(|w| {
            w.set_id(oer_esp32s31_pac::modem_etm::ch_task::Id::BtdmLowPowerTask0f.bits());
        });
    }

    fn clear_controls_0_4(&mut self) {
        self.runtime_control.modify_word_0058_opaque(|w| {
            w.set_control_0(false);
            w.set_control_4(false);
        });
    }

    fn control_2_is_set(&mut self) -> bool {
        self.runtime_control.read_word_0058_opaque().control_2()
    }

    fn publish_control_1(&mut self) {
        self.runtime_control.trigger_word_0058_opaque_control_1();
    }

    fn fence(&mut self) {
        device_fence();
    }
}

impl<B: oer_mmio::Placement> ModemLpTimerStartTransaction
    for HardwareModemLpTimerTransaction<'_, B>
{
    fn start_counter(&mut self) {
        self.registers.write_runtime_timer_start_command(
            oer_esp32s31_pac::modem_pwr::runtime_timer_start_command::Command::Start,
            |_| {},
        );
    }

    fn fence(&mut self) {
        device_fence();
    }
}

impl<B: oer_mmio::Placement> ModemLpTimerTransaction for HardwareModemLpTimerTransaction<'_, B> {
    fn prepare_controls(&mut self) {
        self.registers.write_word_004c_opaque(|w| {
            w.set_bits_0_31_opaque(0);
        });
        self.registers.modify_word_0078_opaque(|w| {
            w.set_control_25(true);
        });
        self.registers
            .prepare_bluetooth_modem_lp_timer_command_0004();
        self.registers
            .prepare_bluetooth_modem_lp_timer_command_0008();
        self.registers
            .prepare_bluetooth_modem_lp_timer_command_0014();
    }

    fn sample_pending(&mut self) -> BluetoothModemLpTimerPendingObservation {
        BluetoothModemLpTimerPendingObservation {
            state_0024: self.registers.read_word_0024_opaque().low_byte(),
            state_002c: self.registers.read_word_002c_opaque().low_byte(),
        }
    }

    fn clear_pending_with_command(&mut self) {
        self.registers
            .prepare_bluetooth_modem_lp_timer_command_0034();
    }

    fn enable_rollover(&mut self) {
        self.registers.write_word_0010_opaque(|w| {
            w.set_bit_1_opaque(true);
        });
    }

    fn fence(&mut self) {
        device_fence();
    }
}

impl<B: oer_mmio::Placement> ModemLpTimerStopTransaction
    for HardwareModemLpTimerTransaction<'_, B>
{
    fn disable_delivery(&mut self) {
        self.registers
            .prepare_bluetooth_modem_lp_timer_command_0014();
    }
    fn stop_counter(&mut self) {
        self.registers
            .prepare_bluetooth_modem_lp_timer_command_0004();
    }
    fn sample_pending(&mut self) -> BluetoothModemLpTimerPendingObservation {
        ModemLpTimerTransaction::sample_pending(self)
    }
    fn read_status(&mut self) -> u32 {
        self.status.read_word_0038_opaque().bits_0_31_opaque()
    }
    fn clear_rollover(&mut self) {
        self.registers.clear_bluetooth_modem_lp_timer_state_002c();
    }
    fn clear_pending(&mut self) {
        self.registers.clear_bluetooth_modem_lp_timer_state_0024();
        self.registers.clear_bluetooth_modem_lp_timer_state_002c();
    }
    fn fence(&mut self) {
        device_fence();
    }
}

impl<B: oer_mmio::Placement> ModemLpTimerInterruptTransaction
    for HardwareModemLpTimerTransaction<'_, B>
{
    fn read_status_0038(&mut self) -> u32 {
        self.status.read_word_0038_opaque().bits_0_31_opaque()
    }

    fn read_value_006c(&mut self) -> u32 {
        self.registers.read_word_006c_opaque().bits_0_31_opaque()
    }

    fn control_0058_bit_2_is_set(&mut self) -> bool {
        self.registers.read_word_0058_opaque().control_2()
    }

    fn set_control_0058_bit_1_from_fresh_read(&mut self) {
        self.registers.trigger_word_0058_opaque_control_1();
    }

    fn fence(&mut self) {
        device_fence();
    }
}

impl<B: oer_mmio::Placement> ModemLpTimerHandlerRegisterTransaction
    for HardwareModemLpTimerTransaction<'_, B>
{
    fn sample_state_0024_low_byte_nonzero(&mut self) -> bool {
        self.registers.read_word_0024_opaque().low_byte() != 0
    }

    fn clear_state_0024(&mut self) {
        self.registers.clear_bluetooth_modem_lp_timer_state_0024();
    }

    fn sample_state_002c_low_byte_nonzero(&mut self) -> bool {
        self.registers.read_word_002c_opaque().low_byte() != 0
    }

    fn clear_state_002c(&mut self) {
        self.registers.clear_bluetooth_modem_lp_timer_state_002c();
    }

    fn sample_final_state_0024(&mut self) {
        let _ = self.registers.read_word_0024_opaque().low_byte();
    }

    fn fence(&mut self) {
        device_fence();
    }
}

impl<B: oer_mmio::Placement> ModemLpTimerRuntimeTransaction
    for HardwareModemLpTimerTransaction<'_, B>
{
    fn read_counter(&mut self) -> u32 {
        self.registers.read_word_0048_opaque().counter_low().get()
    }

    fn rollover_low_byte_nonzero(&mut self) -> bool {
        self.registers.read_word_002c_opaque().low_byte() != 0
    }

    fn clear_rollover(&mut self) {
        self.registers.clear_bluetooth_modem_lp_timer_state_002c();
    }

    fn publish_software_pending(&mut self) {
        self.registers
            .publish_bluetooth_modem_lp_timer_software_pending();
    }

    fn disable_compare(&mut self) {
        self.registers.disable_bluetooth_modem_lp_timer_compare();
    }

    fn write_compare(&mut self, image: u32) {
        self.registers.modify_word_006c_opaque(|w| {
            w.set_bits_0_31_opaque(image);
        });
    }

    fn trigger_timer_command(&mut self) {
        self.registers.write_word_0010_opaque(|w| {
            w.set_bit_18_opaque(true);
        });
    }

    fn fence(&mut self) {
        device_fence();
    }
}

impl<P: BluetoothPlacements> BluetoothModemLpTimerRegisters<P> {
    /// Apply the exact controller-register prefix before source 127 is routed.
    ///
    /// SOURCE: public ESP32-S31 `libbtdm_common.a` member `9.o`, complete
    /// symbol `r_sym_bt_waDX0omCE7oLuPVSoPOK`, role-mapped by the public
    /// same-chip unobfuscated `r_btdm_hal_rtc_init`. The body performs seven
    /// writes and one fresh read. Both pending bytes are sampled before
    /// the whole-word all-ones +0x34 command clears the two tested causes,
    /// then the final rollover command arms delivery. The returned
    /// observation exposes the removed stale state to the caller.
    /// SOURCE: HW9_S31_LP_CLEAR34_20261027, hw9-20261027-lp-06,
    /// lp-clear.json 0x9600-0x9608 and 0x9640-0x9650
    /// (oer-notes/research/hw-ble-irq-wave2.md). No per-bit or all-source
    /// clearing semantics are implied.
    pub fn prepare_registers(&mut self) -> BluetoothModemLpTimerPendingObservation {
        let mut transaction = HardwareModemLpTimerTransaction {
            registers: &mut self.runtime_control,
            status: self.status,
        };
        execute_modem_lp_timer_prepare(&mut transaction)
    }

    /// Disable both measured delivery lanes, stop the counter and fence,
    /// sample pending, then clear both pending words and fence again. The
    /// returned observation retains the removed pending bytes. No exhaustive
    /// all-source or concurrent-arrival proof is implied.
    /// SOURCE: HW6_S31_LP_QUIET_20261026, hw6-20261026-reset-02,
    /// tags 0x3260/0x3380-0x33e2 (oer-notes/research/hw-ble-lp-reset.md).
    pub fn stop_for_shutdown(&mut self) -> BluetoothModemLpTimerPendingObservation {
        execute_modem_lp_timer_stop(&mut HardwareModemLpTimerTransaction {
            registers: &mut self.runtime_control,
            status: self.status,
        })
    }

    /// Acknowledge only rollover while stopped, with no compare command or
    /// software-pending publication. Invalid state remains unacknowledged.
    pub fn acknowledge_stopped_interrupt(
        &mut self,
        epoch: &mut BluetoothModemLpTimerEpoch,
    ) -> Result<
        BluetoothModemLpTimerStoppedInterruptObservation,
        BluetoothModemLpTimerStoppedInterruptFault,
    > {
        execute_modem_lp_timer_stopped_interrupt(
            &mut HardwareModemLpTimerTransaction {
                registers: &mut self.runtime_control,
                status: self.status,
            },
            epoch,
        )
    }

    /// Complete the reviewed BTDM low-power hardware component.
    ///
    /// SOURCE: complete public ESP32-S31 `libbtdm_common.a` member `20.o`
    /// symbol `r_sym_bt_JHP69cMcA5vCzdaxdFgT` proves the generated twelve-write
    /// sequence that disables and reprograms modem ETM channels four through
    /// seven. Its complete caller
    /// `r_sym_bt_cgaCegmpqnbaoszyOy3c` then clears `CONTROL_0` and `CONTROL_4`,
    /// samples `CONTROL_2`, and conditionally publishes `CONTROL_1` from a
    /// fresh read. A device fence closes this register transaction.
    ///
    /// This partition supplies `BTDM_RUNTIME_CONTROL` and the Bluetooth
    /// channels four through seven of `MODEM_ETM`.
    pub fn initialize_low_power_hardware(&mut self) -> BluetoothLowPowerRuntimeControlObservation {
        let mut transaction = HardwareModemLpTimerLowPowerInitTransaction {
            config: &mut self.etm,
            runtime_control: &mut self.runtime_control,
        };
        execute_modem_lp_timer_low_power_init(&mut transaction)
    }

    /// Start the BTDM runtime timer during controller hardware enable.
    ///
    /// SOURCE: complete current `libbtdm_common.a` member `9.o` symbol
    /// `r_sym_bt_ymLPVGRY14FVW494j9ZD` writes the sole reviewed command and
    /// returns. The instruction-identical public same-chip predecessor names
    /// the operation `r_btdm_hal_rtc_start`.
    pub fn start_runtime_counter(&mut self) {
        let mut transaction = HardwareModemLpTimerTransaction {
            registers: &mut self.runtime_control,
            status: self.status,
        };
        execute_modem_lp_timer_start(&mut transaction);
    }

    /// Execute exactly one source-127 register-classification prefix.
    ///
    /// The method never waits, loops, allocates or calls software. A zero
    /// `WORD_0038_OPAQUE` returns `None` after one read. Every other branch is
    /// fenced and returns the path that requires the common timer handler.
    pub fn classify_interrupt(&mut self) -> Option<BluetoothModemLpTimerInterruptObservation> {
        let mut transaction = HardwareModemLpTimerTransaction {
            registers: &mut self.runtime_control,
            status: self.status,
        };
        match execute_modem_lp_timer_interrupt(&mut transaction) {
            ModemLpTimerInterruptDisposition::Spurious => None,
            ModemLpTimerInterruptDisposition::HandlerPending(observation) => Some(observation),
        }
    }

    /// Sample the two positional state bytes at the start of the common
    /// timer handler and acknowledge each nonzero byte with a whole-word zero.
    ///
    /// SOURCE: BLOB_LIBBTDM_LP_PENDING_WORDS_CURRENT (L6/L7). The reviewed
    /// stores also clear the unmodeled upper bytes; no W1C or output-low
    /// semantics are inferred.
    ///
    /// This finite transaction never waits, loops, allocates or invokes
    /// software. If neither byte requests work, the vendor path performs its
    /// final fresh read and this returns `None`. Otherwise it returns the
    /// state bytes whose software consequences remain pending.
    pub fn acknowledge_handler_registers(
        &mut self,
    ) -> Option<BluetoothModemLpTimerHandlerRegisterObservation> {
        let mut transaction = HardwareModemLpTimerTransaction {
            registers: &mut self.runtime_control,
            status: self.status,
        };
        match execute_modem_lp_timer_handler_registers(&mut transaction) {
            ModemLpTimerHandlerRegisterDisposition::Rearmed => None,
            ModemLpTimerHandlerRegisterDisposition::SoftwarePending(observation) => {
                Some(observation)
            }
        }
    }

    /// Acknowledge a handler epoch and sample time before publishing value-only
    /// queue work. The handler retains both the registers and the actual epoch.
    pub fn take_software_work(
        &mut self,
        epoch: &mut BluetoothModemLpTimerEpoch,
    ) -> Option<crate::bluetooth::ModemLpTimerWork> {
        let observation = self.acknowledge_handler_registers()?;
        epoch.advance_for_handler_registers(observation);
        let now = self.sample_counter(epoch).instant();
        Some(crate::bluetooth::ModemLpTimerWork {
            now,
            epoch: *epoch,
            dispatch_requested: observation.state_0024_low_byte_was_nonzero(),
        })
    }

    /// Acknowledge a new entry while the task holds the earlier snapshot.
    /// Keep its software facts with the handler, without replacing that work.
    pub fn retain_interrupt(
        &mut self,
        epoch: &mut BluetoothModemLpTimerEpoch,
        deferred_dispatch: &mut Option<bool>,
    ) {
        if self.classify_interrupt().is_none() {
            return;
        }
        if let Some(observation) = self.acknowledge_handler_registers() {
            epoch.advance_for_handler_registers(observation);
            *deferred_dispatch = Some(
                deferred_dispatch.unwrap_or(false) || observation.state_0024_low_byte_was_nonzero(),
            );
        }
    }

    /// Apply the software decision, then publish any software work retained
    /// by intervening handler entries. Newly arriving state bytes remain for
    /// the next register phase; the compare sample never clears them.
    pub fn apply_plan(
        &mut self,
        plan: crate::bluetooth::ModemLpTimerPlan,
        epoch: &mut BluetoothModemLpTimerEpoch,
        deferred_dispatch: Option<bool>,
    ) -> Option<crate::bluetooth::ModemLpTimerWork> {
        match plan.deadline {
            Some(deadline) => {
                let _ = self.program_compare(deadline, *epoch);
            }
            None => self.disable_compare(),
        }
        self.sample_final_state();
        deferred_dispatch.map(|dispatch_requested| crate::bluetooth::ModemLpTimerWork {
            now: self.sample_counter(epoch).instant(),
            epoch: *epoch,
            dispatch_requested,
        })
    }

    /// Sample one complete positional LP-timer instant.
    ///
    /// A newly observed rollover advances `epoch` before the fresh counter
    /// read and publishes the exact hardware acknowledgement. The method is
    /// finite and returns without polling.
    pub fn sample_counter(
        &mut self,
        epoch: &mut BluetoothModemLpTimerEpoch,
    ) -> BluetoothModemLpTimerCounterObservation {
        let mut transaction = HardwareModemLpTimerTransaction {
            registers: &mut self.runtime_control,
            status: self.status,
        };
        execute_modem_lp_timer_counter_sample(&mut transaction, epoch)
    }

    /// Disable the currently programmed positional compare.
    pub fn disable_compare(&mut self) {
        let mut transaction = HardwareModemLpTimerTransaction {
            registers: &mut self.runtime_control,
            status: self.status,
        };
        execute_modem_lp_timer_compare_disable(&mut transaction);
    }

    /// Program one positional deadline using the current software epoch.
    ///
    /// The exact finite transaction either requests immediate software work,
    /// publishes the deadline low counter image, or places a half-range
    /// checkpoint when the deadline is at least one full low-counter span
    /// away. It never waits for the compare to fire.
    pub fn program_compare(
        &mut self,
        deadline: BluetoothModemLpTimerInstant,
        epoch: BluetoothModemLpTimerEpoch,
    ) -> BluetoothModemLpTimerCompareDisposition {
        let mut transaction = HardwareModemLpTimerTransaction {
            registers: &mut self.runtime_control,
            status: self.status,
        };
        execute_modem_lp_timer_compare_program(&mut transaction, deadline, epoch)
    }

    /// Perform the common handler's final fresh state read after software work.
    pub fn sample_final_state(&mut self) {
        let mut transaction = HardwareModemLpTimerTransaction {
            registers: &mut self.runtime_control,
            status: self.status,
        };
        transaction.sample_final_state_0024();
    }
}

#[cfg(test)]
mod tests;
