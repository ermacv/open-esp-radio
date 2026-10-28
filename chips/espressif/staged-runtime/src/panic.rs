//! The image's one panic entry.
//!
//! The handler first masks its hart (maximum CLIC threshold, then MIE off)
//! and orders the mask before claiming or writing the retained slot. It
//! records the panic in `.rtc_fast.persistent`, RTC fast memory no
//! reset entry initializes, without formatting: the location's file bytes (the
//! last [`FILE_BYTES`]), line and column, the message when it is a static
//! string (its first [`MESSAGE_BYTES`]; a formatted message is only marked),
//! the hart, whether it ran on the interrupt stack and, there, the interrupted
//! PC. Interrupt-context panics omit textual metadata: compiler location and
//! format constants may live in cached PSRAM even when their code is in SRAM.
//! Every step is a bounded copy, so the path stays a short leaf in every
//! context's stack bound.
//! Named faults retain a numeric reason code and four observation words;
//! only next-boot thread reporting decodes the reason's text.
//!
//! An image with the `panic-hook` feature defines `oer_platform_panic_hook`,
//! which the entry calls after the record: it records more of the image's
//! state the same way, without formatting, and returns. The chip then resets
//! ([`fatal::reset`](crate::fatal::reset));
//! the next boot takes the record with [`take_previous`] and reports it in
//! thread context.
use core::ops::DerefMut;
mod fault;
pub use fault::FaultReason;

#[cfg(target_arch = "riscv32")]
use oer_memory::retained::RetainedGuard;

/// Bytes of the location's file path kept: its end, which names the file.
pub const FILE_BYTES: usize = 48;
/// Bytes of a static panic message kept: its start.
pub const MESSAGE_BYTES: usize = 64;
const INVALID_HART: u32 = 2;
const NAMED_FAULT: u32 = 3;
const MAGIC: u32 = 0x5045_524f; // "OREP"

/// Metadata safe for the image's panic hook to read. Task context keeps the
/// caller's metadata; interrupt context leaves it empty, because compiler
/// location and format constants may be in unavailable cached memory. The
/// hook's code and every state object it reads must themselves be in SRAM or
/// retained memory, and it must not acquire a lock.
pub struct PanicDetails<'a> {
    pub file: &'a str,
    pub line: u32,
    pub column: u32,
    pub message: Option<&'a str>,
}

/// One panic, as the next boot reads it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PanicRecord {
    pub hart: u8,
    /// The panic ran on the hart's interrupt stack.
    pub in_interrupt: bool,
    /// `mepc` when [`Self::in_interrupt`]: the interrupted instruction.
    pub interrupted_pc: Option<u32>,
    pub line: u32,
    pub column: u32,
    file: [u8; FILE_BYTES],
    file_length: u8,
    message: Message,
}

/// A panic's message as recorded.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Message {
    /// The start of a static message.
    Static([u8; MESSAGE_BYTES], u8),
    /// A message with arguments, which the entry does not format.
    Formatted,
}

impl PanicRecord {
    /// The end of the location's file path, as recorded.
    pub fn file(&self) -> &str {
        let bytes = self
            .file
            .get(..usize::from(self.file_length))
            .unwrap_or_default();
        // The kept suffix may start inside a UTF-8 sequence; skip to the
        // first boundary.
        let start = (0..bytes.len())
            .find(|&i| {
                bytes
                    .get(i..)
                    .is_some_and(|tail| core::str::from_utf8(tail).is_ok())
            })
            .unwrap_or(bytes.len());
        bytes
            .get(start..)
            .and_then(|tail| core::str::from_utf8(tail).ok())
            .unwrap_or("")
    }

    /// The start of the panic's message when it was a static string; `None`
    /// for a formatted one.
    pub fn message(&self) -> Option<&str> {
        let Message::Static(bytes, length) = &self.message else {
            return None;
        };
        let bytes = bytes.get(..usize::from(*length)).unwrap_or_default();
        // The kept prefix may end inside a UTF-8 sequence; drop that part.
        let kept = (0..=bytes.len())
            .rev()
            .find_map(|i| {
                bytes
                    .get(..i)
                    .and_then(|head| core::str::from_utf8(head).ok())
            })
            .unwrap_or("");
        Some(kept)
    }
}

