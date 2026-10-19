#![no_std]
//! The chip-neutral steps of the staged-boot bootstrap.
//!
//! Each chip's bootstrap (`platform/<chip>/bootstrap`) is a Flash-resident
//! ESP-IDF application that carries the packed stage-two runtime as Flash
//! read-only data. After it initialized esp-hal and its board's PSRAM it calls
//! [`stage`], which checks the payload's header and checksum, copies it to its
//! PSRAM load address, clears its PSRAM BSS and checks the copy; the chip may
//! then retune its Flash, and [`hand_off`] publishes the copied code to
//! instruction fetch and enters it. Every step reports an `OER_BOOT
//! bootstrap=` line through the ROM's `ets_printf`, which the host's boot
//! observation reads; a failure halts the hart with its reason.
//!
//! The library also owns the bootstrap's panic entry: a panic prints
//! `OER_BOOT bootstrap=PANIC` and halts.

use core::{arch::asm, ffi::CStr, mem::size_of, ptr};

use oer_espressif_staged_layout::{
    Layout,
    stage_two::{Header as RuntimeHeader, PayloadCrc},
};

unsafe extern "C" {
    fn ets_install_usb_printf();
    fn ets_printf(format: *const core::ffi::c_char, ...);
    static __stack_chk_guard: u32;
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo<'_>) -> ! {
    print(c"OER_BOOT bootstrap=PANIC\r\n");
    halt()
}

/// Route the ROM's `ets_printf` to the USB Serial/JTAG console and report the
/// bootstrap's start. Call first.
pub fn start() {
    // SAFETY: the ROM function only selects the console of its own printf.
    unsafe { ets_install_usb_printf() };
    print(c"OER_BOOT bootstrap=START\r\n");
}

/// A stage-two payload that [`stage`] copied and checked in PSRAM.
#[derive(Clone, Copy, Debug)]
pub struct Staged {
    load_address: usize,
    entry: usize,
    payload_len: usize,
}

impl Staged {
    /// Where the payload lies in PSRAM.
    pub fn load_address(&self) -> usize {
        self.load_address
    }

    pub fn payload_len(&self) -> usize {
        self.payload_len
    }
}

/// Check `payload` (the packed runtime the bootstrap carries in Flash) against
/// `layout` and the initialized PSRAM `psram_base..psram_base + psram_size`,
/// copy it to its load address, clear its PSRAM BSS and check the copy.
///
/// Reports `PSRAM` after the PSRAM probe, `SOURCE_CRC` after the payload's
/// checksum and `DESTINATION_CRC` after the copy's; halts with the reason
/// otherwise.
///
/// The numeric checks against `layout` (whose fields are public and
/// mutable) do not prove that the region exists or may be written: the
/// caller does.
///
/// # Safety
///
/// `psram_base..psram_base + psram_size` must be an initialized, mapped,
/// readable and writable region of `psram_size` bytes, aligned for `u32`
/// at `psram_base`, owned exclusively by the caller: no reference or other
/// pointer in use aliases any byte of it, and nothing else reads or writes
/// it until the runtime this hands off to owns it. The mapping must stay in
/// place, unchanged, until [`hand_off`] entered the runtime.
pub unsafe fn stage(
    layout: &Layout,
    payload: &'static [u8],
    psram_base: *mut u8,
    psram_size: usize,
) -> Staged {
    if psram_base as usize != layout.psram.origin as usize
        || psram_size != layout.psram.length as usize
    {
        fail(c"OER_BOOT bootstrap=FAIL reason=psram-init\r\n");
    }
    // SAFETY: `stage`'s `# Safety` contract: the whole region, the probe
    // page included, is initialized, writable and exclusively the caller's.
    unsafe { verify_psram_probe(psram_base) };
    print(c"OER_BOOT bootstrap=PSRAM\r\n");

    let source = payload.as_ptr();
    let source_address = source as usize;
    let xip = layout.flash_xip;
    let source_end = source_address
        .checked_add(payload.len())
        .unwrap_or_else(|| fail(c"OER_BOOT bootstrap=FAIL reason=source-overflow\r\n"));
    if !(xip.origin as usize..xip.end() as usize).contains(&source_address)
        || source_end > xip.end() as usize
    {
        fail(c"OER_BOOT bootstrap=FAIL reason=source-not-xip\r\n");
    }

    let header = read_header(payload);
    let layout = validate_header(header, layout, psram_base as usize, psram_size);
    if layout.payload_len != payload.len() {
        fail(c"OER_BOOT bootstrap=FAIL reason=payload-length\r\n");
    }
    let source_crc = payload_crc32(source, layout.payload_len);
    if source_crc != layout.expected_crc32 {
        print_crc_failure(c"source-crc", layout.expected_crc32, source_crc);
    }
    print(c"OER_BOOT bootstrap=SOURCE_CRC\r\n");

    // SAFETY: the header names a load address and length inside
    // `psram_base..psram_base + psram_size` (checked above), which `stage`'s
    // `# Safety` contract makes initialized, writable and exclusively ours;
    // the source is the bootstrap's own Flash-mapped payload.
    unsafe { ptr::copy_nonoverlapping(source, layout.load_address as *mut u8, layout.payload_len) };
    // SAFETY: the BSS range follows the payload inside the same region
    // (checked by `validate_header`), under the same `# Safety` contract.
    unsafe {
        ptr::write_bytes(
            layout.bss_start as *mut u8,
            0,
            layout.bss_end - layout.bss_start,
        )
    };
    if payload_crc32(layout.load_address as *const u8, layout.payload_len) != layout.expected_crc32
    {
        fail(c"OER_BOOT bootstrap=FAIL reason=destination-crc\r\n");
    }
    print(c"OER_BOOT bootstrap=DESTINATION_CRC\r\n");
    Staged {
        load_address: layout.load_address,
        entry: layout.entry,
        payload_len: layout.payload_len,
    }
}

