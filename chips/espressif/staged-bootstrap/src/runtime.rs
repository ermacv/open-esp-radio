//! Slice-based stage-two validation, PSRAM probing and relocation.
//! The chip bootstrap checks its physical mapping and flash source, reports
//! progress, publishes the copied code and enters it through [`hand_off`].

use core::mem::size_of;

use oer_espressif_staged_layout::{
    Layout,
    stage_two::{Header as RuntimeHeader, PayloadCrc},
};
use oer_memory::{read_volatile, write_volatile};

#[cfg(all(target_arch = "riscv32", feature = "esp32s31"))]
use oer_esp32s31_hal::usb_serial_jtag::early_text;

#[cfg(target_arch = "riscv32")]
#[panic_handler]
fn panic(_info: &core::panic::PanicInfo<'_>) -> ! {
    print("OER_BOOT bootstrap=PANIC\r\n");
    halt()
}

/// Report the bootstrap's start on the USB Serial/JTAG console. Call first.
/// Any hart but [`oer_espressif_staged_layout::boot::BOOT_HART`] stops here.
#[cfg(target_arch = "riscv32")]
pub fn start() {
    let hart = oer_riscv::hart_id();
    if !oer_espressif_staged_layout::boot::may_boot(hart) {
        fail("OER_BOOT bootstrap=FAIL reason=not-boot-hart\r\n");
    }
    print("OER_BOOT bootstrap=START\r\n");
}

/// The entry word of the packed runtime `payload`, for [`hand_off`]'s
/// constant. A payload too short to hold a header only type-checks (an empty
/// stand-in): its entry is 0, which no header names, and [`stage`] rejects it.
pub const fn runtime_entry(payload: &[u8]) -> usize {
    match oer_espressif_staged_layout::stage_two::entry_of(payload) {
        Some(entry) => entry as usize,
        None => 0,
    }
}

/// A completed step, reported before the next staging operation begins.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StageProgress {
    Psram,
    SourceCrc,
    DestinationCrc,
}

/// A rejected PSRAM region or stage-two image.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StageError {
    PsramRegion,
    PsramProbe,
    ShortHeader,
    Header,
    PayloadLength,
    SourceCrc { expected: u32, observed: u32 },
    DestinationCrc { expected: u32, observed: u32 },
}

impl StageError {
    pub const fn reason(self) -> &'static str {
        match self {
            Self::PsramRegion => "psram-init",
            Self::PsramProbe => "psram-probe",
            Self::ShortHeader => "short-header",
            Self::Header => "header",
            Self::PayloadLength => "payload-length",
            Self::SourceCrc { .. } => "source-crc",
            Self::DestinationCrc { .. } => "destination-crc",
        }
    }
}

/// A checked image, retaining its exclusive payload loan until handoff.
#[derive(Debug)]
#[must_use = "the checked image must be published before handoff"]
pub struct Staged<'a> {
    payload: &'a mut [u8],
    entry: usize,
}

impl Staged<'_> {
    pub fn entry(&self) -> usize {
        self.entry
    }

    /// The actual address of the copied bytes.
    pub fn load_address(&self) -> usize {
        self.payload.as_ptr() as usize
    }

    pub fn payload_len(&self) -> usize {
        self.payload.len()
    }
}