/// The retained words: a magic, the record and a checksum over both.
#[derive(Clone, Copy, bytemuck::AnyBitPattern)]
#[repr(C)]
struct Slot {
    magic: u32,
    hart: u32,
    in_interrupt: u32,
    interrupted_pc: u32,
    line: u32,
    column: u32,
    file_length: u32,
    file: [u8; FILE_BYTES],
    /// A static message's length plus one; zero for a formatted message.
    /// Under NAMED_FAULT this word holds the numeric reason code instead.
    message_length: u32,
    message: [u8; MESSAGE_BYTES],
    checksum: u32,
}

const _: () = assert!(core::mem::size_of::<Slot>() == 148);

impl Slot {
    #[cfg(test)]
    const EMPTY: Self = Self {
        magic: 0,
        hart: 0,
        in_interrupt: 0,
        interrupted_pc: 0,
        line: 0,
        column: 0,
        file_length: 0,
        file: [0; FILE_BYTES],
        message_length: 0,
        message: [0; MESSAGE_BYTES],
        checksum: 0,
    };

    fn sum(&self) -> u32 {
        let mut sum = 0x811c_9dc5_u32;
        let mut mix = |word: u32| sum = (sum ^ word).wrapping_mul(0x0100_0193);
        for word in [
            self.magic,
            self.hart,
            self.in_interrupt,
            self.interrupted_pc,
            self.line,
            self.column,
            self.file_length,
            self.message_length,
        ] {
            mix(word);
        }
        for byte in self.file.into_iter().chain(self.message) {
            mix(u32::from(byte));
        }
        sum
    }
}

crate::retained_static! {
    static SLOT: Slot;
}

/// Take the record the previous boot's panic left, and clear it. Call once,
/// early at boot. Retained named faults (including `InvalidHart`) are reported
/// through the early console and cleared only after successful output; they
/// are never returned as panics.
/// `None` when the previous boot did not panic, the chip lost
/// power or the slot is held by a panic already being recorded.
pub fn take_previous() -> Option<PanicRecord> {
    let mut slot = SLOT.try_lock()?;
    if slot.invalid_hart().is_some() || slot.named_fault().is_some() {
        #[cfg(feature = "esp32s31")]
        report_fault_early(&mut slot);
        return None;
    }
    slot.take()
}

/// Every staged image attempts fault output during adoption, including images
/// that do not consume ordinary previous-panic metadata.
#[cfg(all(target_arch = "riscv32", feature = "esp32s31"))]
pub(crate) fn report_previous_fault() {
    if let Some(mut slot) = SLOT.try_lock() {
        report_fault_early(&mut slot);
    }
}

#[cfg(feature = "esp32s31")]
fn report_fault_early(slot: &mut Slot) {
    let _reported = report_named_fault(slot, |name, observations| {
        write_named_fault(name, observations, |bytes| {
            oer_esp32s31_hal::usb_serial_jtag::early_text(bytes)
                .then_some(())
                .ok_or(())
        })
    });
    let _reported = report_invalid_hart(slot, |note| {
        oer_diagnostics::emit_note(note, |bytes| {
            oer_esp32s31_hal::usb_serial_jtag::early_text(bytes)
                .then_some(())
                .ok_or(())
        })
    });
}

