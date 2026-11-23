//! The probe's second claim of the system timer: comparator 1's words for
//! the boot hart's task, comparator 2's for hart 1, and the counter's latch
//! for the ticks, beside the executor's time driver, which holds the
//! root's claim of the same part (`tasks/380`, the architect's addendum).
//!
//! The claim's blocks never leave this module: the probe reaches them
//! through [`One`], [`Two`] and [`Ticks`], whose operations touch only the
//! words their row of the table below names.

use oer_chips_esp32s31_pac::systimer::{
    Systimer, SystimerClaim, SystimerCounter, SystimerCounterClaim,
    dictionaries::{ComparatorInterrupt, ComparatorWork},
};
use oer_chips_family_drivers_system_timer::{CounterLatch, Unverified};
use oer_portable_bounded::Index;
use oer_portable_mmio::{Bits, ValueError};

/// The counter's range: a target is a value below it.
const RANGE: u64 = 1 << oer_hil_services_timer_facts::COUNTER_BITS;

/// Comparator 1's element of the SYSTIMER arrays.
const ONE: Index<3> = Index::constant::<1>();
/// Comparator 2's element.
const TWO: Index<3> = Index::constant::<2>();

/// Comparator 1, on the boot hart's thread context: its target, its load
/// and its raw status.
pub(crate) struct One(Systimer);

/// Comparator 2, on hart 1: its target, its load and its raw status.
pub(crate) struct Two(Systimer);

/// Unit 0's counter latch, read on the boot hart's thread context.
pub(crate) struct Ticks(SystimerCounter);

/// The second claim, made once by `main` before the executor runs: the
/// three owners, with comparators 1 and 2 set up.
///
/// Comparators 1 and 2 are set once here and never after: each in one-shot
/// mode on unit 0 (`TARGET1_CONF`, `TARGET2_CONF`), both enabled
/// (`CONF.TARGET1_WORK_EN`, `CONF.TARGET2_WORK_EN`), comparator 1's
/// interrupt enabled and comparator 2's left clear (`INT_ENA`).
pub(crate) fn claim() -> (One, Two, Ticks) {
    #[allow(
        unsafe_code,
        reason = "tasks/380: the probe's second claim of the system timer, approved by the architect"
    )]
    #[allow(
        clippy::disallowed_methods,
        reason = "tasks/380: the probe's second claim of the system timer, approved by the architect"
    )]
    // SAFETY: the words of these three blocks are also listed by the root's
    // claim, whose owners the executor's time driver holds; the letter of
    // `assume_owned`'s contract is not kept, and the words are kept apart by
    // hand. Each word has one writing context:
    //
    // | Context | Writes | Reads |
    // |---|---|---|
    // | hart 0 thread, the executor's time driver | comparator 0's `TARGET0_*`, `COMP0_LOAD`; read-modify-write of `CONF` and `INT_ENA` (its bind at init, its program) | `CONF`, `INT_ENA`, the counter latch |
    // | hart 0 thread, this probe's task (`One`, `Ticks`) | `TARGET1_CONF`, `TARGET2_CONF`, `CONF`, `INT_ENA` once, here, after the executor's init and before its run; then `TARGET1_HI`, `TARGET1_LO`, `COMP1_LOAD`; the latch's update | `INT_RAW`, `CONF`, `INT_ENA`, the latch |
    // | hart 1 (`Two`) | `TARGET2_HI`, `TARGET2_LO`, `COMP2_LOAD` | `INT_RAW` |
    // | each comparator's `SystimerAlarmN` part | its own `INT_CLR` bit (a write trigger, zero in the others) | |
    //
    // The two hart-0 rows run in one context, the executor's thread mode,
    // which never preempts itself: their read-modify-writes of `CONF` and
    // `INT_ENA` do not interleave, and the executor's keep comparators 1 and
    // 2's bits as they read them. Hart 1 writes no word another context
    // writes. The latch is read in the hart-0 thread context alone: no
    // handler of this image reads the time. The blocks are volatile accesses
    // at fixed addresses with no reference to memory.
    let (one, two, counter) = unsafe {
        oer_portable_mmio::assume_owned::<(SystimerClaim, SystimerClaim, SystimerCounterClaim)>()
    };
    let mut one = Systimer::new(one);
    for index in [ONE, TWO] {
        one.modify_target_conf(index, |conf| {
            conf.set_period_mode(false).set_timer_unit_sel(false);
        });
    }
    one.modify_conf(|conf| {
        conf.set_target1_work_en(ComparatorWork::Enabled)
            .set_target2_work_en(ComparatorWork::Enabled);
    });
    one.modify_int_ena(|enable| {
        enable.set_target1_int_ena(ComparatorInterrupt::Enabled);
    });
    (
        One(one),
        Two(Systimer::new(two)),
        Ticks(SystimerCounter::new(counter)),
    )
}

