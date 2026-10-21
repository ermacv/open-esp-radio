//! The executor's comparator ownership, independent of esp-hal's UNIT0 latch.

use esp_hal::peripherals::SYSTIMER;

/// A hardware comparator; every value names an existing target.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Comparator {
    Zero,
    One,
    Two,
}

impl Comparator {
    pub(crate) const fn from_index(index: u8) -> Option<Self> {
        match index {
            0 => Some(Self::Zero),
            1 => Some(Self::One),
            2 => Some(Self::Two),
            _ => None,
        }
    }

    const fn index(self) -> usize {
        self as usize
    }
}

/// The sole writer of comparator configuration, held by the writer executor.
pub struct ComparatorWriter {
    _systimer: SYSTIMER<'static>,
}

/// One handler's authority to write its comparator's W1C acknowledgement.
pub struct ComparatorAcknowledger<const CHANNEL: u8> {
    _private: (),
}

impl ComparatorWriter {
    /// Consume the running timer without resetting its counter or clock.
    /// The returned owners cannot be cloned or obtained a second time.
    pub fn adopt(
        systimer: SYSTIMER<'static>,
    ) -> (
        Self,
        ComparatorAcknowledger<0>,
        ComparatorAcknowledger<1>,
        ComparatorAcknowledger<2>,
    ) {
        (
            Self {
                _systimer: systimer,
            },
            ComparatorAcknowledger { _private: () },
            ComparatorAcknowledger { _private: () },
            ComparatorAcknowledger { _private: () },
        )
    }

    /// Bind unit zero in one-shot mode before its handler's route is enabled.
    /// Setup still holds both the writer and this comparator's acknowledger.
    pub fn bind<const CHANNEL: u8>(&mut self, ack: &mut ComparatorAcknowledger<CHANNEL>) {
        let comparator = ComparatorAcknowledger::<CHANNEL>::COMPARATOR;
        let regs = SYSTIMER::regs();
        regs.target_conf(comparator.index())
            .modify(|_, w| w.period_mode().clear_bit().timer_unit_sel().clear_bit());
        self.set_enabled(comparator, false);
        ack.acknowledge();
        regs.int_ena().modify(|_, w| w.target(CHANNEL).set_bit());
    }

    #[inline(always)]
    fn set_enabled(&mut self, comparator: Comparator, enabled: bool) {
        SYSTIMER::regs().conf().modify(|_, w| match comparator {
            Comparator::Zero => w.target0_work_en().bit(enabled),
            Comparator::One => w.target1_work_en().bit(enabled),
            Comparator::Two => w.target2_work_en().bit(enabled),
        });
    }

    /// Disable, load the high/low target and COMP_LOAD, then enable.
    /// `None` only disables the comparator. The typed index needs no check.
    pub fn program(&mut self, comparator: Comparator, ticks: Option<u64>) {
        self.set_enabled(comparator, false);
        let Some(ticks) = ticks else { return };
        let regs = SYSTIMER::regs();
        let target = regs.trgt(comparator.index());
        // Both supported targets are little-endian; split the counter
        // value into words without raw register masks or shifts.
        let [lo, hi]: [u32; 2] = bytemuck::cast(ticks);
        target.hi().write(|w| w.hi().set(hi));
        target.lo().write(|w| w.lo().set(lo));
        regs.comp_load(comparator.index())
            .write(|w| w.load().set_bit());
        self.set_enabled(comparator, true);
    }
}

impl<const CHANNEL: u8> ComparatorAcknowledger<CHANNEL> {
    const COMPARATOR: Comparator = match Comparator::from_index(CHANNEL) {
        Some(comparator) => comparator,
        None => panic!("no such system timer comparator"),
    };

    /// Write only this comparator's bit. No read or read-modify-write occurs.
    #[inline(always)]
    pub fn acknowledge(&mut self) {
        const { assert!(CHANNEL < 3, "no such system timer comparator") };
        crate::systimer_ack::clear_alarm!(SYSTIMER::regs().int_clr(), CHANNEL);
    }
}
