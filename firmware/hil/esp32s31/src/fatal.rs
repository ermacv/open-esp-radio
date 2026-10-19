//! A fatal error's machine state, kept for the next boot.
//!
//! The panic hook and the exception entry record the hart's trap registers,
//! the vector it last took, the sources pending and routed to that vector and
//! its CLIC line in `.rtc_fast.persistent` without formatting: every step is
//! a register read through a typed accessor or a bounded copy, so the fatal
//! path stays a short leaf in every context's stack bound. The chip then
//! resets, and the next boot's [`report_previous`] prints the record once.

use esp_hal::system::Cpu;
use oer_platform_esp32s31_board::rom;
use oer_platform_staged_runtime::trap::{self, TrapRegisters};

const MAGIC: u32 = u32::from_le_bytes(*b"OFTL");
#[cfg(feature = "panic-hook")]
const PANIC: u32 = 1;
const EXCEPTION: u32 = 2;
/// The interrupt matrix's sources, in 32-bit words.
const SOURCE_WORDS: usize = 6;

/// The retained words: a magic, the record and a checksum over both. Any
/// bit pattern is a slot; an invalid one reports nothing.
#[derive(Clone, Copy, bytemuck::Zeroable, bytemuck::Pod)]
#[repr(C)]
struct Slot {
    magic: u32,
    kind: u32,
    hart: u32,
    mcause: u32,
    mepc: u32,
    mtval: u32,
    ra: u32,
    mscratch: u32,
    mtvec: u32,
    mtvt: u32,
    /// The handler MTVT holds for the vector in `mcause`.
    mtvt_slot: u32,
    /// The image's diagnostic stage, for a panic.
    stage: u32,
    /// The vector's CLIC line: bit 0 pending, 1 enabled, 2 edge-triggered,
    /// bits 8.. its level; zero when no source is routed to it.
    line: u32,
    pending: [u32; SOURCE_WORDS],
    /// The sources routed to the vector in `mcause` on this hart.
    routed: [u32; SOURCE_WORDS],
    checksum: u32,
}

impl Slot {
    #[inline(always)]
    fn sum(&self) -> u32 {
        let mut sum = 0x811c_9dc5_u32;
        let mut mix = |word: u32| sum = (sum ^ word).wrapping_mul(0x0100_0193);
        for word in [
            self.magic,
            self.kind,
            self.hart,
            self.mcause,
            self.mepc,
            self.mtval,
            self.ra,
            self.mscratch,
            self.mtvec,
            self.mtvt,
            self.mtvt_slot,
            self.stage,
            self.line,
        ]
        .into_iter()
        .chain(self.pending)
        .chain(self.routed)
        {
            mix(word);
        }
        sum
    }
}

oer_platform_staged_runtime::retained_static! {
    /// The fatal record. The fatal path takes it without waiting: while the
    /// other hart records its own failure, this one's is skipped.
    static SLOT: Slot;
}

oer_platform_staged_runtime::sram_fn! {
    in rwtext "fatal";
    /// Record a panic's machine state; the platform resets the chip next.
    #[cfg(feature = "panic-hook")]
    pub(crate) fn record_panic(stage: u32) {
        record(PANIC, 0, stage);
    }
}

oer_platform_staged_runtime::sram_fn! {
    in rwtext "fatal";
    /// Record an exception's machine state and its `ra`, then reset.
    pub(crate) fn record_exception(ra: u32) -> ! {
        record(EXCEPTION, ra, 0);
        esp_hal::system::software_reset()
    }
}

