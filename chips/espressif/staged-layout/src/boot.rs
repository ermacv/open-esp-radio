//! Which hart runs the staged boot. ROM releases a core other than hart 0 only
//! through an explicit boot address. SOURCE: pinned S31 rev0 ROM
//! normal_boot @0x2f801790 and main @0x2f804652. A stale address can hand it to the
//! bootstrap or the runtime, so both entries admit only the boot hart and stop
//! any other before it touches memory. A secondary hart enters only through
//! its own generated entry (`handoff::SecondaryEntry`), which the runtime's
//! launch writes to its boot word.
//!
//! SOURCE: chips/esp32s31/platform/README.md#secondary-hart-launch-and-containment.

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
