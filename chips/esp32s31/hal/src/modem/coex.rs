//! The internal coexistence timer bank, borrowed apart from the PHY.
#![forbid(unsafe_code)]
use super::values::{CoexTimerClientValue, CoexTimerPtiValue};
use oer_esp32s31_pac::{coex::CoexHwTimer, instances};
use oer_mmio::{Bits, Fixed, Index, Placement};

pub const COEX_TIMER_COUNT: u8 = oer_esp32s31_pac::coex::configuration::COUNT as u8;
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum CoexTimerRegister {
    Timer0,
    Timer1,
    Timer2,
    Timer3,
    Timer4,
    Timer5,
}
impl CoexTimerRegister {
    pub const ALL: [Self; oer_esp32s31_pac::coex::configuration::COUNT] = [
        Self::Timer0,
        Self::Timer1,
        Self::Timer2,
        Self::Timer3,
        Self::Timer4,
        Self::Timer5,
    ];
    pub const fn new(value: u8) -> Option<Self> {
        match value {
            0 => Some(Self::Timer0),
            1 => Some(Self::Timer1),
            2 => Some(Self::Timer2),
            3 => Some(Self::Timer3),
            4 => Some(Self::Timer4),
            5 => Some(Self::Timer5),
            _ => None,
        }
    }
    const fn index(self) -> Index<{ oer_esp32s31_pac::coex::configuration::COUNT }> {
        match self {
            Self::Timer0 => Index::constant::<0>(),
            Self::Timer1 => Index::constant::<1>(),
            Self::Timer2 => Index::constant::<2>(),
            Self::Timer3 => Index::constant::<3>(),
            Self::Timer4 => Index::constant::<4>(),
            Self::Timer5 => Index::constant::<5>(),
        }
    }
}
pub struct CoexTimerBankRegisters<'a, P: Placement = Fixed<{ instances::coex::BASE }>> {
    timers: &'a mut CoexHwTimer<P>,
}
impl<'a, P: Placement> CoexTimerBankRegisters<'a, P> {
    pub(crate) fn new(timers: &'a mut CoexHwTimer<P>) -> Self {
        Self { timers }
    }
    // SOURCE: BLOB_LIBCOEX_HW_TIMER, S31_ESP_PACS_SVD. Fresh-read edges stay separate.
    pub fn enable(&mut self, timer: CoexTimerRegister) {
        self.timers.modify_disable_control(timer.index(), |w| {
            w.set_disable(false);
        });
        self.timers.modify_enable_control(timer.index(), |w| {
            w.set_enable(true);
        });
    }
    // SOURCE: BLOB_LIBCOEX_HW_TIMER, S31_ESP_PACS_SVD.
    pub fn disable(&mut self, timer: CoexTimerRegister) {
        self.timers.modify_enable_control(timer.index(), |w| {
            w.set_enable(false);
        });
        self.timers.modify_disable_control(timer.index(), |w| {
            w.set_disable(true);
        });
    }
    // SOURCE: BLOB_LIBCOEX_HW_TIMER. Only the 24-bit target changes.
    pub fn force(&mut self, timer: CoexTimerRegister) {
        self.set_primary_target(timer, 0);
    }
    // SOURCE: BLOB_LIBCOEX_HW_TIMER. Vendor unforce target is 1000 ticks.
    pub fn unforce(&mut self, timer: CoexTimerRegister) {
        self.set_primary_target(timer, 1000);
    }
    // SOURCE: BLOB_LIBCOEX_HW_TIMER. Client precedes PTI, each with a fresh read.
    pub fn configure(
        &mut self,
        timer: CoexTimerRegister,
        client: CoexTimerClientValue,
        pti: CoexTimerPtiValue,
    ) {
        self.timers.modify_configuration(timer.index(), |w| {
            w.set_parameter_1(Bits::<2>::truncate(client.get()));
        });
        self.timers.modify_configuration(timer.index(), |w| {
            w.set_parameter_2(Bits::<4>::truncate(pti.get()));
        });
    }
    // SOURCE: BLOB_LIBCOEX_HW_TIMER. The vendor narrows both targets to 24 bits.
    pub fn set_primary_target(&mut self, timer: CoexTimerRegister, tick_image: u32) {
        self.timers.modify_configuration(timer.index(), |w| {
            w.set_primary_tick_image(Bits::truncate(tick_image));
        });
    }
    pub fn set_secondary_target(&mut self, timer: CoexTimerRegister, tick_image: u32) {
        self.timers.modify_secondary_target(timer.index(), |w| {
            w.set_secondary_tick_image(Bits::truncate(tick_image));
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::vec;
    #[test]
    fn each_timer_keeps_the_whole_bank_except_its_fields() {
        for (fill, config, secondary) in [
            (0, 0x2734_5678, 0x00ab_cdef),
            (u32::MAX, 0xe734_5678, 0xffab_cdef),
        ] {
            for (index, timer) in CoexTimerRegister::ALL.into_iter().enumerate() {
                let mut memory = vec![fill; instances::coex::SIZE.div_ceil(4)];
                let mut expected = memory.clone();
                let mut registers = CoexHwTimer::host(&mut memory).expect("host bank fits");
                let mut bank = CoexTimerBankRegisters::new(&mut registers);
                bank.configure(
                    timer,
                    CoexTimerClientValue::new(2).expect("two-bit client"),
                    CoexTimerPtiValue::new(7).expect("four-bit priority"),
                );
                bank.set_primary_target(timer, 0x1234_5678);
                bank.set_secondary_target(timer, 0x89ab_cdef);
                bank.enable(timer);
                expected[index * 4] = config;
                expected[index * 4 + 1] = secondary;
                expected[index * 4 + 2] = if fill == 0 { 1 } else { u32::MAX };
                expected[index * 4 + 3] = if fill == 0 { 0 } else { 0xffff_fffe };
                drop(registers);
                assert_eq!(memory, expected);
            }
        }
    }
    #[test]
    fn forcing_and_unforcing_a_timer_preserves_client_and_priority() {
        let mut memory = vec![u32::MAX; instances::coex::SIZE.div_ceil(4)];
        let mut expected = memory.clone();
        let mut registers = CoexHwTimer::host(&mut memory).expect("host bank fits");
        let mut bank = CoexTimerBankRegisters::new(&mut registers);
        bank.force(CoexTimerRegister::Timer5);
        bank.unforce(CoexTimerRegister::Timer5);
        bank.disable(CoexTimerRegister::Timer5);
        expected[0x50 / 4] = 0xff00_03e8;
        expected[0x58 / 4] = 0xffff_fffe;
        drop(registers);
        assert_eq!(memory, expected);
    }
}
