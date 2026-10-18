//! RV32 high/low/high snapshots with compiler memory barriers.

use core::{
    arch::asm,
    sync::atomic::{Ordering, compiler_fence},
};

#[derive(Clone, Copy, Default)]
pub(super) struct Counters {
    pub(super) cycles: u64,
    pub(super) instructions: u64,
}

impl Counters {
    #[inline(always)]
    pub(super) fn read() -> Self {
        compiler_fence(Ordering::SeqCst);
        let cycles = loop {
            let (high, low, check): (u32, u32, u32);
            // SAFETY: these read-only standard counters are available in the
            // runtime privilege mode. Retry handles rollover during RV32 reads.
            unsafe {
                asm!("rdcycleh {high}", "rdcycle {low}", "rdcycleh {check}",
                high = out(reg) high, low = out(reg) low, check = out(reg) check,
                options(nostack))
            };
            if high == check {
                break (u64::from(high) << 32) | u64::from(low);
            }
        };
        let instructions = loop {
            let (high, low, check): (u32, u32, u32);
            // SAFETY: same read-only counter contract as above.
            unsafe {
                asm!("rdinstreth {high}", "rdinstret {low}", "rdinstreth {check}",
                high = out(reg) high, low = out(reg) low, check = out(reg) check,
                options(nostack))
            };
            if high == check {
                break (u64::from(high) << 32) | u64::from(low);
            }
        };
        compiler_fence(Ordering::SeqCst);
        Self {
            cycles,
            instructions,
        }
    }

    pub(super) fn since(self, earlier: Self) -> Self {
        Self {
            cycles: self.cycles.wrapping_sub(earlier.cycles),
            instructions: self.instructions.wrapping_sub(earlier.instructions),
        }
    }

    pub(super) fn add(&mut self, other: Self) {
        self.cycles += other.cycles;
        self.instructions += other.instructions;
    }
}

/// Use the same memory-ordering boundary as the production DMA publication.
/// This orders memory accesses; it does not write back dirty PSRAM cache lines.
#[inline(always)]
pub(super) fn memory_fence() {
    compiler_fence(Ordering::SeqCst);
    // SAFETY: the standard fence orders this hart's memory accesses and does
    // not access an address or alter peripheral ownership.
    unsafe {
        asm!("fence rw, rw", options(nostack));
    }
    compiler_fence(Ordering::SeqCst);
}