/// A target's words: its low 32 bits and its high 20, the value taken
/// modulo the counter's range first, so that its high word fits.
fn words(target: u64) -> Result<(u32, Bits<20>), ValueError> {
    let [low, high]: [u32; 2] = bytemuck::cast(target % RANGE);
    Ok((low, Bits::new(high)?))
}

impl One {
    /// `TARGET1_HI` alone, with no load.
    pub(crate) fn write_high(&mut self, target: u64) -> Result<(), ValueError> {
        let (_, high) = words(target)?;
        self.0.modify_target_hi(ONE, |word| {
            word.set_hi(high);
        });
        Ok(())
    }

    /// `TARGET1_LO` alone, with no load.
    pub(crate) fn write_low(&mut self, target: u64) -> Result<(), ValueError> {
        let (low, _) = words(target)?;
        self.0.modify_target_lo(ONE, |word| {
            word.set_lo(low);
        });
        Ok(())
    }

    /// `COMP1_LOAD`'s strobe alone.
    pub(crate) fn strobe(&mut self) {
        self.0.trigger_comp_load_load(ONE);
    }

    /// The target's high and low words, without the strobe.
    pub(crate) fn prepare(&mut self, target: u64) -> Result<(), ValueError> {
        let (low, high) = words(target)?;
        self.0.modify_target_hi(ONE, |word| {
            word.set_hi(high);
        });
        self.0.modify_target_lo(ONE, |word| {
            word.set_lo(low);
        });
        Ok(())
    }

    /// The target's high and low words, then the load's strobe; the
    /// comparator's enable untouched.
    pub(crate) fn load(&mut self, target: u64) -> Result<(), ValueError> {
        self.prepare(target)?;
        self.strobe();
        Ok(())
    }

    /// `INT_RAW.TARGET1_INT_RAW`.
    pub(crate) fn raised(&self) -> bool {
        self.0.read_int_raw().target1_int_raw()
    }

    /// Every comparator's enable and interrupt enable, read.
    pub(crate) fn enables(&self) -> ([bool; 3], [bool; 3]) {
        let conf = self.0.read_conf();
        let enable = self.0.read_int_ena();
        let work = |value| matches!(value, ComparatorWork::Enabled);
        let interrupt = |value| matches!(value, ComparatorInterrupt::Enabled);
        (
            [
                work(conf.target0_work_en()),
                work(conf.target1_work_en()),
                work(conf.target2_work_en()),
            ],
            [
                interrupt(enable.target0_int_ena()),
                interrupt(enable.target1_int_ena()),
                interrupt(enable.target2_int_ena()),
            ],
        )
    }
}

impl Two {
    /// The target's high and low words, then the load's strobe.
    pub(crate) fn load(&mut self, target: u64) -> Result<(), ValueError> {
        let (low, high) = words(target)?;
        self.0.modify_target_hi(TWO, |word| {
            word.set_hi(high);
        });
        self.0.modify_target_lo(TWO, |word| {
            word.set_lo(low);
        });
        self.0.trigger_comp_load_load(TWO);
        Ok(())
    }

    /// `INT_RAW.TARGET2_INT_RAW`.
    pub(crate) fn raised(&self) -> bool {
        self.0.read_int_raw().target2_int_raw()
    }
}

impl CounterLatch for Ticks {
    fn update(&self) {
        self.0.trigger_unit0_op_timer_unit0_update();
    }
    fn valid(&self) -> bool {
        self.0.read_unit0_op().timer_unit0_value_valid()
    }
    fn high(&self) -> u32 {
        self.0.read_unit0_value_hi().timer_unit0_value_hi().get()
    }
    fn low(&self) -> u32 {
        self.0.read_unit0_value_lo().timer_unit0_value_lo()
    }
}

impl Ticks {
    /// The counter, in ticks, read whole.
    pub(crate) fn now(&self) -> Result<u64, Unverified> {
        oer_chips_family_drivers_system_timer::read(self)
    }
}