/// Probe `psram`, validate and copy `payload`, zero its BSS and check the copy.
/// Header addresses are offsets within `layout.psram`; `psram` lends that
/// complete region. The chip bootstrap checks the slice's physical base and
/// the source's flash mapping before staging proceeds past the probe.
///
/// Writes stay ordered: two volatile u32 probe words, payload copy, BSS zero,
/// then volatile destination CRC reads. `report` observes each completed step.
///
/// # Errors
///
/// Returns a typed region, header, length or checksum failure. A failure
/// before the source CRC succeeds leaves payload and BSS untouched.
///
/// The payload loan prevents rewriting the region before publication:
///
/// ```compile_fail,E0499
/// use oer_espressif_staged_bootstrap::stage;
/// use oer_espressif_staged_layout::Layout;
/// fn rewrite(layout: &Layout, payload: &[u8], psram: &mut [u8]) {
///     let staged = stage(layout, payload, psram, |_| {}).expect("valid image");
///     psram.fill(0);
///     let _ = staged.payload_len();
/// }
/// ```
pub fn stage<'a>(
    layout: &Layout,
    payload: &[u8],
    psram: &'a mut [u8],
    mut report: impl FnMut(StageProgress),
) -> Result<Staged<'a>, StageError> {
    if psram.len() != layout.psram.length() as usize {
        return Err(StageError::PsramRegion);
    }
    verify_psram_probe(psram)?;
    report(StageProgress::Psram);

    let header = RuntimeHeader::from_le_bytes(payload).ok_or(StageError::ShortHeader)?;
    let validated = validate_header(header, layout)?;
    if validated.payload_len != payload.len() {
        return Err(StageError::PayloadLength);
    }
    let source_crc = payload_crc32(payload);
    if source_crc != header.payload_crc32 {
        return Err(StageError::SourceCrc {
            expected: header.payload_crc32,
            observed: source_crc,
        });
    }
    report(StageProgress::SourceCrc);

    let destination = psram
        .get_mut(validated.payload.clone())
        .ok_or(StageError::Header)?;
    // The validated range and payload have the same checked length. Zip
    // copies that complete range without a panicking slice-copy entry point.
    for (byte, &source) in destination.iter_mut().zip(payload) {
        *byte = source;
    }
    psram
        .get_mut(validated.bss)
        .ok_or(StageError::Header)?
        .fill(0);
    let destination = psram.get_mut(validated.payload).ok_or(StageError::Header)?;
    let destination_crc = payload_crc32(destination);
    if destination_crc != header.payload_crc32 {
        return Err(StageError::DestinationCrc {
            expected: header.payload_crc32,
            observed: destination_crc,
        });
    }
    report(StageProgress::DestinationCrc);
    Ok(Staged {
        payload: destination,
        entry: header.entry as usize,
    })
}

/// Accept the cache publication of the staged code, report
/// `PASS handoff=stage2`, run the final boot writes and enter stage two
/// with interrupts disabled. Never returns.
///
/// `finish_boot` disables the RTC watchdog immediately before the jump;
/// publication and all fallible boot work must already have completed.
///
/// `ENTRY` is the stage-two entry the bootstrap's build knows (the entry word
/// of the payload it carries, see [`runtime_entry`]). The jump goes to
/// that constant, so the stack analysis sees where the bootstrap's stack
/// ends; the header's own entry must equal it or the bootstrap halts with an
/// error instead of jumping.
#[cfg(target_arch = "riscv32")]
#[allow(unsafe_code, reason = "publishes and enters the stage-two image")]
pub fn hand_off<const ENTRY: usize>(
    staged: Staged<'_>,
    published: oer_espressif_boot::cache::PublishedCode,
    finish_boot: impl FnOnce(),
) -> ! {
    if published.range().start() as usize != staged.load_address()
        || (published.range().size() as usize) < staged.payload_len()
    {
        fail("OER_BOOT bootstrap=FAIL reason=psram-code-range\r\n");
    }
    let payload_end = staged.load_address().checked_add(staged.payload_len());
    if staged.entry != ENTRY
        || !payload_end.is_some_and(|end| (staged.load_address()..end).contains(&ENTRY))
    {
        fail("OER_BOOT bootstrap=FAIL reason=entry-mismatch\r\n");
    }
    print("OER_BOOT bootstrap=PASS handoff=stage2\r\n");
    finish_boot();
    // SAFETY: the entry equals the build's constant and lies in the checked
    // executable range of the published image; its entry installs its own stack.
    unsafe { oer_riscv::jump_to_entry::<ENTRY>() }
}

struct ValidatedLayout {
    payload: core::ops::Range<usize>,
    payload_len: usize,
    bss: core::ops::Range<usize>,
}

fn validate_header(header: RuntimeHeader, layout: &Layout) -> Result<ValidatedLayout, StageError> {
    let load_address = header.load_address;
    let Some(header_end) = load_address.checked_add(size_of::<RuntimeHeader>() as u32) else {
        return Err(StageError::Header);
    };
    if !header.is_compatible()
        || !layout
            .runtime_psram()
            .is_some_and(|region| load_address == region.origin())
        || header.payload_end <= load_address
        || header.text_start < header_end
        || header.text_end <= header.text_start
        || header.text_end > header.payload_end
        || header.entry < header.text_start
        || header.entry >= header.text_end
        || !header.entry.is_multiple_of(2)
        || header.bss_start < header.payload_end
        || header.bss_end < header.bss_start
        || header.payload_end > layout.psram.end()
        || header.bss_end > layout.psram.end()
    {
        return Err(StageError::Header);
    }
    let offset = |address: u32| {
        address
            .checked_sub(layout.psram.origin())
            .map(|offset| offset as usize)
            .ok_or(StageError::Header)
    };
    Ok(ValidatedLayout {
        payload: offset(load_address)?..offset(header.payload_end)?,
        payload_len: (header.payload_end - load_address) as usize,
        bss: offset(header.bss_start)?..offset(header.bss_end)?,
    })
}

