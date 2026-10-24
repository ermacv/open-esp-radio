//! Which hart runs the staged boot. ROM releases a core other than hart 0 only
//! through an explicit boot address ([`chips/esp32s31/notes/core1-boot.md`]);
//! a stale address left by an earlier image would hand such a hart to the
//! bootstrap or the runtime, so both stop any hart but this one before it
//! touches memory.
//!
//! [`chips/esp32s31/notes/core1-boot.md`]: https://github.com/ermacv/open-esp-radio/blob/main/chips/esp32s31/notes/core1-boot.md

/// The one hart that runs the bootstrap and the runtime. The runtime's entry
/// assembly (`chips/espressif/staged-runtime/src/entry.rs`) compares `mhartid`
/// with zero.
pub const BOOT_HART: usize = 0;

/// Whether `hart` may run the staged boot.
pub const fn may_boot(hart: usize) -> bool {
    hart == BOOT_HART
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_boot_hart_boots() {
        assert!(may_boot(BOOT_HART));
        assert!(!may_boot(BOOT_HART + 1));
        assert!(!may_boot(usize::MAX));
    }
}
