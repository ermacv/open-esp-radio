//! `tcontrol` across nested traps. A trap into machine mode copies MTE to
//! MPTE and clears MTE; `mret` copies MPTE back to MTE. A nested trap thus
//! overwrites the MPTE its outer `mret` needs, so every entry of
//! `vectors.s` keeps the trap's `tcontrol` in its frame and the exit writes
//! it back before `mret`. An interrupt's entry sets MTE from MPTE, so its
//! handler runs with the triggers of the context it interrupted.
//!
//! SOURCE: RISC-V Debug Specification (Sdtrig), "Trigger Control (tcontrol,
//! at 0x7a5)"; ESP-IDF 4d59230d `riscv/include/riscv/rv_utils.h`
//! (`rv_utils_set_watchpoint` programs MTE and MPTE).

use oer_riscv::trigger::{TCONTROL_MPTE, TCONTROL_MTE};

/// The bits from MPTE down to MTE: the entry's shift of the trap's
/// `tcontrol` before it keeps MTE alone.
pub(crate) const MPTE_SHIFT: u32 = TCONTROL_MPTE.trailing_zeros() - TCONTROL_MTE.trailing_zeros();

#[cfg(test)]
mod tests {
    use super::{MPTE_SHIFT, TCONTROL_MPTE, TCONTROL_MTE};

    /// A hart's `tcontrol` and the frames of its nested traps, with the
    /// hardware's trap and `mret` and the steps of the runtime's entry and
    /// exit (`vectors.s`).
    struct Hart {
        tcontrol: usize,
        frames: std::vec::Vec<usize>,
        /// Whether the entry and exit keep `tcontrol` (`vectors.s`), or
        /// leave it to the hardware alone.
        keeps: bool,
    }

    impl Hart {
        fn new(mte: bool, keeps: bool) -> Self {
            Self {
                tcontrol: if mte { TCONTROL_MTE } else { 0 },
                frames: std::vec::Vec::new(),
                keeps,
            }
        }

        fn mte(&self) -> bool {
            self.tcontrol & TCONTROL_MTE != 0
        }

        /// The trap, then the interrupt's entry up to its handler's call.
        fn interrupt(&mut self) {
            let mpte = if self.mte() { TCONTROL_MPTE } else { 0 };
            self.tcontrol = mpte;
            // `csrr; sw 88(sp)`
            self.frames.push(self.tcontrol);
            if self.keeps {
                // `csrr; srli {mpte_shift}; andi {mte}; csrs`
                self.tcontrol |= (self.tcontrol >> MPTE_SHIFT) & TCONTROL_MTE;
            }
        }

        /// The exit after the handler's return, then `mret`.
        fn exit(&mut self) {
            let saved = self.frames.pop().expect("an exit follows its entry");
            if self.keeps {
                // `lw 88(sp); csrw`
                self.tcontrol = saved;
            }
            let mte = if self.tcontrol & TCONTROL_MPTE != 0 {
                TCONTROL_MTE
            } else {
                0
            };
            self.tcontrol = (self.tcontrol & !TCONTROL_MTE) | mte;
        }
    }

    #[test]
    fn nested_returns_give_each_level_back_its_triggers() {
        for task_mte in [true, false] {
            let mut hart = Hart::new(task_mte, true);
            hart.interrupt();
            // The outer handler watches as the task did.
            assert_eq!(hart.mte(), task_mte);
            hart.interrupt();
            assert_eq!(hart.mte(), task_mte);
            hart.exit();
            // Back in the outer handler.
            assert_eq!(hart.mte(), task_mte);
            hart.exit();
            assert_eq!(hart.mte(), task_mte);
            assert!(hart.frames.is_empty());
        }
    }

    #[test]
    fn left_to_the_hardware_a_nested_return_loses_the_task_s_triggers() {
        let mut hart = Hart::new(true, false);
        hart.interrupt();
        // The handler runs unguarded, and the nested trap moves its clear
        // MTE over the task's MPTE.
        assert!(!hart.mte());
        hart.interrupt();
        hart.exit();
        hart.exit();
        assert!(!hart.mte());
    }
}