fn verify_psram_probe(psram: &mut [u8]) -> Result<(), StageError> {
    let probe = psram.get_mut(0x100..0x108).ok_or(StageError::PsramRegion)?;
    let words: &mut [u32] =
        bytemuck::try_cast_slice_mut(probe).map_err(|_| StageError::PsramRegion)?;
    for (word, expected) in words.iter_mut().zip([0x31a5_c33c, 0xc35a_3cc3]) {
        write_volatile(word, expected);
        if read_volatile(word) != expected {
            return Err(StageError::PsramProbe);
        }
    }
    Ok(())
}

fn payload_crc32(payload: &[u8]) -> u32 {
    let mut crc = PayloadCrc::new();
    for byte in payload {
        crc.push(read_volatile(byte));
    }
    crc.finish()
}

/// Print one `OER_BOOT` line through the early USB writer, which drops what
/// the FIFO has no room for and counts it. The bootstrap never retires the
/// writer, so it owns the block throughout.
#[cfg(target_arch = "riscv32")]
pub fn print(message: &'static str) {
    early_text(message.as_bytes());
}

/// Report the typed cold-boot failure, retaining its observed density, and halt.
#[cfg(target_arch = "riscv32")]
pub fn fail_boot(error: oer_espressif_boot::BootFailure) -> ! {
    BootFailureRecord(error).write(|bytes| {
        let _ = early_text(bytes);
    });
    halt()
}

#[cfg(any(test, target_arch = "riscv32"))]
struct BootFailureRecord(oer_espressif_boot::BootFailure);

#[cfg(any(test, target_arch = "riscv32"))]
impl BootFailureRecord {
    fn write(self, mut output: impl FnMut(&[u8])) {
        use oer_espressif_boot::{BootFailure, FlashFailure, psram::PsramFailure};

        output(b"OER_BOOT bootstrap=FAIL reason=");
        output(self.0.reason().as_bytes());
        match self.0 {
            BootFailure::Psram(PsramFailure::DensityMismatch { found }) => {
                output(b" found=");
                decimal(found as usize, &mut output);
            }
            BootFailure::Psram(PsramFailure::UnsupportedDensity(code)) => {
                output(b" code=");
                decimal(code as usize, &mut output);
            }
            BootFailure::Flash(FlashFailure::XipOutside { index, words }) => {
                output(b" index=");
                decimal(index, &mut output);
                output(b" words=");
                decimal(words, &mut output);
            }
            _ => {}
        }
        if let Some(wait) = self.0.wait() {
            output(b" wait=");
            output(wait.as_bytes());
        }
        output(b"\r\n");
    }
}

#[cfg(any(test, target_arch = "riscv32"))]
fn decimal(mut value: usize, output: &mut impl FnMut(&[u8])) {
    // At most one decimal digit per bit, on both the host and the chip.
    let mut digits = [b'0'; usize::BITS as usize];
    let mut first = digits.len();
    for digit in digits.iter_mut().rev() {
        first -= 1;
        *digit = b'0' + (value % 10) as u8;
        value /= 10;
        if value == 0 {
            break;
        }
    }
    for digit in digits.iter().skip(first) {
        output(core::slice::from_ref(digit));
    }
}

/// Report a typed staging failure and halt the bootstrap.
#[cfg(target_arch = "riscv32")]
pub fn fail_stage(error: StageError) -> ! {
    print("OER_BOOT bootstrap=FAIL reason=");
    print(error.reason());
    if let StageError::SourceCrc { expected, observed } = error {
        print(" expected=");
        print_hex(expected);
        print(" observed=");
        print_hex(observed);
    }
    fail("\r\n")
}

#[cfg(target_arch = "riscv32")]
fn print_hex(word: u32) {
    let mut digits = [0; 8];
    for (index, digit) in digits.iter_mut().enumerate() {
        let nibble = (word >> (28 - 4 * index)) & 0xf;
        // A nibble is always an ASCII hexadecimal digit.
        *digit = if nibble < 10 {
            b'0' + nibble as u8
        } else {
            b'a' + (nibble as u8 - 10)
        };
    }
    early_text(&digits);
}

/// Print `message` and halt.
#[cfg(target_arch = "riscv32")]
pub fn fail(message: &'static str) -> ! {
    print(message);
    halt()
}

#[cfg(target_arch = "riscv32")]
fn halt() -> ! {
    loop {
        oer_riscv::wait_for_interrupt();
    }
}

#[cfg(test)]
mod tests;
