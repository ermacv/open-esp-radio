//! The image's `critical-section` implementation: a hart-local section
//! that masks every interrupt of the current hart.
//!
//! Acquiring raises the CLIC threshold (`mintthresh`) to its maximum, then
//! clears `mstatus.MIE`; the token keeps the raw threshold and whether MIE
//! was set. Releasing, in reverse order of acquiring, sets MIE again when it
//! was set and then writes the raw threshold back, so nested sections return
//! the hart to exactly the state the outermost one found; the threshold is
//! never rebuilt from the running level. The three instructions after the
//! release give a pending interrupt its window.
//!
//! The section is hart-local and takes no lock, so only one hart may run
//! code: an image whose hart 1 runs (`cpu1-runs`) does not build until a
//! cross-hart lock without spinning exists. A second hart of the chip's
//! facts alone has its interrupt stack and vectors and starts nothing.
//!
//! SOURCE: esp-hal 1f646c5 `esp-sync/src/raw.rs` (`SingleCoreInterruptLock`:
//! the raised threshold, the token's layout, the order of release and the
//! three `nop`s); ESP-IDF 4d59230d `riscv/include/esp_private/interrupt_clic.h`
//! (`rv_utils_set_intlevel_regval`).

// Hart-local masking excludes nothing on a second running hart: two harts
// could borrow one `critical_section::Mutex<RefCell<_>>` at once.
#[cfg(feature = "cpu1-runs")]
compile_error!(
    "the runtime's critical-section masks only the current hart; an image whose hart 1 runs code needs a cross-hart lock without spinning"
);

/// The hart's interrupt mask, as a section changes it.
pub(crate) trait Mask {
    /// Write `threshold` and return the previous raw threshold.
    fn swap_threshold(&mut self, threshold: u8) -> u8;
    /// Clear MIE and return whether it was set.
    fn take_interrupts(&mut self) -> bool;
    /// Set MIE.
    fn give_interrupts(&mut self);
    /// Write the raw threshold `threshold`.
    fn restore_threshold(&mut self, threshold: u8);
}

/// The token's bit for MIE; the raw threshold sits in bits 8..16.
const MIE: u32 = 1 << 3;

/// Acquire: mask every level by the threshold, then clear MIE.
pub(crate) fn acquire(mask: &mut impl Mask) -> u32 {
    let threshold = mask.swap_threshold(u8::MAX);
    let enabled = mask.take_interrupts();
    (u32::from(threshold) << 8) | if enabled { MIE } else { 0 }
}

/// Release `token`, the last unreleased one [`acquire`] returned on this
/// hart: MIE first, then the raw threshold.
pub(crate) fn release(mask: &mut impl Mask, token: u32) {
    if token & MIE != 0 {
        mask.give_interrupts();
    }
    mask.restore_threshold(token.to_le_bytes()[1]);
}

#[cfg(target_arch = "riscv32")]
pub(crate) mod hart {
    use core::sync::atomic::{Ordering, compiler_fence};

    /// The current hart's CSRs.
    struct Csrs;

    impl super::Mask for Csrs {
        #[inline(always)]
        fn swap_threshold(&mut self, threshold: u8) -> u8 {
            oer_riscv::swap_interrupt_threshold(threshold)
        }

        #[inline(always)]
        fn take_interrupts(&mut self) -> bool {
            oer_riscv::take_interrupts()
        }

        #[inline(always)]
        fn give_interrupts(&mut self) {
            oer_riscv::enable_interrupts();
        }

        #[inline(always)]
        fn restore_threshold(&mut self, threshold: u8) {
            oer_riscv::restore_interrupt_threshold(threshold);
        }
    }

    /// Mask every interrupt of the current hart for good, as a section
    /// acquires (the threshold first, then MIE), for a fatal path that
    /// never releases it.
    #[inline(always)]
    pub(crate) fn mask_for_good() {
        let _ = super::acquire(&mut Csrs);
        // No access of the fatal path moves before the mask.
        compiler_fence(Ordering::SeqCst);
    }

    struct Section;
    critical_section::set_impl!(Section);

    // SAFETY: `acquire` masks every interrupt of the hart and returns the
    // state `release` restores; `critical-section` releases in reverse order
    // on the acquiring hart, and no other hart runs code (`cpu1-runs` does
    // not build).
    unsafe impl critical_section::Impl for Section {
        unsafe fn acquire() -> critical_section::RawRestoreState {
            let token = super::acquire(&mut Csrs);
            // No access of the section moves before the mask.
            compiler_fence(Ordering::SeqCst);
            token
        }

        unsafe fn release(token: critical_section::RawRestoreState) {
            // No access of the section moves after the unmask.
            compiler_fence(Ordering::SeqCst);
            super::release(&mut Csrs, token);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Mask, acquire, release};

    /// A hart's MIE and raw threshold, and the order of their writes.
    struct Fake {
        mie: bool,
        threshold: u8,
        writes: std::vec::Vec<&'static str>,
    }

    impl Mask for Fake {
        fn swap_threshold(&mut self, threshold: u8) -> u8 {
            self.writes.push("threshold");
            core::mem::replace(&mut self.threshold, threshold)
        }
        fn take_interrupts(&mut self) -> bool {
            self.writes.push("clear mie");
            core::mem::replace(&mut self.mie, false)
        }
        fn give_interrupts(&mut self) {
            self.writes.push("set mie");
            self.mie = true;
        }
        fn restore_threshold(&mut self, threshold: u8) {
            self.writes.push("threshold");
            self.threshold = threshold;
        }
    }

    #[test]
    fn nested_sections_return_the_raw_threshold_and_mie_in_reverse_order() {
        // A threshold between levels, as a task may hold one.
        let mut hart = Fake {
            mie: true,
            threshold: 0x3f,
            writes: std::vec::Vec::new(),
        };
        let outer = acquire(&mut hart);
        assert_eq!((hart.mie, hart.threshold), (false, 0xff));
        let inner = acquire(&mut hart);
        release(&mut hart, inner);
        // The inner release finds MIE clear and leaves it so.
        assert_eq!((hart.mie, hart.threshold), (false, 0xff));
        release(&mut hart, outer);
        assert_eq!((hart.mie, hart.threshold), (true, 0x3f));
        assert_eq!(
            hart.writes,
            [
                "threshold",
                "clear mie",
                "threshold",
                "clear mie",
                "threshold",
                "set mie",
                "threshold"
            ]
        );
    }

    #[test]
    fn a_section_entered_with_interrupts_masked_leaves_them_masked() {
        let mut hart = Fake {
            mie: false,
            threshold: 0,
            writes: std::vec::Vec::new(),
        };
        let token = acquire(&mut hart);
        release(&mut hart, token);
        assert_eq!((hart.mie, hart.threshold), (false, 0));
        assert!(!hart.writes.contains(&"set mie"));
    }
}
