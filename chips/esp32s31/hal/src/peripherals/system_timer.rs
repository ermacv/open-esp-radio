//! The system timer over the root's parts: the comparators the time driver
//! programs ([`SystimerComparators`]), the counter latch every context
//! reads ([`SystimerCounter`]) and each comparator's alarm part, which its
//! handler holds to acknowledge its own interrupt ([`SystimerAlarm0`] ..
//! [`SystimerAlarm2`]).
//!
//! The executor's time driver calls these operations; the register
//! sequences are here.

use oer_esp32s31_pac::{
    hp_sys_clkrst::HpSysClkrstSystimer, instances, ownership::SystemTimerPeripherals, systimer,
};
use oer_espressif_boot::drivers::system_timer::CounterLatch;
use oer_espressif_platform_port::{AlarmPart, Comparator, Comparators};
use oer_mmio::{Bits, Fixed, Index, InterruptSource, Placement};

#[cfg(test)]
mod tests;

/// The system timer's ticks per microsecond: it counts at 16 MHz.
///
/// SOURCE: ESP-IDF 4d80c1b2 `esp_hw_support/port/esp32s31/systimer.c`
/// (`systimer_ticks_to_us`: `ticks / 16`).
pub const TICKS_PER_MICROSECOND: u64 = 16;

/// Where the system timer lies.
pub type SystimerPlacement = Fixed<{ instances::systimer::BASE }>;

/// The bootstrap's unstarted timer clock and counter latch.
#[must_use = "the bootstrap must open the timer before checking its latch"]
pub struct BootstrapTimer {
    clock: HpSysClkrstSystimer<Fixed<{ instances::hp_sys_clkrst::BASE }>>,
    counter: SystimerCounter,
}

impl BootstrapTimer {
    pub(crate) fn new(
        clock: HpSysClkrstSystimer<Fixed<{ instances::hp_sys_clkrst::BASE }>>,
        counter: SystimerCounter,
    ) -> Self {
        Self { clock, counter }
    }

    /// Open the sourced timer gates and retire their owner, returning the latch.
    pub fn open_clock(mut self) -> SystimerCounter {
        open_clock(&mut self.clock);
        self.counter
    }
}

/// The comparator's element of the SYSTIMER arrays.
const fn index(comparator: Comparator) -> Index<3> {
    const INDICES: [Index<3>; 3] = [
        Index::constant::<0>(),
        Index::constant::<1>(),
        Index::constant::<2>(),
    ];
    match comparator {
        Comparator::Zero => INDICES[0],
        Comparator::One => INDICES[1],
        Comparator::Two => INDICES[2],
    }
}

/// The comparators' configuration, targets, loads, enables and interrupt
/// enables: the time driver's writer owns it.
#[must_use = "dropping a register owner permanently loses its register authority"]
pub struct SystimerComparators<P: Placement = SystimerPlacement>(systimer::Systimer<P>);

/// The counter latch of unit zero, which every context reads: a strobe
/// latches the counter, a flag says when the latched words are valid.
pub struct SystimerCounter<P: Placement = SystimerPlacement>(systimer::SystimerCounter<P>);

