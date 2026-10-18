//! Complete, fact-bounded BTDM controller HAL initialization.
//!
//! This module owns the finite MMIO body reached by the ESP32-S31 controller
//! enable path immediately before controller-side IRQ output preparation.  It
//! deliberately models the vendor input ABI as typed hardware inputs instead
//! of copying its private structure layout or task/runtime machinery.

use crate::{BluetoothTaskRegisters, device_fence};

/// First positional scaling input accepted by the complete HAL-config setter.
///
/// The instruction evidence accepts exactly the byte values 8 and 16.  No
/// clock-unit meaning is assigned until a public hardware description or HIL
/// establishes it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothHalInitScale {
    /// Positional input image 8.
    Eight,
    /// Positional input image 16; used by the reviewed standalone profile.
    Sixteen,
}

impl BluetoothHalInitScale {
    const fn image(self) -> u8 {
        match self {
            Self::Eight => 8,
            Self::Sixteen => 16,
        }
    }
}

/// Second positional scaling input accepted by the complete HAL-config setter.
///
/// Only these three complete integer images are accepted by the recovered
/// setter.  The names intentionally do not claim a time unit.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothHalInitPeriod {
    /// Positional input image 500.
    Image500,
    /// Positional input image 1000.
    Image1000,
    /// Positional input image 2000; used by the reviewed standalone profile.
    Image2000,
}

impl BluetoothHalInitPeriod {
    const fn image(self) -> u32 {
        match self {
            Self::Image500 => 500,
            Self::Image1000 => 1_000,
            Self::Image2000 => 2_000,
        }
    }

    const fn transform_byte(self, value: u8) -> u8 {
        match self {
            // The internal selector is two and the helper shifts right by
            // selector-minus-one.
            Self::Image500 => value >> 1,
            // Selector one produces a zero-bit right shift.
            Self::Image1000 => value,
            // Selector zero takes the helper's alternate one-bit left shift;
            // the caller then retains only the low byte.
            Self::Image2000 => value.wrapping_shl(1),
        }
    }
}

/// Typed inputs to the complete ESP32-S31 BTDM HAL-init MMIO body.
///
/// `value_0` and `value_1` remain positional because their undocumented
/// hardware meanings have not been established.  Unlike an opaque vendor
/// structure, every field here has a bounded, reviewed transformation into a
/// specific register position.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BluetoothControllerHalInitConfig {
    scale: BluetoothHalInitScale,
    value_0: u8,
    value_1: u8,
    period: BluetoothHalInitPeriod,
}

/// Exact scale between a raw latched controller tick delta and microseconds.
///
/// Current and same-chip named bodies compose
/// `r_btdm_sleep_timer_ticks_get`, `r_btdm_hal_util_ticks_to_us` and
/// `r_sched_timer_convertTimeToUs`. DTM then combines that result with its
/// independently established microsecond durations before converting the
/// descriptor deadlines back through `r_sched_timer_convertTimeToTicks`.
/// Effective hardware counter width, wrap and wake causality remain separate
/// evidence obligations.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BluetoothControllerTimeScale {
    period: BluetoothHalInitPeriod,
}

/// Projection of raw ticks into whole microseconds and an unconsumed remainder.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BluetoothMicrosecondDeltaProjection {
    /// Whole microseconds produced by the forward conversion.
    pub whole_micros: u32,
    /// Raw ticks discarded by that conversion; retain them across epoch updates.
    pub remainder_ticks: u8,
}

/// Projection of one microsecond delta into the raw controller-tick domain.
///
/// `remainder_micros` retains the microsecond low bits discarded by the exact
/// inverse shift. Callers must choose a rounding policy explicitly rather than
/// silently treating the projection as exact.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BluetoothRawTickDeltaProjection {
    /// Whole raw-tick delta produced by the reviewed inverse transform.
    pub whole_ticks: u32,
    /// Microseconds discarded by that transform.
    pub remainder_micros: u8,
}

impl BluetoothControllerTimeScale {
    /// Software conversion selector established by the HAL config setter.
    ///
    /// This is independent of `BluetoothControllerHalInitConfig::sleep_timer_shift`,
    /// which configures the hardware divider. The standalone period selects zero:
    /// two raw ticks per microsecond.
    pub const fn shift_image(self) -> u8 {
        match self.period {
            BluetoothHalInitPeriod::Image500 => 2,
            BluetoothHalInitPeriod::Image1000 => 1,
            BluetoothHalInitPeriod::Image2000 => 0,
        }
    }

