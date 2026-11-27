//! The coexistence timer the PHY's grant protect programs (timer 5),
//! borrowed apart from the PHY.
#![forbid(unsafe_code)]
use crate::radio::modem::values::{CoexTimerClientValue, CoexTimerPtiValue};
use oer_chips_esp32s31_pac::{coex::PhyGrantProtectTimer, instances};
use oer_portable_mmio::{Fixed, Placement};

/// The grant-protect timer's words.
pub struct CoexGrantTimerRegisters<'a, P: Placement = Fixed<{ instances::coex::BASE }>> {
    grant: &'a mut PhyGrantProtectTimer<P>,
}
impl<'a, P: Placement> CoexGrantTimerRegisters<'a, P> {
    pub(crate) fn new(grant: &'a mut PhyGrantProtectTimer<P>) -> Self {
        Self { grant }
    }
    // SOURCE: BLOB_LIBCOEX_HW_TIMER, S31_ESP_PACS_SVD. Fresh-read edges stay separate.
    pub fn enable(&mut self) {
        self.grant.modify_disable_control5(|w| {
            w.set_disable(false);
        });
        self.grant.modify_enable_control5(|w| {
            w.set_enable(true);
        });
    }
    // SOURCE: BLOB_LIBCOEX_HW_TIMER, S31_ESP_PACS_SVD.
    pub fn disable(&mut self) {
        self.grant.modify_enable_control5(|w| {
            w.set_enable(false);
        });
        self.grant.modify_disable_control5(|w| {
            w.set_disable(true);
        });
    }
    // SOURCE: BLOB_LIBCOEX_HW_TIMER. Client precedes PTI, each with a fresh read.
    pub fn configure(&mut self, client: CoexTimerClientValue, pti: CoexTimerPtiValue) {
        self.grant.modify_configuration5(|w| {
            w.set_parameter_1(client.field());
        });
        self.grant.modify_configuration5(|w| {
            w.set_parameter_2(pti.field());
        });
    }
    // SOURCE: BLOB_LIBCOEX_HW_TIMER. The vendor narrows both targets to 24 bits.
    pub fn set_primary_target(&mut self, tick_image: u32) {
        self.grant.modify_configuration5(|w| {
            w.set_primary_tick_image_low_bits(tick_image);
        });
    }
    pub fn set_secondary_target(&mut self, tick_image: u32) {
        self.grant.modify_secondary_target5(|w| {
            w.set_secondary_tick_image_low_bits(tick_image);
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::vec;
    #[test]
    fn the_grant_timer_keeps_the_whole_bank_except_its_fields() {
        for (fill, config, secondary) in [
            (0, 0x2734_5678, 0x00ab_cdef),
            (u32::MAX, 0xe734_5678, 0xffab_cdef),
        ] {
            let mut grant_memory = vec![fill; instances::coex::SIZE.div_ceil(4)];
            let mut expected = grant_memory.clone();
            let mut grant = PhyGrantProtectTimer::host(&mut grant_memory).expect("host bank fits");
            let mut timer = CoexGrantTimerRegisters::new(&mut grant);
            timer.configure(
                CoexTimerClientValue::new(2).expect("two-bit client"),
                CoexTimerPtiValue::new(7).expect("four-bit priority"),
            );
            timer.set_primary_target(0x1234_5678);
            timer.set_secondary_target(0x89ab_cdef);
            timer.enable();
            let index = 5;
            expected[index * 4] = config;
            expected[index * 4 + 1] = secondary;
            expected[index * 4 + 2] = if fill == 0 { 1 } else { u32::MAX };
            expected[index * 4 + 3] = if fill == 0 { 0 } else { 0xffff_fffe };
            drop(grant);
            assert_eq!(grant_memory, expected);
        }
    }
}
