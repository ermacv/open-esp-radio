//! The chip-neutral part of the interrupt contract the image's
//! interrupt-stack gate checks: where the stage-two runtime puts the image's
//! interrupt table, vectors and dispatcher, and the guard of each interrupt
//! stack. The chip's harts, levels and level writers are chip data
//! (`platform/<chip>/chip.toml` `[interrupts]`).
//!
//! The trap entries learn the interrupt stack as the lower of `sp` and
//! `mscratch` because every interrupt stack lies in SRAM, below every task
//! stack in PSRAM; [`Layout::check`](crate::Layout::check) keeps that true of
//! every chip's map.

/// The exported slice of the image's interrupt table.
pub const TABLE_SYMBOL: &str = "__OER_INTERRUPT_TABLE";
/// Bytes of one entry, `Binding<Interrupt, Priority, Cpu>`, whose field
/// offsets the interrupt-table adapter asserts: source `u16` at 0, level
/// `u8` at 2, core `u32` at 4, handler word at 8.
pub const TABLE_ENTRY_BYTES: u32 = 12;
pub const TABLE_SOURCE: (u32, u32) = (0, 2);
pub const TABLE_LEVEL: (u32, u32) = (2, 1);
pub const TABLE_CORE: (u32, u32) = (4, 4);
pub const TABLE_HANDLER: u32 = 8;

/// The source of the hardware-vector entries every hart's MTVT holds.
pub const VECTOR_TABLE_SYMBOL: &str = "_runtime_psram_mtvt_source";
/// The entry of synchronous exceptions.
pub const EXCEPTION_ENTRY_SYMBOL: &str = "_start_trap";
/// esp-hal's per-source handler table, through which the dispatcher calls.
pub const SOURCE_TABLE_SYMBOL: &str = "__EXTERNAL_INTERRUPTS";

/// Aligned bytes at the bottom of each interrupt stack whose stores trap.
pub const IRQ_STACK_GUARD_BYTES: u32 = 1024;
/// The margin the interrupt-stack bound keeps below the usable stack, in
/// percent of the bound.
pub const IRQ_STACK_MARGIN_PERCENT: u32 = 10;