macro_rules! alarm {
    ($(#[$doc:meta])* $name:ident, $comparator:ident, $trigger:ident) => {
        $(#[$doc])*
        #[must_use = "dropping a register owner permanently loses its register authority"]
        pub struct $name<P: Placement = SystimerPlacement>(systimer::$name<P>);

        impl<P: Placement> InterruptSource for $name<P> {
            const SOURCE: u16 = <systimer::$name<P> as InterruptSource>::SOURCE;
        }

        impl<P: Placement> AlarmPart for $name<P> {
            const COMPARATOR: Comparator = Comparator::$comparator;

            #[inline(always)]
            fn acknowledge(&mut self) {
                self.0.$trigger();
            }
        }
    };
}

alarm!(
    /// Comparator 0's alarm part.
    SystimerAlarm0, Zero, trigger_int_clr_target0_int_clr
);
alarm!(
    /// Comparator 1's alarm part.
    SystimerAlarm1, One, trigger_int_clr_target1_int_clr
);
alarm!(
    /// Comparator 2's alarm part.
    SystimerAlarm2, Two, trigger_int_clr_target2_int_clr
);

/// The running timer's disjoint owners, split from the root.
pub struct SystemTimer {
    pub comparators: SystimerComparators,
    pub counter: SystimerCounter,
    pub alarm0: SystimerAlarm0,
    pub alarm1: SystimerAlarm1,
    pub alarm2: SystimerAlarm2,
}

impl SystemTimer {
    /// Split the root's system timer partition.
    pub fn new(peripherals: SystemTimerPeripherals) -> Self {
        let SystemTimerPeripherals {
            systimer,
            systimer_counter,
            systimer_alarm0,
            systimer_alarm1,
            systimer_alarm2,
        } = peripherals;
        Self {
            comparators: SystimerComparators(systimer),
            counter: SystimerCounter(systimer_counter),
            alarm0: SystimerAlarm0(systimer_alarm0),
            alarm1: SystimerAlarm1(systimer_alarm1),
            alarm2: SystimerAlarm2(systimer_alarm2),
        }
    }
}

impl<P: Placement> SystimerCounter<P> {
    /// Latch the counter's current value.
    ///
    /// SOURCE: ESP-IDF 4d80c1b2 `systimer_ll.h:120`
    /// (`systimer_ll_counter_snapshot`).
    #[inline(always)]
    pub fn update(&self) {
        self.0.trigger_unit0_op_timer_unit0_update();
    }

    /// Whether the latched words hold the value of the last update.
    ///
    /// SOURCE: ESP-IDF 4d80c1b2 `systimer_ll.h:125`
    /// (`systimer_ll_is_counter_value_valid`).
    #[inline(always)]
    pub fn valid(&self) -> bool {
        self.0.read_unit0_op().timer_unit0_value_valid()
    }

    /// The latched value's high 20 bits.
    #[inline(always)]
    pub fn high(&self) -> u32 {
        self.0.read_unit0_value_hi().timer_unit0_value_hi().get()
    }

    /// The latched value's low 32 bits.
    #[inline(always)]
    pub fn low(&self) -> u32 {
        self.0.read_unit0_value_lo().timer_unit0_value_lo()
    }
}

impl<P: Placement> CounterLatch for SystimerCounter<P> {
    #[inline(always)]
    fn update(&self) {
        Self::update(self);
    }
    #[inline(always)]
    fn valid(&self) -> bool {
        Self::valid(self)
    }
    #[inline(always)]
    fn high(&self) -> u32 {
        Self::high(self)
    }
    #[inline(always)]
    fn low(&self) -> u32 {
        Self::low(self)
    }
}

/// Open the system timer's clocks before the first read of its counter: the
/// bus clock, a reset pulse, then the function clock. The source selector
/// and the counter's own configuration stay as found (unit zero counts from
/// reset).
///
/// SOURCE: esp-hal 1f646c5 `esp-hal/src/soc/esp32s31/mod.rs:77-85` (the
/// S31 ROM leaves both gates closed) and ESP-IDF 4d80c1b2
/// `components/esp_hal_systimer/esp32s31/include/hal/systimer_ll.h:24-89`
/// (`systimer_ll_enable_bus_clock`, `systimer_ll_reset_register`,
/// `systimer_ll_enable_clock`).
pub fn open_clock<P: Placement>(clock: &mut HpSysClkrstSystimer<P>) {
    clock.modify_systimer_ctrl0(|w| {
        w.set_reg_systimer_apb_clk_en(true);
    });
    clock.modify_systimer_ctrl0(|w| {
        w.set_reg_systimer_rst_en(true);
    });
    clock.modify_systimer_ctrl0(|w| {
        w.set_reg_systimer_rst_en(false);
    });
    clock.modify_systimer_ctrl0(|w| {
        w.set_reg_systimer_clk_en(true);
    });
}

impl<P: Placement> SystimerComparators<P> {
    #[inline(always)]
    fn set_enabled(&mut self, comparator: Comparator, enabled: bool) {
        self.0.modify_conf(|conf| {
            match comparator {
                Comparator::Zero => conf.set_target0_work_en(enabled),
                Comparator::One => conf.set_target1_work_en(enabled),
                Comparator::Two => conf.set_target2_work_en(enabled),
            };
        });
    }
}

impl<P: Placement> Comparators for SystimerComparators<P> {
    /// Bind `alarm`'s comparator in one-shot mode on unit zero before its
    /// handler's route is enabled: the mode set, the comparator disabled,
    /// a stale interrupt acknowledged, then its interrupt enabled.
    fn bind<A: AlarmPart>(&mut self, alarm: &mut A) {
        let comparator = A::COMPARATOR;
        self.0.modify_target_conf(index(comparator), |conf| {
            conf.set_period_mode(false).set_timer_unit_sel(false);
        });
        self.set_enabled(comparator, false);
        alarm.acknowledge();
        self.0.modify_int_ena(|enable| {
            match comparator {
                Comparator::Zero => enable.set_target0_int_ena(true),
                Comparator::One => enable.set_target1_int_ena(true),
                Comparator::Two => enable.set_target2_int_ena(true),
            };
        });
    }

    /// Disable `comparator`, load the high and low target and strobe its
    /// load, then enable it. `None` only disables the comparator. The
    /// target's bits above the counter's 52 are dropped.
    ///
    /// SOURCE: ESP-IDF 4d80c1b2 `esp_hal_systimer/systimer_hal.c`
    /// (`systimer_hal_set_alarm_target`).
    fn program(&mut self, comparator: Comparator, ticks: Option<u64>) {
        self.set_enabled(comparator, false);
        let Some(ticks) = ticks else { return };
        let index = index(comparator);
        // Both supported targets are little-endian; split the counter value
        // into words without raw register masks or shifts.
        let [low, high]: [u32; 2] = bytemuck::cast(ticks);
        self.0.modify_target_hi(index, |target| {
            target.set_hi(Bits::truncate(high));
        });
        self.0.modify_target_lo(index, |target| {
            target.set_lo(low);
        });
        self.0.trigger_comp_load_load(index);
        self.set_enabled(comparator, true);
    }
}

#[cfg(any(test, feature = "test-support"))]
impl<'a> SystimerComparators<oer_mmio::Host<'a>> {
    /// The comparators over host memory from the instance's base.
    pub fn host(words: &'a mut [u32]) -> Result<Self, oer_mmio::HostTooSmall> {
        systimer::Systimer::host(words).map(Self)
    }
}

#[cfg(any(test, feature = "test-support"))]
impl<'a> SystimerCounter<oer_mmio::Host<'a>> {
    /// The counter latch over host memory from the instance's base.
    pub fn host(words: &'a mut [u32]) -> Result<Self, oer_mmio::HostTooSmall> {
        systimer::SystimerCounter::host(words).map(Self)
    }
}

/// The three alarm parts over one host memory.
#[cfg(any(test, feature = "test-support"))]
pub type HostAlarms<'a> = (
    SystimerAlarm0<oer_mmio::Host<'a>>,
    SystimerAlarm1<oer_mmio::Host<'a>>,
    SystimerAlarm2<oer_mmio::Host<'a>>,
);

/// The three alarm parts over host memory from the instance's base.
#[cfg(any(test, feature = "test-support"))]
pub fn alarms_host(words: &mut [u32]) -> Result<HostAlarms<'_>, oer_mmio::HostTooSmall> {
    systimer::systimer_host(words).map(|(zero, one, two)| {
        (
            SystimerAlarm0(zero),
            SystimerAlarm1(one),
            SystimerAlarm2(two),
        )
    })
}