#[cfg(any(test, feature = "esp32s31"))]
fn report_invalid_hart<E>(
    slot: &mut Slot,
    emit: impl FnOnce(&oer_diagnostics::Note<'_>) -> Result<(), E>,
) -> Result<(), E> {
    if let Some(raw) = slot.invalid_hart() {
        emit(&oer_diagnostics::Note::fatal_fail(
            raw,
            oer_diagnostics::name!("InvalidHart"),
        ))?;
        slot.magic = 0;
    }
    Ok(())
}

#[cfg(all(target_arch = "riscv32", feature = "esp32s31"))]
pub(crate) fn with_invalid_hart_record<R>(raw: u32, reset: impl FnOnce() -> R) -> R {
    let _record = SLOT.try_lock().map(|mut slot| {
        slot.record_invalid_hart(raw);
        slot
    });
    reset()
}

/// Retain a named fault in the existing panic slot, holding its claim until
/// reset. The four words are the caller's exact source observations. No owner
/// is released, no cached reason text is read, and no retained allocation is added.
#[cfg(all(target_arch = "riscv32", feature = "esp32s31"))]
pub fn reset_fault(reason: FaultReason, observations: [u32; 4]) -> ! {
    crate::fatal::reset_recording(|_| {
        let _record = record_named_fault_in(SLOT.try_lock(), reason, observations).map(|slot| {
            // Keep the claim even if a later reset operation faults.
            core::mem::forget(slot);
        });
    })
}

// The guard is the current boot's first-writer election: it stays held until
// reset. A fresh claim may replace any retained bytes from an earlier boot,
// even when that image never consumed the previous record.
#[cfg(any(test, all(target_arch = "riscv32", feature = "esp32s31")))]
fn record_named_fault_in<G: DerefMut<Target = Slot>>(
    slot: Option<G>,
    reason: FaultReason,
    observations: [u32; 4],
) -> Option<G> {
    let mut slot = slot?;
    slot.record_named_fault(reason, observations);
    Some(slot)
}

#[cfg(any(test, feature = "esp32s31"))]
fn write_named_fault<E>(
    name: &str,
    observations: [u32; 4],
    mut emit: impl FnMut(&[u8]) -> Result<(), E>,
) -> Result<(), E> {
    emit(b"previous fault ")?;
    emit(name.as_bytes())?;
    emit(b":")?;
    for mut word in observations {
        let mut digits = [0; 8];
        for digit in digits.iter_mut().rev() {
            let nibble = (word & 0xf) as u8;
            *digit = if nibble < 10 {
                b'0'.wrapping_add(nibble)
            } else {
                b'a'.wrapping_add(nibble - 10)
            };
            word >>= 4;
        }
        emit(b" ")?;
        emit(&digits)?;
    }
    emit(b"\n")
}

#[cfg(any(test, feature = "esp32s31"))]
fn report_named_fault<E>(
    slot: &mut Slot,
    emit: impl FnOnce(&str, [u32; 4]) -> Result<(), E>,
) -> Result<(), E> {
    if let Some((name, observations)) = slot.named_fault() {
        emit(name, observations)?;
        slot.magic = 0;
    }
    Ok(())
}

impl Slot {
    #[cfg(any(test, all(target_arch = "riscv32", feature = "esp32s31")))]
    fn record_named_fault(&mut self, reason: FaultReason, observations: [u32; 4]) {
        self.magic = 0;
        self.file = [0; FILE_BYTES];
        self.file_length = 0;
        self.message = [0; MESSAGE_BYTES];
        // NAMED_FAULT interprets this existing word as a numeric reason code.
        self.message_length = reason as u32;
        [self.hart, self.interrupted_pc, self.line, self.column] = observations;
        self.in_interrupt = NAMED_FAULT;
        self.magic = MAGIC;
        self.checksum = self.sum();
    }

    fn named_fault(&self) -> Option<(&str, [u32; 4])> {
        if self.magic != MAGIC || self.in_interrupt != NAMED_FAULT || self.checksum != self.sum() {
            return None;
        }
        if self.file_length != 0 {
            return None;
        }
        let name = FaultReason::from_code(self.message_length)?.name();
        Some((
            name,
            [self.hart, self.interrupted_pc, self.line, self.column],
        ))
    }
}

impl Slot {
    fn invalid_hart(&self) -> Option<u32> {
        (self.magic == MAGIC && self.in_interrupt == INVALID_HART && self.checksum == self.sum())
            .then_some(self.hart)
    }

    #[cfg(any(test, all(target_arch = "riscv32", feature = "esp32s31")))]
    fn record_invalid_hart(&mut self, raw: u32) {
        // The retained cell's current-boot claim, not stale valid bytes,
        // excludes nested fault writers.
        self.magic = 0;
        self.hart = raw;
        self.in_interrupt = INVALID_HART;
        self.magic = MAGIC;
        self.checksum = self.sum();
    }
}

impl Slot {
    fn take(&mut self) -> Option<PanicRecord> {
        let valid = self.magic == MAGIC
            && self.in_interrupt <= 1
            && self.checksum == self.sum()
            && self.file_length as usize <= FILE_BYTES
            && self.message_length as usize <= MESSAGE_BYTES + 1;
        let record = valid.then(|| PanicRecord {
            hart: self.hart as u8,
            in_interrupt: self.in_interrupt != 0,
            interrupted_pc: (self.in_interrupt != 0).then_some(self.interrupted_pc),
            line: self.line,
            column: self.column,
            file: self.file,
            file_length: self.file_length as u8,
            message: match self.message_length {
                0 => Message::Formatted,
                length => Message::Static(self.message, (length - 1) as u8),
            },
        });
        self.magic = 0;
        record
    }
}

/// Keep the claim until reset, including while the image's hook runs. A
/// nested panic refuses the claim before reading any textual metadata and
/// resets without overwriting the first record or invoking the hook again.
fn record_in<'a, G: DerefMut<Target = Slot>>(
    slot: Option<G>,
    hart: usize,
    in_interrupt: bool,
    mepc: usize,
    details: impl FnOnce() -> PanicDetails<'a>,
) -> Option<(G, PanicDetails<'a>)> {
    let mut slot = slot?;
    let details = details();
    (slot.file, slot.file_length) = file_tail(details.file);
    (slot.message, slot.message_length) = match details.message {
        Some(message) => {
            let (kept, length) = message_head(message);
            (kept, length + 1)
        }
        None => ([0; MESSAGE_BYTES], 0),
    };
    slot.hart = hart as u32;
    slot.in_interrupt = u32::from(in_interrupt);
    slot.interrupted_pc = if in_interrupt { mepc as u32 } else { 0 };
    slot.line = details.line;
    slot.column = details.column;
    slot.magic = MAGIC;
    slot.checksum = slot.sum();
    Some((slot, details))
}

