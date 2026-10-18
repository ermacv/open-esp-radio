//! A fatal error's machine state, kept for the next boot.
//!
//! The panic hook and the exception entry record the hart's trap registers,
//! the vector it last took, the sources pending and routed to that vector and
//! its CLIC line in `.rtc_fast.persistent` without formatting: every step is
//! a register read through a typed accessor or a bounded copy, so the fatal
//! path stays a short leaf in every context's stack bound. The chip then
//! resets, and the next boot's [`report_previous`] prints the record once.

use core::cell::UnsafeCell;
use core::ffi::c_char;

use esp_hal::system::Cpu;

const MAGIC: u32 = u32::from_le_bytes(*b"OFTL");
#[cfg(feature = "panic-hook")]
const PANIC: u32 = 1;
const EXCEPTION: u32 = 2;
/// The interrupt matrix's sources, in 32-bit words.
const SOURCE_WORDS: usize = 6;

/// The retained words: a magic, the record and a checksum over both.
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
    const EMPTY: Self = Self {
        magic: 0,
        kind: 0,
        hart: 0,
        mcause: 0,
        mepc: 0,
        mtval: 0,
        ra: 0,
        mscratch: 0,
        mtvec: 0,
        mtvt: 0,
        mtvt_slot: 0,
        stage: 0,
        line: 0,
        pending: [0; SOURCE_WORDS],
        routed: [0; SOURCE_WORDS],
        checksum: 0,
    };

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

struct Retained(UnsafeCell<Slot>);

// SAFETY: the fatal path writes the slot on its hart while the image stops;
// `report_previous` runs once at boot, before anything can fail.
#[allow(
    unsafe_code,
    reason = "a static in retained memory needs interior mutability"
)]
unsafe impl Sync for Retained {}

// Every bit pattern is a `Slot`; the NOLOAD section never loads the initializer.
#[allow(
    unsafe_code,
    reason = "placing the record in retained memory needs a link section"
)]
#[unsafe(link_section = ".rtc_fast.persistent")]
static SLOT: Retained = Retained(UnsafeCell::new(Slot::EMPTY));

/// Record a panic's machine state; the platform resets the chip next.
#[cfg(feature = "panic-hook")]
#[unsafe(link_section = ".rwtext.fatal")]
pub(crate) fn record_panic(stage: u32) {
    record(PANIC, 0, stage);
}

/// Record an exception's machine state and its `ra`, then reset.
#[unsafe(link_section = ".rwtext.fatal")]
pub(crate) fn record_exception(ra: u32) -> ! {
    record(EXCEPTION, ra, 0);
    esp_hal::system::software_reset()
}

#[unsafe(link_section = ".rwtext.fatal")]
fn record(kind: u32, ra: u32, stage: u32) {
    let (hart, mcause, mepc, mtval, mscratch, mtvec, mtvt): (
        usize,
        usize,
        usize,
        usize,
        usize,
        usize,
        usize,
    );
    // SAFETY: CSR reads only.
    #[allow(unsafe_code, reason = "the trap registers are CSRs")]
    unsafe {
        core::arch::asm!(
            "csrr {hart}, mhartid",
            "csrr {mcause}, mcause",
            "csrr {mepc}, mepc",
            "csrr {mtval}, mtval",
            "csrr {mscratch}, mscratch",
            "csrr {mtvec}, mtvec",
            "csrr {mtvt}, 0x307",
            hart = out(reg) hart,
            mcause = out(reg) mcause,
            mepc = out(reg) mepc,
            mtval = out(reg) mtval,
            mscratch = out(reg) mscratch,
            mtvec = out(reg) mtvec,
            mtvt = out(reg) mtvt,
            options(nomem, nostack),
        )
    };
    let vector = mcause & 0x0fff;
    // SAFETY: MTVT is the hart's 32-bit CLIC vector table in SRAM, and the
    // CLIC has 48 vectors; reading an entry calls nothing.
    #[allow(unsafe_code, reason = "MTVT is addressed by its CSR")]
    let mtvt_slot = if vector < 48 {
        unsafe { ((mtvt + vector * size_of::<u32>()) as *const u32).read_volatile() }
    } else {
        0
    };
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
    // SAFETY: the other hart may hold any lock while this one fails, so the
    // slot is written without one; the image stops after this write.
    #[allow(unsafe_code, reason = "a fatal error cannot wait for a lock")]
    let slot = unsafe { &mut *SLOT.0.get() };
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

/// Set `source`'s bit, without an index whose bounds check could fail: a
/// panic while recording would re-enter the fatal path.
#[inline(always)]
fn set(words: &mut [u32; SOURCE_WORDS], source: usize) {
    if let Some(word) = words.get_mut(source / 32) {
        *word |= 1 << (source % 32);
    }
}

unsafe extern "C" {
    fn ets_printf(format: *const c_char, ...) -> i32;
}

/// Print and clear the record the previous boot left. Call once, early at
/// boot and before anything can fail.
pub(crate) fn report_previous() {
    // SAFETY: called once at boot, before the fatal path can write the slot.
    #[allow(unsafe_code, reason = "the retained record is a raw static")]
    let slot = unsafe { &mut *SLOT.0.get() };
    let valid = slot.magic == MAGIC && slot.checksum == slot.sum();
    slot.magic = 0;
    if !valid {
        return;
    }
    let [p0, p1, p2, p3, p4, p5] = slot.pending;
    let [r0, r1, r2, r3, r4, r5] = slot.routed;
    // SAFETY: every format names as many arguments as it is given.
    #[allow(unsafe_code, reason = "the ROM's printf is a C variadic")]
    unsafe {
        if slot.kind == EXCEPTION {
            ets_printf(
                c"OPEN_RADIO_HIL runtime=EXCEPTION boot=previous hart=%u mcause=%08x mepc=%08x mtval=%08x ra=%08x mscratch=%08x\r\n".as_ptr(),
                slot.hart, slot.mcause, slot.mepc, slot.mtval, slot.ra, slot.mscratch,
            );
        } else {
            ets_printf(
                c"OPEN_RADIO_HIL runtime=PANIC boot=previous hart=%u stage=%u mcause=%08x mepc=%08x mtval=%08x\r\n".as_ptr(),
                slot.hart, slot.stage, slot.mcause, slot.mepc, slot.mtval,
            );
        }
        ets_printf(
            c"fatal irq_vector vector=%u line=%08x mtvec=%08x mtvt=%08x slot=%08x\r\n".as_ptr(),
            slot.mcause & 0x0fff,
            slot.line,
            slot.mtvec,
            slot.mtvt,
            slot.mtvt_slot,
        );
        ets_printf(
            c"fatal irq_pending words=%08x,%08x,%08x,%08x,%08x,%08x\r\n".as_ptr(),
            p0,
            p1,
            p2,
            p3,
            p4,
            p5,
        );
        ets_printf(
            c"fatal irq_routed words=%08x,%08x,%08x,%08x,%08x,%08x\r\n".as_ptr(),
            r0,
            r1,
            r2,
            r3,
            r4,
            r5,
        );
    }
}