oer_platform_staged_runtime::sram_fn! {
    in rwtext "fatal";
    fn record(kind: u32, ra: u32, stage: u32) {
        let TrapRegisters {
            hart,
            mcause,
            mepc,
            mtval,
            mscratch,
            mtvec,
            mtvt,
        } = TrapRegisters::read();
        let vector = mcause & 0x0fff;
        let mtvt_slot = trap::current_hart_vector_slot(vector).unwrap_or(0);
        let mut pending = [0; SOURCE_WORDS];
        for source in esp_hal::interrupt::InterruptStatus::current().iterator() {
            set(&mut pending, usize::from(source));
        }
        let cpu = Cpu::current();
        let mut routed = [0; SOURCE_WORDS];
        let mut line = 0;
        for (source, cpu_interrupt) in esp_hal::interrupt::mapped_sources(cpu) {
            if cpu_interrupt as usize == vector {
                set(&mut routed, source);
                line = u32::from(cpu_interrupt.is_pending())
                    | u32::from(cpu_interrupt.is_enabled()) << 1
                    | u32::from(matches!(
                        cpu_interrupt.kind(),
                        esp_hal::interrupt::InterruptKind::Edge
                    )) << 2
                    | cpu_interrupt.level() << 8;
            }
        }
        // The other hart may hold any lock while this one fails, so the slot
        // is taken without waiting; when that hart is recording its own
        // failure, this one's is skipped.
        let Some(mut slot) = SLOT.try_lock() else {
            return;
        };
        *slot = Slot {
            magic: MAGIC,
            kind,
            hart: hart as u32,
            mcause: mcause as u32,
            mepc: mepc as u32,
            mtval: mtval as u32,
            ra,
            mscratch: mscratch as u32,
            mtvec: mtvec as u32,
            mtvt: mtvt as u32,
            mtvt_slot,
            stage,
            line,
            pending,
            routed,
            checksum: 0,
        };
        slot.checksum = slot.sum();
    }
}

/// Set `source`'s bit, without an index whose bounds check could fail: a
/// panic while recording would re-enter the fatal path.
#[inline(always)]
fn set(words: &mut [u32; SOURCE_WORDS], source: usize) {
    if let Some(word) = words.get_mut(source / 32) {
        *word |= 1 << (source % 32);
    }
}

/// Print and clear the record the previous boot left. Call once, early at
/// boot and before anything can fail.
pub(crate) fn report_previous() {
    let Some(mut slot) = SLOT.try_lock() else {
        return;
    };
    let valid = slot.magic == MAGIC && slot.checksum == slot.sum();
    slot.magic = 0;
    if !valid {
        return;
    }
    let [p0, p1, p2, p3, p4, p5] = slot.pending;
    let [r0, r1, r2, r3, r4, r5] = slot.routed;
    let Slot {
        hart,
        mcause,
        mepc,
        mtval,
        ra,
        mscratch,
        stage,
        line,
        mtvec,
        mtvt,
        mtvt_slot,
        ..
    } = *slot;
    if slot.kind == EXCEPTION {
        rom::print_fmt(format_args!(
            "OPEN_RADIO_HIL runtime=EXCEPTION boot=previous hart={hart} mcause={mcause:08x} mepc={mepc:08x} mtval={mtval:08x} ra={ra:08x} mscratch={mscratch:08x}\r\n"
        ));
    } else {
        rom::print_fmt(format_args!(
            "OPEN_RADIO_HIL runtime=PANIC boot=previous hart={hart} stage={stage} mcause={mcause:08x} mepc={mepc:08x} mtval={mtval:08x}\r\n"
        ));
    }
    rom::print_fmt(format_args!(
        "fatal irq_vector vector={} line={line:08x} mtvec={mtvec:08x} mtvt={mtvt:08x} slot={mtvt_slot:08x}\r\n",
        mcause & 0x0fff
    ));
    rom::print_fmt(format_args!(
        "fatal irq_pending words={p0:08x},{p1:08x},{p2:08x},{p3:08x},{p4:08x},{p5:08x}\r\n"
    ));
    rom::print_fmt(format_args!(
        "fatal irq_routed words={r0:08x},{r1:08x},{r2:08x},{r3:08x},{r4:08x},{r5:08x}\r\n"
    ));
}
