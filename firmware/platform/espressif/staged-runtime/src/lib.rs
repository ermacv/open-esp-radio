#![no_std]
//! Stage-two entry, relocation and interrupt-stack ownership of the
//! Espressif staged boot.
//!
//! The bootstrap copies stage two into PSRAM and jumps to `_runtime_start`
//! ([`entry`](crate::entry)), which initializes the SRAM sections, switches to
//! the CPU0 PSRAM task stack, installs the vectors and enters the image's
//! `runtime_main`. The image adopts the bootstrap's PSRAM mapping through its
//! board (`platform/<chip>/board`), which calls [`adopt`], and enables
//! interrupts with [`enable_interrupts_after_handoff`] once its handlers are
//! bound. A chip feature selects the chip of esp-hal; `two-harts` adds the
//! second hart's interrupt stack.
mod entry;
pub mod interrupts;
pub mod panic;
pub mod stacks;

#[doc(hidden)]
pub use oer_interrupt_table as __interrupt_table;

/// The image's interrupt table as [`interrupt_table!`] declares it.
pub type InterruptTable =
    oer_interrupt_table::Table<oer_espressif_interrupt_table_esp_hal::EspHalMatrix>;

/// Adopt the bootstrap's PSRAM mapping with `adopt_mapping` and install the
/// stage-two interrupt context: the image's interrupt table, esp-hal's
/// vectoring and the current hart's SRAM interrupt stack, and the task-stack
/// watchpoint.
///
/// `interrupt_table` is the image's `INTERRUPT_TABLE` ([`interrupt_table!`]):
/// every hart installs and checks it.
///
/// # Safety
/// Call once on CPU0 after `_runtime_start`, with interrupts disabled and the
/// bootstrap's board mapping intact; `adopt_mapping` must adopt that mapping
/// without resetting or remapping PSRAM, which code and stacks already run
/// from. Keep interrupts disabled until the application's timer and executor
/// handlers have been bound.
pub unsafe fn adopt<T>(
    interrupt_table: &'static InterruptTable,
    adopt_mapping: impl FnOnce() -> T,
) -> T {
    oer_espressif_interrupt_table_esp_hal::adopt(interrupt_table);
    let mapping = adopt_mapping();
    // SAFETY: the caller keeps interrupts disabled on the only running hart,
    // and no driver relies on the bootloader's interrupt mappings yet.
    unsafe { esp_hal::interrupt::reinitialize_vectoring_after_handoff() };
    // SAFETY: interrupts are disabled and this hart has not admitted one.
    unsafe { stacks::install_current_hart_interrupt_stack() };
    unsafe extern "C" {
        static _stack_end: u8;
    }
    // SAFETY: `_stack_end` is the word-aligned bottom of the CPU0 task stack
    // the runtime linker script places.
    unsafe { esp_hal::debugger::set_stack_watchpoint(core::ptr::addr_of!(_stack_end) as usize) };
    mapping
}

/// Hand global interrupt enable (`mstatus.MIE`) to the current hart's executor.
///
/// The staged handoff keeps MIE clear, so interrupt handlers cannot run
/// against unbound timer, executor or stack state.
///
/// # Safety
/// Call once per hart, after its interrupt vectors, interrupt stack, timer and
/// executor handlers are bound. MIE must have stayed clear since that hart
/// entered the runtime. Earlier enabling can dispatch an interrupt into
/// uninitialized ownership state.
///
/// # Panics
/// When a hardware-vector slot of the hart's active MTVT is not the runtime's
/// stack-switching entry, or the interrupt matrix disagrees with the image's
/// interrupt table ([`interrupts`]).
pub unsafe fn enable_interrupts_after_handoff() {
    stacks::verify_current_hart_vectors();
    oer_espressif_interrupt_table_esp_hal::verify_current_hart();
    // SAFETY: the caller guarantees that every handler this hart can dispatch
    // is bound; setting MIE touches no memory and leaves the stack unchanged.
    unsafe { core::arch::asm!("csrsi mstatus, 8", options(nomem, nostack)) };
}

/// The stack-protector guard that `esp_hal::init` arms its watchpoint on.
/// esp-hal's `linkall.x` defines it; stage two links its own scripts, so the
/// runtime owns it, in SRAM critical data.
#[unsafe(no_mangle)]
#[unsafe(link_section = ".critical.data.stack_guard")]
static mut __stack_chk_guard: u32 = 0xDEED_BAAD;