/// Publish the staged code to instruction fetch, report
/// `PASS handoff=stage2`, release the bootstrap's stack watchpoint and enter
/// stage two with interrupts disabled. Never returns.
pub fn hand_off(staged: Staged) -> ! {
    // SAFETY: the bytes are the checked stage-two image, which nothing
    // modifies or executes until the jump below; the other hart, if any,
    // has not been started.
    if unsafe { esp_hal::psram::prepare_code(staged.load_address as *const u8, staged.payload_len) }
        .is_err()
    {
        fail(c"OER_BOOT bootstrap=FAIL reason=psram-code\r\n");
    }
    print(c"OER_BOOT bootstrap=PASS handoff=stage2\r\n");
    // SAFETY: the bootstrap is about to leave its stack for good.
    unsafe { release_bootstrap_stack_watchpoint() };
    // SAFETY: the entry lies in the checked executable range of the image.
    unsafe { jump_to_runtime(staged.entry) }
}

#[derive(Clone, Copy)]
struct ValidatedLayout {
    load_address: usize,
    entry: usize,
    payload_len: usize,
    bss_start: usize,
    bss_end: usize,
    expected_crc32: u32,
}

fn read_header(payload: &[u8]) -> RuntimeHeader {
    if payload.len() < size_of::<RuntimeHeader>() {
        fail(c"OER_BOOT bootstrap=FAIL reason=short-header\r\n");
    }
    // SAFETY: the payload holds at least one header, read without alignment.
    unsafe { payload.as_ptr().cast::<RuntimeHeader>().read_unaligned() }
}

fn validate_header(
    header: RuntimeHeader,
    layout: &Layout,
    psram_base: usize,
    psram_size: usize,
) -> ValidatedLayout {
    let load_address = header.load_address as usize;
    let entry = header.entry as usize;
    let payload_end = header.payload_end as usize;
    let bss_start = header.bss_start as usize;
    let bss_end = header.bss_end as usize;
    let text_start = header.text_start as usize;
    let text_end = header.text_end as usize;
    let psram_end = psram_base
        .checked_add(psram_size)
        .unwrap_or_else(|| fail(c"OER_BOOT bootstrap=FAIL reason=psram-range\r\n"));
    if !header.is_compatible()
        || load_address != layout.runtime_psram().origin as usize
        || payload_end <= load_address
        || text_start < load_address + size_of::<RuntimeHeader>()
        || text_end <= text_start
        || text_end > payload_end
        || entry < text_start
        || entry >= text_end
        || !entry.is_multiple_of(2)
        || bss_start < payload_end
        || bss_end < bss_start
        || payload_end > psram_end
        || bss_end > psram_end
    {
        fail(c"OER_BOOT bootstrap=FAIL reason=header\r\n");
    }

    ValidatedLayout {
        load_address,
        entry,
        payload_len: payload_end - load_address,
        bss_start,
        bss_end,
        expected_crc32: header.payload_crc32,
    }
}