    /// Convert one raw latched-tick delta into microseconds.
    ///
    /// This uses wrapping 32-bit arithmetic, matching the complete RISC-V
    /// shift helper. It does not decide whether a particular wrapped delta is
    /// temporally before or after an anchor.
    pub const fn micros_from_raw_ticks(self, raw_ticks: u32) -> u32 {
        self.project_raw_ticks(raw_ticks).whole_micros
    }

    /// Convert ticks while preserving the fractional microsecond for re-anchoring.
    pub const fn project_raw_ticks(self, raw_ticks: u32) -> BluetoothMicrosecondDeltaProjection {
        match self.period {
            BluetoothHalInitPeriod::Image500 => BluetoothMicrosecondDeltaProjection {
                whole_micros: raw_ticks.wrapping_mul(2),
                remainder_ticks: 0,
            },
            BluetoothHalInitPeriod::Image1000 => BluetoothMicrosecondDeltaProjection {
                whole_micros: raw_ticks,
                remainder_ticks: 0,
            },
            BluetoothHalInitPeriod::Image2000 => BluetoothMicrosecondDeltaProjection {
                whole_micros: raw_ticks / 2,
                remainder_ticks: (raw_ticks % 2) as u8,
            },
        }
    }

    /// Project a physical duration without wrapping its tick count. This
    /// retains the reviewed inverse transform's fractional remainder; the
    /// accepting hardware owner separately enforces its delta range.
    pub fn checked_raw_ticks_from_micros(
        self,
        micros: u64,
    ) -> Option<BluetoothRawTickDeltaProjection> {
        let (ticks, remainder) = match self.period {
            BluetoothHalInitPeriod::Image500 => (micros / 2, (micros % 2) as u8),
            BluetoothHalInitPeriod::Image1000 => (micros, 0),
            BluetoothHalInitPeriod::Image2000 => (micros.checked_mul(2)?, 0),
        };
        Some(BluetoothRawTickDeltaProjection {
            whole_ticks: u32::try_from(ticks).ok()?,
            remainder_micros: remainder,
        })
    }

    /// Convert microseconds into raw ticks while retaining discarded time.
    pub const fn raw_ticks_from_micros(self, micros: u32) -> BluetoothRawTickDeltaProjection {
        match self.period {
            BluetoothHalInitPeriod::Image500 => BluetoothRawTickDeltaProjection {
                whole_ticks: micros / 2,
                remainder_micros: (micros % 2) as u8,
            },
            BluetoothHalInitPeriod::Image1000 => BluetoothRawTickDeltaProjection {
                whole_ticks: micros,
                remainder_micros: 0,
            },
            BluetoothHalInitPeriod::Image2000 => BluetoothRawTickDeltaProjection {
                whole_ticks: micros.wrapping_mul(2),
                remainder_micros: 0,
            },
        }
    }
}

impl BluetoothControllerHalInitConfig {
    /// Construct one configuration from the complete setter's accepted input
    /// domains.
    pub const fn new(
        scale: BluetoothHalInitScale,
        value_0: u8,
        value_1: u8,
        period: BluetoothHalInitPeriod,
    ) -> Self {
        Self {
            scale,
            value_0,
            value_1,
            period,
        }
    }

    /// Exact input profile constructed by the pinned ESP32-S31 controller
    /// task: bytes 16, 11 and 33 plus period image 2000.
    pub const fn reviewed_standalone() -> Self {
        Self::new(
            BluetoothHalInitScale::Sixteen,
            11,
            33,
            BluetoothHalInitPeriod::Image2000,
        )
    }

    /// Low-three-bit sleep-timer image derived by exact integer division.
    pub const fn sleep_timer_shift(self) -> u8 {
        match (self.scale, self.period) {
            (BluetoothHalInitScale::Eight, BluetoothHalInitPeriod::Image500) => 4,
            (BluetoothHalInitScale::Eight, BluetoothHalInitPeriod::Image1000) => 3,
            (BluetoothHalInitScale::Eight, BluetoothHalInitPeriod::Image2000) => 2,
            (BluetoothHalInitScale::Sixteen, BluetoothHalInitPeriod::Image500) => 5,
            (BluetoothHalInitScale::Sixteen, BluetoothHalInitPeriod::Image1000) => 4,
            (BluetoothHalInitScale::Sixteen, BluetoothHalInitPeriod::Image2000) => 3,
        }
    }