/// Record `info` in the retained slot without formatting.
#[cfg(target_arch = "riscv32")]
fn record<'a>(
    info: &'a core::panic::PanicInfo<'_>,
) -> Option<(RetainedGuard<'static, Slot>, PanicDetails<'a>)> {
    let hart = oer_riscv::hart_id();
    if oer_esp32s31_hal::platform::Hart::new(hart as u32).is_err() {
        let mut slot = SLOT.try_lock()?;
        slot.record_invalid_hart(hart as u32);
        return Some((
            slot,
            PanicDetails {
                file: "",
                line: 0,
                column: 0,
                message: Some("InvalidHart"),
            },
        ));
    }
    let sp = oer_riscv::stack_pointer();
    let mepc = oer_riscv::read_mepc();
    let in_interrupt = crate::stacks::on_interrupt_stack(hart, sp);
    record_in(SLOT.try_lock(), hart, in_interrupt, mepc, || {
        if in_interrupt {
            PanicDetails {
                file: "",
                line: 0,
                column: 0,
                message: None,
            }
        } else {
            let (file, line, column) = info
                .location()
                .map_or(("", 0, 0), |l| (l.file(), l.line(), l.column()));
            PanicDetails {
                file,
                line,
                column,
                message: info.message().as_str(),
            }
        }
    })
}

#[cfg(all(target_arch = "riscv32", feature = "panic-hook"))]
// SAFETY: `panic_hook!` defines this symbol with the same Rust signature
// and no memory-safety precondition; a missing definition fails the link.
unsafe extern "Rust" {
    /// The image's record of its own state after the platform's record, in
    /// the fatal path with the other hart stopped (or why it is not). It
    /// formats nothing, like the entry: it runs in every context's stack
    /// bound.
    safe fn oer_platform_panic_hook(
        details: &PanicDetails<'_>,
        stop: Result<&crate::fatal::StopTheWorld<'_>, crate::fatal::StopError>,
    );
}

#[cfg(target_arch = "riscv32")]
#[panic_handler]
fn panic(info: &core::panic::PanicInfo<'_>) -> ! {
    let recorded = with_panic_mask(
        || {
            let _ = oer_riscv::swap_interrupt_threshold(u8::MAX);
        },
        oer_riscv::disable_interrupts,
        || record(info),
    );
    let Some((_slot, details)) = recorded else {
        crate::fatal::reset()
    };
    #[cfg(feature = "panic-hook")]
    crate::fatal::reset_recording(|stop| oer_platform_panic_hook(&details, stop));
    #[cfg(not(feature = "panic-hook"))]
    {
        let _ = details;
        crate::fatal::reset()
    }
}

