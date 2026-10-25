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
    fn prepare_hardware(&mut self);
    fn fence(&mut self);
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

const MODEM_LP_TIMER_LOW_SPAN: u32 = 0x0100_0000;
const MODEM_LP_TIMER_HALF_RANGE: u32 = 0x0080_0000;
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

fn execute_modem_lp_timer_prepare(transaction: &mut impl ModemLpTimerTransaction) {
    transaction.prepare_hardware();
    transaction.fence();
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

/// The modem event and task identities the BTDM low-power component routes
/// on modem ETM channels four through seven, each channel disabled first.
///
/// SOURCE: BLOB_LIBBTDM_RUNTIME_CONTROL.
const LOW_POWER_ETM_CH5_EVENT: u32 = 0x43;
/// SOURCE: BLOB_LIBBTDM_RUNTIME_CONTROL.
const LOW_POWER_ETM_CH5_TASK: u32 = 0x12;
/// SOURCE: BLOB_LIBBTDM_RUNTIME_CONTROL.
const LOW_POWER_ETM_CH4_EVENT: u32 = 0x45;
/// SOURCE: BLOB_LIBBTDM_RUNTIME_CONTROL.
const LOW_POWER_ETM_CH4_TASK: u32 = 0x11;
/// SOURCE: BLOB_LIBBTDM_RUNTIME_CONTROL.
const LOW_POWER_ETM_CH6_EVENT: u32 = 0x46;
/// SOURCE: BLOB_LIBBTDM_RUNTIME_CONTROL.
const LOW_POWER_ETM_CH6_TASK: u32 = 0x11;
/// SOURCE: BLOB_LIBBTDM_RUNTIME_CONTROL.
const LOW_POWER_ETM_CH7_EVENT: u32 = 0x46;
/// SOURCE: BLOB_LIBBTDM_RUNTIME_CONTROL.
const LOW_POWER_ETM_CH7_TASK: u32 = 0x0f;

struct HardwareModemLpTimerTransaction<'registers, B: oer_mmio::Placement> {
    registers: &'registers mut oer_esp32s31_pac::modem_pwr::BtdmRuntimeControl<B>,
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
            w.set_id(LOW_POWER_ETM_CH5_EVENT);
        });
        etm.modify_ch5_task(|w| {
            w.set_id(LOW_POWER_ETM_CH5_TASK);
        });
        etm.trigger_channel_enable_clear_ch4();
        etm.modify_ch4_event(|w| {
            w.set_id(LOW_POWER_ETM_CH4_EVENT);
        });
        etm.modify_ch4_task(|w| {
            w.set_id(LOW_POWER_ETM_CH4_TASK);
        });
        etm.trigger_channel_enable_clear_ch6();
        etm.modify_ch6_event(|w| {
            w.set_id(LOW_POWER_ETM_CH6_EVENT);
        });
        etm.modify_ch6_task(|w| {
            w.set_id(LOW_POWER_ETM_CH6_TASK);
        });
        etm.trigger_channel_enable_clear_ch7();
        etm.modify_ch7_event(|w| {
            w.set_id(LOW_POWER_ETM_CH7_EVENT);
        });
        etm.modify_ch7_task(|w| {
            w.set_id(LOW_POWER_ETM_CH7_TASK);
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
        self.runtime_control.modify_word_0058_opaque(|w| {
            w.set_control_1(true);
        });
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
    fn prepare_hardware(&mut self) {
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
        self.registers
            .prepare_bluetooth_modem_lp_timer_command_0034();
        self.registers.write_word_0010_opaque(|w| {
            w.set_bit_1_opaque(true);
        });
    }

    fn fence(&mut self) {
        device_fence();
    }
}

impl<B: oer_mmio::Placement> ModemLpTimerInterruptTransaction
    for HardwareModemLpTimerTransaction<'_, B>
{
    fn read_status_0038(&mut self) -> u32 {
        self.registers.read_word_0038_opaque().bits_0_31_opaque()
    }

    fn read_value_006c(&mut self) -> u32 {
        self.registers.read_word_006c_opaque().bits_0_31_opaque()
    }

    fn control_0058_bit_2_is_set(&mut self) -> bool {
        self.registers.read_word_0058_opaque().control_2()
    }

    fn set_control_0058_bit_1_from_fresh_read(&mut self) {
        self.registers.modify_word_0058_opaque(|w| {
            w.set_control_1(true);
        });
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
        self.registers.modify_word_0024_opaque(|w| {
            w.set_low_byte(0);
        });
    }

    fn sample_state_002c_low_byte_nonzero(&mut self) -> bool {
        self.registers.read_word_002c_opaque().low_byte() != 0
    }

    fn clear_state_002c(&mut self) {
        self.registers.modify_word_002c_opaque(|w| {
            w.set_low_byte(0);
        });
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
        self.registers.read_word_0048_opaque().bits_0_31_opaque()
    }

    fn rollover_low_byte_nonzero(&mut self) -> bool {
        self.registers.read_word_002c_opaque().low_byte() != 0
    }

    fn clear_rollover(&mut self) {
        self.registers.modify_word_002c_opaque(|w| {
            w.set_low_byte(0);
        });
    }

    fn publish_software_pending(&mut self) {
        self.registers.modify_word_0024_opaque(|w| {
            w.set_low_byte(1);
        });
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
    /// writes and one fresh read in the exact order encoded by this module.
    /// A single device fence follows the last write. The reviewed source-127
    /// teardown does not reverse these eight operations.
    pub fn prepare_registers(&mut self) {
        let mut transaction = HardwareModemLpTimerTransaction {
            registers: &mut self.runtime_control,
        };
        execute_modem_lp_timer_prepare(&mut transaction);
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
        };
        match execute_modem_lp_timer_interrupt(&mut transaction) {
            ModemLpTimerInterruptDisposition::Spurious => None,
            ModemLpTimerInterruptDisposition::HandlerPending(observation) => Some(observation),
        }
    }

    /// Acknowledge the two positional state bytes at the start of the common
    /// timer handler.
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
        };
        match execute_modem_lp_timer_handler_registers(&mut transaction) {
            ModemLpTimerHandlerRegisterDisposition::Rearmed => None,
            ModemLpTimerHandlerRegisterDisposition::SoftwarePending(observation) => {
                Some(observation)
            }
        }
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
        };
        execute_modem_lp_timer_counter_sample(&mut transaction, epoch)
    }

    /// Disable the currently programmed positional compare.
    pub fn disable_compare(&mut self) {
        let mut transaction = HardwareModemLpTimerTransaction {
            registers: &mut self.runtime_control,
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
        };
        execute_modem_lp_timer_compare_program(&mut transaction, deadline, epoch)
    }

    /// Perform the common handler's final fresh state read after software work.
    pub fn sample_final_state(&mut self) {
        let mut transaction = HardwareModemLpTimerTransaction {
            registers: &mut self.runtime_control,
        };
        transaction.sample_final_state_0024();
    }
}

#[cfg(test)]
mod tests;
