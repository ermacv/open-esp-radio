//! Open the three access-path filters; preserve the bootloader's PMS/PMA.
//! SOURCE: esp-hal 1f646c5 `soc/mod.rs:81-565`; ESP-IDF 4d80c1b2
//! `hp_apm_reg.h:1947-2013`, `lp_apm_reg.h:987-1026`, `hp_mem_apm_reg.h:1211-1269`.
use crate::Access;

pub trait Registers {
    fn open_lp_filter(&mut self, access: Access<'_>);
    fn open_hp_filter(&mut self, access: Access<'_>);
    fn open_memory_filter(&mut self, access: Access<'_>);
}

pub fn open<R: Registers>(registers: &mut R) {
    registers.open_lp_filter(Access::new());
    registers.open_hp_filter(Access::new());
    registers.open_memory_filter(Access::new());
}
