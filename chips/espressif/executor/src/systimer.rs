//! The executor's system timer: the comparator owner and each comparator's
//! acknowledgement on the typed SYSTIMER blocks, and the shared counter
//! latch published for [`crate::chip`]'s clock.

use crate::chip::platform;

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

    /// The comparator's element of the SYSTIMER arrays.
    const fn index(self) -> oer_mmio::Index<3> {
        const INDICES: [oer_mmio::Index<3>; 3] = {
            let mut indices = [match oer_mmio::Index::new(0) {
                Ok(index) => index,
                Err(_) => panic!("the arrays have a first element"),
            }; 3];
            let mut element = 1;
            while element < 3 {
                indices[element] = match oer_mmio::Index::new(element) {
                    Ok(index) => index,
                    Err(_) => panic!("every comparator indexes the three-element arrays"),
                };
                element += 1;
            }
            indices
        };
        match self {
            Self::Zero => INDICES[0],
            Self::One => INDICES[1],
            Self::Two => INDICES[2],
        }
    }
}

/// The running timer's disjoint owners, moved from the reset root.
pub struct SystemTimer {
    pub comparators: platform::Systimer,
    pub counter: platform::SystimerCounter,
    pub acknowledge: platform::SystimerAcknowledge,
}

/// The sole writer of comparator configuration, held by the writer executor.
pub struct ComparatorWriter {
    timer: platform::Systimer,
}

/// One handler's authority to strobe its comparator's acknowledgement.
pub struct ComparatorAcknowledger<const CHANNEL: u8> {
    acknowledge: &'static platform::SystimerAcknowledge,
}

/// The shared acknowledgement block, which the three alarm handlers strobe.
static ACKNOWLEDGE: oer_memory::zeroed::ZeroedOnce<platform::SystimerAcknowledge> =
    oer_memory::zeroed::ZeroedOnce::new();

impl ComparatorWriter {
    /// Consume the running timer without resetting its counter or clock:
    /// claim the system timer's blocks, publish the counter latch and return
    /// the comparator owner and each comparator's acknowledger. The returned
    /// owners cannot be cloned or obtained a second time.
    pub fn adopt(
        systimer: SystemTimer,
    ) -> (
        Self,
        ComparatorAcknowledger<0>,
        ComparatorAcknowledger<1>,
        ComparatorAcknowledger<2>,
    ) {
        crate::chip::install_counter(systimer.counter);
        assert!(
            ACKNOWLEDGE.get().is_none(),
            "the acknowledgement owner is published once"
        );
        let acknowledge = ACKNOWLEDGE.get_or_init(|| systimer.acknowledge);
        (
            Self {
                timer: systimer.comparators,
            },
            ComparatorAcknowledger { acknowledge },
            ComparatorAcknowledger { acknowledge },
            ComparatorAcknowledger { acknowledge },
        )
    }

    /// Bind unit zero in one-shot mode before its handler's route is enabled.
    /// Setup still holds both the writer and this comparator's acknowledger.
    pub fn bind<const CHANNEL: u8>(&mut self, ack: &mut ComparatorAcknowledger<CHANNEL>) {
        let comparator = ComparatorAcknowledger::<CHANNEL>::COMPARATOR;
        self.timer.modify_target_conf(comparator.index(), |conf| {
            conf.set_period_mode(false).set_timer_unit_sel(false);
        });
        self.set_enabled(comparator, false);
        ack.acknowledge();
        self.timer.modify_int_ena(|enable| {
            match comparator {
                Comparator::Zero => enable.set_target0_int_ena(true),
                Comparator::One => enable.set_target1_int_ena(true),
                Comparator::Two => enable.set_target2_int_ena(true),
            };
        });
    }

    #[inline(always)]
    fn set_enabled(&mut self, comparator: Comparator, enabled: bool) {
        self.timer.modify_conf(|conf| {
            match comparator {
                Comparator::Zero => conf.set_target0_work_en(enabled),
                Comparator::One => conf.set_target1_work_en(enabled),
                Comparator::Two => conf.set_target2_work_en(enabled),
            };
        });
    }

    /// Disable, load the high/low target and COMP_LOAD, then enable.
    /// `None` only disables the comparator. The typed index needs no check.
    ///
    /// SOURCE: ESP-IDF 4d59230d `esp_hal_systimer/systimer_hal.c`
    /// (`systimer_hal_set_alarm_target`).
    pub fn program(&mut self, comparator: Comparator, ticks: Option<u64>) {
        self.set_enabled(comparator, false);
        let Some(ticks) = ticks else { return };
        let index = comparator.index();
        // Both supported targets are little-endian; split the counter value
        // into words without raw register masks or shifts.
        let [low, high]: [u32; 2] = bytemuck::cast(ticks);
        self.timer.modify_target_hi(index, |target| {
            target.set_hi(oer_mmio::Bits::truncate(high));
        });
        self.timer.modify_target_lo(index, |target| {
            target.set_lo(low);
        });
        self.timer.write_comp_load(index, |load| {
            load.set_load(true);
        });
        self.set_enabled(comparator, true);
    }
}

impl<const CHANNEL: u8> ComparatorAcknowledger<CHANNEL> {
    const COMPARATOR: Comparator = match Comparator::from_index(CHANNEL) {
        Some(comparator) => comparator,
        None => panic!("no such system timer comparator"),
    };

    /// Strobe only this comparator's acknowledgement bit; nothing is read
    /// back and no other bit is written.
    #[inline(always)]
    pub fn acknowledge(&mut self) {
        match Self::COMPARATOR {
            Comparator::Zero => self.acknowledge.trigger_int_clr_target0_int_clr(),
            Comparator::One => self.acknowledge.trigger_int_clr_target1_int_clr(),
            Comparator::Two => self.acknowledge.trigger_int_clr_target2_int_clr(),
        }
    }
}
