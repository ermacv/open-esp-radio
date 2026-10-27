//! Which hart runs the staged boot. ROM releases a core other than hart 0 only
//! through an explicit boot address ([`chips/esp32s31/notes/core1-boot.md`]);
//! a stale address left by an earlier image would hand such a hart to the
//! bootstrap or the runtime, so both stop any hart but this one before it
//! touches memory.
//!
//! [`chips/esp32s31/notes/core1-boot.md`]: https://github.com/ermacv/open-esp-radio/blob/main/chips/esp32s31/notes/core1-boot.md

/// The bootstrap's admitted boot hart. Every facts-generated layout const-checks
/// this value against its boot-hart fact; a disagreement fails compilation.
/// Bootstrap entry compares `mhartid` with this value, while runtime entry uses
/// the same checked fact through its generated `BOOT_HART`.
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