    /// Exact raw-controller-time to BLE-scheduler scale selected by this
    /// initialization profile.
    pub const fn controller_time_scale(self) -> BluetoothControllerTimeScale {
        BluetoothControllerTimeScale {
            period: self.period,
        }
    }

    /// First byte image after the reviewed period-dependent transformation.
    pub const fn transformed_value_0(self) -> u8 {
        self.period.transform_byte(self.value_0)
    }

    /// Second byte image after the reviewed period-dependent transformation.
    pub const fn transformed_value_1(self) -> u8 {
        self.period.transform_byte(self.value_1)
    }

    /// Original finite scale image, useful for evidence and diagnostics.
    pub const fn scale_image(self) -> u8 {
        self.scale.image()
    }

    /// Original finite period image, useful for evidence and diagnostics.
    pub const fn period_image(self) -> u32 {
        self.period.image()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum HalInitRegister {
    SlotMap0,
    SlotMap1,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum HalInitOperation {
    PublishSchedulerSramPrefix,
    PublishSleepTimerShift(u8),
    PublishValue0(u8),
    PublishValue1(u8),
    InitializeLatch,
    InitializeLow20,
    EnableLatch,
    ConfigureControl1High,
    ConfigureControl1Low,
    EnableControl0,
    ClearSchedulerConfig16To20,
    PublishSchedulerConfig16To20,
    EnableSchedulerControl,
    ClearLowHalf,
    FillLowHalf,
    ClearSchedulerByte1,
    PublishSchedulerByte1,
    ClearSlotLaneUpper {
        register: HalInitRegister,
        lane: u8,
    },
    PublishSlotLane {
        register: HalInitRegister,
        lane: u8,
        set_retained_index_low: bool,
        index_high: u8,
    },
}

trait HalInitTransaction {
    fn apply(&mut self, operation: HalInitOperation);
}

fn execute_hal_init(
    transaction: &mut impl HalInitTransaction,
    config: BluetoothControllerHalInitConfig,
) {
    transaction.apply(HalInitOperation::PublishSchedulerSramPrefix);
    transaction.apply(HalInitOperation::PublishSleepTimerShift(
        config.sleep_timer_shift(),
    ));
    transaction.apply(HalInitOperation::PublishValue0(
        config.transformed_value_0(),
    ));
    transaction.apply(HalInitOperation::PublishValue1(
        config.transformed_value_1(),
    ));
    transaction.apply(HalInitOperation::InitializeLatch);
    transaction.apply(HalInitOperation::InitializeLow20);
    transaction.apply(HalInitOperation::EnableLatch);
    transaction.apply(HalInitOperation::ConfigureControl1High);
    transaction.apply(HalInitOperation::ConfigureControl1Low);
    transaction.apply(HalInitOperation::EnableControl0);
    transaction.apply(HalInitOperation::ClearSchedulerConfig16To20);
    transaction.apply(HalInitOperation::PublishSchedulerConfig16To20);
    transaction.apply(HalInitOperation::EnableSchedulerControl);
    transaction.apply(HalInitOperation::ClearLowHalf);
    transaction.apply(HalInitOperation::FillLowHalf);
    transaction.apply(HalInitOperation::ClearSchedulerByte1);
    transaction.apply(HalInitOperation::PublishSchedulerByte1);

    for global_index in 0..16_u8 {
        let register = if global_index < 8 {
            HalInitRegister::SlotMap0
        } else {
            HalInitRegister::SlotMap1
        };
        let lane = global_index % 8;
        let index_in_group = global_index % 4;
        transaction.apply(HalInitOperation::ClearSlotLaneUpper { register, lane });
        transaction.apply(HalInitOperation::PublishSlotLane {
            register,
            lane,
            set_retained_index_low: index_in_group % 2 == 1,
            index_high: u8::from(index_in_group >= 2),
        });
    }
}

struct MmioHalInit<'a> {
    registers: &'a crate::svd::BluetoothControllerCore,
}

impl HalInitTransaction for MmioHalInit<'_> {
    fn apply(&mut self, operation: HalInitOperation) {
        match operation {
            HalInitOperation::PublishSchedulerSramPrefix => {
                crate::svd::fixed_register_image::publish_bluetooth_hal_scheduler_sram_prefix(
                    self.registers,
                );
            }
            HalInitOperation::PublishSleepTimerShift(shift) => {
                let shift = match shift {
                    2 => crate::generated::BluetoothHalSleepTimerShift::Two,
                    3 => crate::generated::BluetoothHalSleepTimerShift::Three,
                    4 => crate::generated::BluetoothHalSleepTimerShift::Four,
                    5 => crate::generated::BluetoothHalSleepTimerShift::Five,
                    _ => unreachable!("HAL input domains produce only reviewed timer shifts"),
                };
                crate::generated::publish_bluetooth_hal_sleep_timer_shift(self.registers, shift);
            }
            HalInitOperation::PublishValue0(value) => {
                let value = crate::generated::BluetoothHalInitByte::new(u32::from(value))
                    .expect("one byte always fits the generated HAL-init domain");
                crate::generated::publish_bluetooth_hal_value_0(self.registers, value);
            }
            HalInitOperation::PublishValue1(value) => {
                let value = crate::generated::BluetoothHalInitByte::new(u32::from(value))
                    .expect("one byte always fits the generated HAL-init domain");
                crate::generated::publish_bluetooth_hal_value_1(self.registers, value);
            }
            HalInitOperation::InitializeLatch => {
                crate::svd::zero_based_field_write::initialize_bluetooth_hal_latch(
                    self.registers,
                    true,
                );
            }
            HalInitOperation::InitializeLow20 => {
                crate::svd::zero_based_field_write::initialize_bluetooth_hal_low_20(
                    self.registers,
                    0x000f_ffff,
                );
            }
            HalInitOperation::EnableLatch => {
                crate::svd::field_replace_modify::enable_bluetooth_hal_latch(self.registers);
            }
            HalInitOperation::ConfigureControl1High => {
                crate::svd::field_replace_modify::configure_bluetooth_hal_control_1_high(
                    self.registers,
                );
            }
            HalInitOperation::ConfigureControl1Low => {
                crate::svd::field_replace_modify::configure_bluetooth_hal_control_1_low(
                    self.registers,
                );
            }
            HalInitOperation::EnableControl0 => {
                crate::svd::field_replace_modify::enable_bluetooth_hal_control_0(self.registers);
            }
            HalInitOperation::ClearSchedulerConfig16To20 => {
                crate::svd::field_replace_modify::clear_bluetooth_hal_scheduler_config_16_20(
                    self.registers,
                );
            }
            HalInitOperation::PublishSchedulerConfig16To20 => {
                crate::svd::field_replace_modify::publish_bluetooth_hal_scheduler_config_16_20(
                    self.registers,
                );
            }
            HalInitOperation::EnableSchedulerControl => {
                crate::svd::field_replace_modify::enable_bluetooth_hal_scheduler_control(
                    self.registers,
                );
            }
            HalInitOperation::ClearLowHalf => {
                crate::svd::field_replace_modify::clear_bluetooth_hal_low_half(self.registers);
            }
            HalInitOperation::FillLowHalf => {
                crate::svd::field_replace_modify::fill_bluetooth_hal_low_half(self.registers);
            }
            HalInitOperation::ClearSchedulerByte1 => {
                crate::svd::field_replace_modify::clear_bluetooth_hal_scheduler_byte_1(
                    self.registers,
                );
            }
            HalInitOperation::PublishSchedulerByte1 => {
                crate::svd::field_replace_modify::publish_bluetooth_hal_scheduler_byte_1(
                    self.registers,
                );
            }
            HalInitOperation::ClearSlotLaneUpper { register, lane } => {
                let register_index = match register {
                    HalInitRegister::SlotMap0 => 0,
                    HalInitRegister::SlotMap1 => 1,
                };
                match lane {
                    0 => crate::svd::field_replace_modify::clear_bluetooth_hal_slot_lane_0_upper(
                        self.registers,
                        register_index,
                    ),
                    1 => crate::svd::field_replace_modify::clear_bluetooth_hal_slot_lane_1_upper(
                        self.registers,
                        register_index,
                    ),
                    2 => crate::svd::field_replace_modify::clear_bluetooth_hal_slot_lane_2_upper(
                        self.registers,
                        register_index,
                    ),
                    3 => crate::svd::field_replace_modify::clear_bluetooth_hal_slot_lane_3_upper(
                        self.registers,
                        register_index,
                    ),
                    4 => crate::svd::field_replace_modify::clear_bluetooth_hal_slot_lane_4_upper(
                        self.registers,
                        register_index,
                    ),
                    5 => crate::svd::field_replace_modify::clear_bluetooth_hal_slot_lane_5_upper(
                        self.registers,
                        register_index,
                    ),
                    6 => crate::svd::field_replace_modify::clear_bluetooth_hal_slot_lane_6_upper(
                        self.registers,
                        register_index,
                    ),
                    7 => crate::svd::field_replace_modify::clear_bluetooth_hal_slot_lane_7_upper(
                        self.registers,
                        register_index,
                    ),
                    _ => unreachable!("HAL-init slot lane is bounded to eight entries"),
                };
            }
            HalInitOperation::PublishSlotLane {
                register,
                lane,
                set_retained_index_low,
                index_high,
            } => {
                let register_index = match register {
                    HalInitRegister::SlotMap0 => 0,
                    HalInitRegister::SlotMap1 => 1,
                };
                match (lane, set_retained_index_low, index_high) {
                    (0, false, 0) => {
                        crate::svd::field_replace_modify::publish_bluetooth_hal_slot_lane_0(
                            self.registers,
                            register_index,
                        )
                    }
                    (1, true, 0) => {
                        crate::svd::field_replace_modify::publish_bluetooth_hal_slot_lane_1(
                            self.registers,
                            register_index,
                        )
                    }
                    (2, false, 1) => {
                        crate::svd::field_replace_modify::publish_bluetooth_hal_slot_lane_2(
                            self.registers,
                            register_index,
                        )
                    }
                    (3, true, 1) => {
                        crate::svd::field_replace_modify::publish_bluetooth_hal_slot_lane_3(
                            self.registers,
                            register_index,
                        )
                    }
                    (4, false, 0) => {
                        crate::svd::field_replace_modify::publish_bluetooth_hal_slot_lane_4(
                            self.registers,
                            register_index,
                        )
                    }
                    (5, true, 0) => {
                        crate::svd::field_replace_modify::publish_bluetooth_hal_slot_lane_5(
                            self.registers,
                            register_index,
                        )
                    }
                    (6, false, 1) => {
                        crate::svd::field_replace_modify::publish_bluetooth_hal_slot_lane_6(
                            self.registers,
                            register_index,
                        )
                    }
                    (7, true, 1) => {
                        crate::svd::field_replace_modify::publish_bluetooth_hal_slot_lane_7(
                            self.registers,
                            register_index,
                        )
                    }
                    _ => unreachable!("HAL-init slot lane is bounded to eight entries"),
                };
            }
        }
    }
}

impl BluetoothTaskRegisters {
    /// Execute the complete recovered BTDM controller HAL-init MMIO body.
    ///
    /// SOURCE: pinned ESP32-S31 `libbtdm_common.a` member `7.o`, complete
    /// symbol `r_sym_bt_aGdrujd2MUAzWYH75baR`, plus its complete config setter,
    /// byte-scaling helper and caller `r_btdm_task_init` in member `20.o`.
    /// The transaction has 49 ordered writes/RMWs and finishes before
    /// controller IRQ output setup; unlike earlier bodies it leaves the
    /// sleep-timer control bits 24..31 as they are.
    ///
    /// This method does not initialize software events/lists, route a CPU
    /// interrupt, enable the Link Layer or claim HCI readiness.
    ///
    /// Enabled clocks, the selected SRAM-prefix policy and inactive interrupt
    /// lifecycle are upper-layer ownership facts. PHY, BTBB, scheduler lists,
    /// Link Layer and HCI are later stages and are not established here. None
    /// of those facts is represented by a forgeable PAC proof token.
    #[doc(hidden)]
    pub fn initialize_controller_hal(&mut self, config: BluetoothControllerHalInitConfig) {
        let mut transaction = MmioHalInit {
            registers: &self.bluetooth.bluetooth_controller_core,
        };
        execute_hal_init(&mut transaction, config);
        device_fence();
    }
}

#[cfg(test)]
mod tests;