/// Prevent interrupt reentry from resetting a partially written first
/// record. The callbacks let a host hart model inject a pending interrupt
/// during the write; the production callbacks use the architectural APIs.
fn with_panic_mask<T>(
    mask_threshold: impl FnOnce(),
    disable_interrupts: impl FnOnce(),
    record: impl FnOnce() -> T,
) -> T {
    mask_threshold();
    disable_interrupts();
    core::sync::atomic::compiler_fence(core::sync::atomic::Ordering::SeqCst);
    record()
}

/// The last [`FILE_BYTES`] of `file`, zero-padded, and how many there are.
/// It copies through iterators, without an index whose bounds check could
/// fail: a panic while recording re-enters the handler, a call-graph cycle
/// that leaves every interrupt's stack bound unknown.
fn file_tail(file: &str) -> ([u8; FILE_BYTES], u32) {
    let bytes = file.as_bytes();
    let mut kept = [0; FILE_BYTES];
    let mut length = 0;
    for (to, from) in kept
        .iter_mut()
        .zip(bytes.iter().skip(bytes.len().saturating_sub(FILE_BYTES)))
    {
        *to = *from;
        length += 1;
    }
    (kept, length)
}

/// The first [`MESSAGE_BYTES`] of `message`, zero-padded, and how many there
/// are; like [`file_tail`], without an index.
fn message_head(message: &str) -> ([u8; MESSAGE_BYTES], u32) {
    let mut kept = [0; MESSAGE_BYTES];
    let mut length = 0;
    for (to, from) in kept.iter_mut().zip(message.as_bytes()) {
        *to = *from;
        length += 1;
    }
    (kept, length)
}

#[cfg(test)]
mod tests {
    use core::cell::{Cell, RefCell};

    use super::*;