/// Write and read back two words of the PSRAM probe page, before stage two
/// owns the rest.
///
/// # Safety
///
/// `base` is the start of a `u32`-aligned region that [`stage`]'s
/// `# Safety` contract covers, at least `0x108` bytes long.
unsafe fn verify_psram_probe(base: *mut u8) {
    // SAFETY: the probe page is the first page of the region the caller's
    // `# Safety` contract covers, which stage two never occupies.
    let probe = unsafe { base.add(0x100).cast::<u32>() };
    for (index, expected) in [0x31a5_c33c, 0xc35a_3cc3].into_iter().enumerate() {
        // SAFETY: as above; two aligned words of the probe page.
        unsafe { probe.add(index).write_volatile(expected) };
        // SAFETY: as above.
        if unsafe { probe.add(index).read_volatile() } != expected {
            fail(c"OER_BOOT bootstrap=FAIL reason=psram-probe\r\n");
        }
    }
}

fn payload_crc32(address: *const u8, len: usize) -> u32 {
    let mut crc = PayloadCrc::new();
    for index in 0..len {
        // SAFETY: the caller's range is mapped and readable.
        crc.push(unsafe { address.add(index).read_volatile() });
    }
    crc.finish()
}

/// Clear esp-hal's task-stack watchpoint of the bootstrap's stack, which stage
/// two's stacks do not share.
///
/// # Safety
/// Only before leaving the bootstrap's stack for good.
unsafe fn release_bootstrap_stack_watchpoint() {
    let expected_tdata2 = core::ptr::addr_of!(__stack_chk_guard) as usize | 1;
    let observed_tdata2: usize;
    // SAFETY: selects trigger 0 and reads its address; no memory is touched.
    unsafe {
        asm!(
            "csrw 0x7a0, zero",
            "csrr {observed}, 0x7a2",
            observed = out(reg) observed_tdata2,
            options(nostack),
        )
    };
    if observed_tdata2 == expected_tdata2 {
        // SAFETY: clears the trigger esp-hal armed on the bootstrap's guard.
        unsafe {
            asm!(
                "csrw 0x7a1, zero",
                "csrw 0x7a2, zero",
                "fence rw, rw",
                options(nostack),
            )
        };
    }
}

/// # Safety
/// `entry` must be the checked entry of a published stage-two image.
unsafe fn jump_to_runtime(entry: usize) -> ! {
    // SAFETY: the caller's contract; interrupts stay disabled across the jump.
    unsafe {
        asm!(
            "csrci mstatus, 8",
            "fence rw, rw",
            "fence.i",
            "jalr zero, 0({entry})",
            entry = in(reg) entry,
            options(noreturn),
        )
    }
}

/// Print one `OER_BOOT` line through the ROM.
pub fn print(message: &'static CStr) {
    // SAFETY: a NUL-terminated format without arguments.
    unsafe { ets_printf(message.as_ptr()) };
}

fn print_crc_failure(reason: &'static CStr, expected: u32, observed: u32) -> ! {
    // SAFETY: the format consumes one string and two words.
    unsafe {
        ets_printf(
            c"OER_BOOT bootstrap=FAIL reason=%s expected=%08x observed=%08x\r\n".as_ptr(),
            reason.as_ptr(),
            expected,
            observed,
        )
    };
    halt()
}

/// Print `message` and halt.
pub fn fail(message: &'static CStr) -> ! {
    print(message);
    halt()
}

fn halt() -> ! {
    loop {
        // SAFETY: `wfi` stalls the hart until an interrupt; it touches no memory.
        unsafe { asm!("wfi", options(nomem, nostack)) };
    }
}
