//! Boot containment of the profile's secondary harts.
//! SOURCE: esp-hal 1f646c5 `soc/esp32s31/cpu_control.rs:6-96`;
//! ESP-IDF 4d80c1b2 `system_internal.c:61-100`.
use crate::Access;

pub trait Registers {
    type Secondary: Copy + 'static;
    const SECONDARIES: &'static [Self::Secondary];
    fn request_stall(&mut self, access: Access<'_>, hart: Self::Secondary);
    fn hold(&mut self, access: Access<'_>, hart: Self::Secondary);
    fn clear_boot_address(&mut self, access: Access<'_>, hart: Self::Secondary);
}

/// Stall, gate and hold each secondary, then clear its ROM entry.
pub fn contain<R: Registers>(registers: &mut R) {
    for &hart in R::SECONDARIES {
        registers.request_stall(Access::new(), hart);
        registers.hold(Access::new(), hart);
        registers.clear_boot_address(Access::new(), hart);
    }
}