    fn details() -> PanicDetails<'static> {
        PanicDetails {
            file: "src/first.rs",
            line: 17,
            column: 4,
            message: Some("first panic"),
        }
    }

    #[test]
    fn the_first_panic_records_its_metadata_and_is_taken_once() {
        let storage = RefCell::new(Slot::EMPTY);
        let (mut slot, _) = record_in(storage.try_borrow_mut().ok(), 0, false, 0x1234, details)
            .expect("the first panic claims the slot");
        let record = slot.take().expect("the complete record validates");
        assert_eq!(record.file(), "src/first.rs");
        assert_eq!(record.message(), Some("first panic"));
        assert_eq!((record.hart, record.line, record.column), (0, 17, 4));
        assert!(!record.in_interrupt);
        assert_eq!(record.interrupted_pc, None);
        assert_eq!(slot.take(), None);
    }

    #[test]
    fn nested_panics_during_recording_and_the_hook_preserve_the_first_record() {
        let storage = RefCell::new(Slot::EMPTY);
        let nested_metadata_read = Cell::new(false);
        let nested = || {
            nested_metadata_read.set(true);
            details()
        };
        let (mut first, _) = record_in(storage.try_borrow_mut().ok(), 0, false, 0, || {
            // Reentry while the first panic gathers its metadata.
            assert!(record_in(storage.try_borrow_mut().ok(), 1, true, 0x5678, nested).is_none());
            details()
        })
        .expect("the outer panic owns the claim");
        // Reentry from the image hook after the complete record was written.
        assert!(record_in(storage.try_borrow_mut().ok(), 1, true, 0x5678, nested).is_none());
        assert!(!nested_metadata_read.get());
        let record = first.take().expect("nested entry left the record intact");
        assert_eq!(record.message(), Some("first panic"));
        assert_eq!((record.hart, record.line), (0, 17));
    }

    #[test]
    fn an_interrupt_during_a_partial_write_stays_pending_and_retains_the_first_panic() {
        use core::cell::RefMut;
        use core::ops::Deref;

        struct InterruptingWrite<'a> {
            slot: RefMut<'a, Slot>,
            threshold: &'a Cell<u8>,
            mie: &'a Cell<bool>,
            pending: &'a Cell<bool>,
        }

        impl Deref for InterruptingWrite<'_> {
            type Target = Slot;

            fn deref(&self) -> &Slot {
                &self.slot
            }
        }

        impl DerefMut for InterruptingWrite<'_> {
            fn deref_mut(&mut self) -> &mut Slot {
                if self.slot.file_length != 0 && self.slot.magic == 0 {
                    // The first record has a file but no valid checksum yet.
                    // An admitted interrupt would panic, be refused the held
                    // slot and reset before the first record can complete.
                    self.pending.set(true);
                    assert!(
                        !self.mie.get() || self.threshold.get() == u8::MAX,
                        "a nested interrupt panic would reset the incomplete first record"
                    );
                }
                &mut self.slot
            }
        }

        let storage = RefCell::new(Slot::EMPTY);
        let threshold = Cell::new(0_u8);
        let mie = Cell::new(true);
        let pending = Cell::new(false);
        let steps = RefCell::new(std::vec::Vec::new());
        let (first, _) = with_panic_mask(
            || {
                threshold.set(u8::MAX);
                steps.borrow_mut().push("threshold");
            },
            || {
                mie.set(false);
                steps.borrow_mut().push("MIE off");
            },
            || {
                steps.borrow_mut().push("claim");
                let slot = storage.try_borrow_mut().ok().map(|slot| InterruptingWrite {
                    slot,
                    threshold: &threshold,
                    mie: &mie,
                    pending: &pending,
                });
                record_in(slot, 0, false, 0, details)
            },
        )
        .expect("the masked hart completes the first record");
        assert_eq!(*steps.borrow(), ["threshold", "MIE off", "claim"]);
        assert!(
            pending.get(),
            "an interrupt arrived during the partial write"
        );
        assert_eq!((threshold.get(), mie.get()), (u8::MAX, false));
        assert!(storage.try_borrow_mut().is_err());
        let next_boot = RefCell::new(*first);
        let record = next_boot
            .borrow_mut()
            .take()
            .expect("complete retained record");
        assert_eq!(record.message(), Some("first panic"));
        assert_eq!((record.hart, record.line), (0, 17));
    }

    #[test]
    fn a_reset_retains_the_record_and_clears_the_claim_on_the_fake() {
        let storage = RefCell::new(Slot::EMPTY);
        let (first, _) = record_in(storage.try_borrow_mut().ok(), 0, true, 0x1234, details)
            .expect("the first boot records its panic");
        assert!(storage.try_borrow_mut().is_err());
        // Reset preserves the RTC bytes, but constructs a fresh, unclaimed
        // SRAM cell. The old boot's guard is still held when bytes are copied.
        let next_boot = RefCell::new(*first);
        let record = next_boot.borrow_mut().take().expect("retained record");
        assert!(record.in_interrupt);
        assert_eq!(record.interrupted_pc, Some(0x1234));
        assert_eq!(record.message(), Some("first panic"));
        assert_eq!(next_boot.borrow_mut().take(), None);
    }

    fn record_with(file: &[u8]) -> PanicRecord {
        let mut stored = [0; FILE_BYTES];
        stored[..file.len()].copy_from_slice(file);
        PanicRecord {
            hart: 0,
            in_interrupt: false,
            interrupted_pc: None,
            line: 1,
            column: 1,
            file: stored,
            file_length: file.len() as u8,
            message: Message::Formatted,
        }
    }

    #[test]
    fn a_static_message_keeps_its_start_and_a_formatted_one_is_marked() {
        let (kept, length) = message_head("index out of range");
        let record = PanicRecord {
            message: Message::Static(kept, length as u8),
            ..record_with(b"src/lib.rs")
        };
        assert_eq!(record.message(), Some("index out of range"));
        // "é" is 0xc3 0xa9; a kept prefix may end after its first byte.
        let long = "a".repeat(MESSAGE_BYTES - 1) + "é";
        let (kept, length) = message_head(&long);
        assert_eq!(length as usize, MESSAGE_BYTES);
        let record = PanicRecord {
            message: Message::Static(kept, length as u8),
            ..record_with(b"src/lib.rs")
        };
        assert_eq!(record.message(), Some(&long[..MESSAGE_BYTES - 1]));
        assert_eq!(record_with(b"src/lib.rs").message(), None);
    }

    #[test]
    fn a_kept_suffix_starting_inside_a_character_skips_to_its_end() {
        assert_eq!(record_with(b"src/lib.rs").file(), "src/lib.rs");
        // "é" is 0xc3 0xa9; a suffix may start at its second byte.
        assert_eq!(record_with(b"\xa9/lib.rs").file(), "/lib.rs");
    }

    #[test]
    fn a_file_keeps_its_last_bytes_zero_padded() {
        let (kept, length) = file_tail("src/lib.rs");
        assert_eq!((&kept[..10], length), (&b"src/lib.rs"[..], 10));
        assert!(kept[10..].iter().all(|&b| b == 0));
        let long = "a/".repeat(30) + "tail.rs";
        let (kept, length) = file_tail(&long);
        assert_eq!(length as usize, FILE_BYTES);
        assert_eq!(&kept[..], &long.as_bytes()[long.len() - FILE_BYTES..]);
    }

    #[test]
    fn the_checksum_covers_every_field() {
        let mut slot = Slot::EMPTY;
        slot.magic = MAGIC;
        let empty = slot.sum();
        slot.file[FILE_BYTES - 1] = 1;
        assert_ne!(slot.sum(), empty);
        slot.file[FILE_BYTES - 1] = 0;
        slot.line = 7;
        assert_ne!(slot.sum(), empty);
        slot.line = 0;
        slot.message[0] = 1;
        assert_ne!(slot.sum(), empty);
    }
    #[test]
    fn invalid_hart_retains_full_raw_id_and_never_decodes_as_a_valid_hart() {
        let mut slot = Slot::EMPTY;
        slot.record_invalid_hart(u32::MAX);
        assert_eq!(slot.invalid_hart(), Some(u32::MAX));
        assert!(
            slot.take().is_none(),
            "a fault is never a panic with a truncated hart"
        );
        let mut damaged = Slot::EMPTY;
        damaged.record_invalid_hart(u32::MAX);
        damaged.hart ^= 1;
        assert!(
            damaged.invalid_hart().is_none(),
            "checksum covers the raw ID"
        );
    }
    #[test]
    fn next_boot_reports_raw_fault_and_console_refusal_keeps_it() {
        let mut slot = Slot::EMPTY;
        slot.record_invalid_hart(u32::MAX);
        assert!(report_invalid_hart(&mut slot, |_| Err(())).is_err());
        assert_eq!(slot.invalid_hart(), Some(u32::MAX));
        let mut bytes = std::vec::Vec::new();
        let mut console = oer_diagnostics::Console::new(|part: &[u8]| {
            bytes.extend_from_slice(part);
            Ok::<_, ()>(())
        });
        report_invalid_hart(&mut slot, |note| console.note(note)).unwrap();
        drop(console);
        let line = bytes
            .strip_prefix(b"\r\n")
            .unwrap()
            .strip_suffix(b"\r\n")
            .unwrap();
        let mut decoded = [0; oer_diagnostics::MAX_PAYLOAD + 7];
        let (_, record) = oer_diagnostics::decode(line, &mut decoded)
            .unwrap()
            .unwrap();
        let oer_diagnostics::Record::Note(note) = record else {
            panic!("a named fault note")
        };
        assert_eq!(note.words(), &[u32::MAX]);
        assert_eq!(note.names(), &["InvalidHart"]);
        assert_eq!(slot.invalid_hart(), None);
    }
}

