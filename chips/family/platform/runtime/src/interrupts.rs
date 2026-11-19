//! The image's interrupt table: [`interrupt_table!`](crate::interrupt_table)
//! declares it; the hart's [`Router`](oer_chips_family_platform_interrupt_table::Router) binds its
//! entries, and the runtime's vectors call each entry's handler directly.

/// The CPU lines of the chip's CLIC and its harts, generated from the chip
/// facts' interrupt contract (`[interrupts] lines`, `first-source-line`,
/// `control-bits`, `level-bits` and `harts`).
#[cfg(chip_vectors)]
pub const LINES: oer_chips_family_platform_interrupt_table::Lines = crate::vectors::LINES;

/// Declare the image's interrupt table, once per image, and hand its
/// `INTERRUPT_TABLE` to [`adoption::run`](crate::adoption::run):
///
/// ```ignore
/// oer_chips_family_platform_runtime::interrupt_table! {
///     contexts { radio: 3; }
///     /// The IEEE 802.15.4 MAC's events.
///     ieee802154: Ieee802154Token = Ieee802154MacIrq => crate::radio::on_event,
///         line 20, context radio, hart 0, Hardware;
/// }
/// ```
///
/// Each entry names the token field, the token type, the interrupt part its
/// handler holds (its type names the source:
/// [`oer_portable_mmio::InterruptSource`]), the handler (`fn(&mut Part)`), the CPU
/// line ([`LINES`]: 16 to 47), a named context, the hart and the handler's class
/// (`oer_chips_family_platform_interrupt_table::Class`: `Hardware`, `Executor` or
/// `Exempt(VIOLATION, "reason")`), which the image gate checks. The macro
/// checks the declared low-to-high line order at each level against the
/// facts' tie-break rule and defines the line's handler symbol, which the line's vector calls, the
/// token type, and `Interrupts::take` that hands out each token once.
///
/// Level tasks come first, in `level_tasks { Task => handler, line LINE,
/// context NAME, hart HART; ... }` before `contexts`: each the one task of
/// its level on a software line of its own, which the hart's router binds
/// when it installs (`oer_chips_family_platform_interrupt_table::LevelTask`).
#[macro_export]
macro_rules! interrupt_table {
    ($($entries:tt)*) => {
        $crate::__interrupt_table::interrupt_table! {
            lines: $crate::interrupts::LINES,
            handler_attributes: [];
            $($entries)*
        }
    };
}

/// Retain a binding's invariant fault and reset with every setup, handler and
/// interrupted owner held, through the image's platform `P`. No owner
/// recovery or retry is issued.
#[cfg(all(target_arch = "riscv32", chip_vectors))]
pub fn reset_binding<
    P: oer_chips_family_platform_port::Platform,
    E: oer_chips_family_platform_interrupt_table::Entry,
>(
    held: oer_chips_family_platform_interrupt_table::Quarantined<E>,
) -> ! {
    let held = core::mem::ManuallyDrop::new(held);
    let (code, observations) = held.fault.diagnostic();
    crate::panic::reset_fault::<P>(code.into(), observations)
}