#[cfg(test)]
mod fault_tests {
    use super::*;

    use core::cell::RefCell;

    #[test]
    fn named_fault_reuses_the_slot_and_preserves_the_current_boot_claim() {
        let storage = RefCell::new(Slot::EMPTY);
        let words = [0xffff_ffff, 0xfedc_ba98, 0x7654_3210, 0x8000_0000];
        let mut slot = record_named_fault_in(
            storage.try_borrow_mut().ok(),
            FaultReason::RouteWrongHart,
            words,
        )
        .expect("first fault owns the current boot claim");
        assert!(
            record_named_fault_in(
                storage.try_borrow_mut().ok(),
                FaultReason::OutsideExecutor,
                [0; 4],
            )
            .is_none()
        );
        assert_eq!(slot.file, [0; FILE_BYTES], "capture keeps no text");
        assert_eq!(slot.message, [0; MESSAGE_BYTES], "capture keeps no text");
        assert_eq!(slot.message_length, FaultReason::RouteWrongHart as u32);
        assert_eq!(slot.named_fault(), Some(("RouteWrongHart", words)));
        assert!(report_named_fault(&mut slot, |_, _| Err(())).is_err());
        assert_eq!(slot.named_fault(), Some(("RouteWrongHart", words)));
        let mut output = std::vec::Vec::new();
        assert!(
            report_named_fault(&mut slot, |name, observations| {
                write_named_fault(name, observations, |bytes| {
                    output.extend_from_slice(bytes);
                    Ok::<_, ()>(())
                })
            })
            .is_ok()
        );
        assert_eq!(
            output,
            b"previous fault RouteWrongHart: ffffffff fedcba98 76543210 80000000\n"
        );
        assert!(slot.named_fault().is_none());
        assert!(slot.take().is_none());
    }

    #[test]
    fn a_fresh_boot_claim_replaces_unconsumed_panic_or_fault_records() {
        let mut previous_named = Slot::EMPTY;
        previous_named.record_named_fault(FaultReason::OutsideExecutor, [1; 4]);
        let mut previous_invalid_hart = Slot::EMPTY;
        previous_invalid_hart.record_invalid_hart(u32::MAX);
        let storage = RefCell::new(Slot::EMPTY);
        let (panic, _) = record_in(storage.try_borrow_mut().ok(), 0, false, 0, || {
            PanicDetails {
                file: "old.rs",
                line: 1,
                column: 2,
                message: None,
            }
        })
        .expect("previous boot recorded a panic");
        for previous in [previous_named, previous_invalid_hart, *panic] {
            // RTC bytes survive; the SRAM claim starts unheld each boot.
            // No take_previous or successful console output precedes this fault.
            let next_boot = RefCell::new(previous);
            let current = record_named_fault_in(
                next_boot.try_borrow_mut().ok(),
                FaultReason::TimerBankUnavailable,
                [u32::MAX, 2, 3, 4],
            )
            .expect("current boot replaces stale valid bytes");
            assert!(
                record_named_fault_in(
                    next_boot.try_borrow_mut().ok(),
                    FaultReason::InvalidExecutor,
                    [9; 4],
                )
                .is_none(),
                "a nested writer cannot replace the current record"
            );
            let reboot = RefCell::new(*current);
            assert_eq!(
                reboot.borrow().named_fault(),
                Some(("TimerBankUnavailable", [u32::MAX, 2, 3, 4]))
            );
        }
    }

    #[test]
    fn corrupt_and_unknown_numeric_faults_are_rejected() {
        let mut slot = Slot::EMPTY;
        slot.record_named_fault(FaultReason::InvalidExecutor, [255, 0, 0, 0]);
        slot.hart ^= 1;
        assert!(slot.named_fault().is_none());
        slot.record_named_fault(FaultReason::InvalidExecutor, [255, 0, 0, 0]);
        slot.message_length = u32::MAX;
        slot.checksum = slot.sum();
        assert!(slot.named_fault().is_none());
        assert_eq!(core::mem::size_of::<Slot>(), 148);
    }
    #[test]
    fn hil_faults_retain_their_reason_and_exact_observations() {
        for (reason, name) in [
            (FaultReason::HilReceiverLimit, "HilReceiverLimit"),
            (FaultReason::HilConsole, "HilConsole"),
            (FaultReason::HilCheckpoint, "HilCheckpoint"),
            (FaultReason::HilRateOverflow, "HilRateOverflow"),
            (FaultReason::HilHealthCount, "HilHealthCount"),
        ] {
            let mut slot = Slot::EMPTY;
            let observations = [u32::MAX, 0, 16, 1];
            slot.record_named_fault(reason, observations);
            assert_eq!(slot.named_fault(), Some((name, observations)));
            assert_eq!(report_named_fault(&mut slot, |_, _| Err(())), Err(()));
            assert_eq!(slot.named_fault(), Some((name, observations)));
            report_named_fault(&mut slot, |kept, words| {
                assert_eq!((kept, words), (name, observations));
                Ok::<_, ()>(())
            })
            .unwrap();
            assert_eq!(slot.named_fault(), None);
        }
    }
}
