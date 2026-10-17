//! The image's one panic entry.
//!
//! The handler records the panic in `.rtc_fast.persistent`, RTC fast memory no
//! reset entry initializes, without formatting: the location's file bytes (the
//! last [`FILE_BYTES`]), line and column, the message when it is a static
//! string (its first [`MESSAGE_BYTES`]; a formatted message is only marked),
//! the hart, whether it ran on the interrupt stack and, there, the interrupted
//! PC. Interrupt-context panics omit textual metadata: compiler location and
//! format constants may live in cached PSRAM even when their code is in SRAM.
//! Every step is a bounded copy, so the path stays a short leaf in every
//! context's stack bound.
//!
//! An image with the `panic-hook` feature defines `oer_platform_panic_hook`,
//! which the entry calls after the record: it records more of the image's
//! state the same way, without formatting, and returns. The chip then resets;
//! the next boot takes the record with [`take_previous`] and reports it in
//! thread context.
use core::cell::UnsafeCell;

/// Bytes of the location's file path kept: its end, which names the file.
pub const FILE_BYTES: usize = 48;
/// Bytes of a static panic message kept: its start.
pub const MESSAGE_BYTES: usize = 64;
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
        let bytes = &self.file[..usize::from(self.file_length)];
        // The kept suffix may start inside a UTF-8 sequence; skip to the
        // first boundary.
        let start = (0..bytes.len())
            .find(|&i| core::str::from_utf8(&bytes[i..]).is_ok())
            .unwrap_or(bytes.len());
        core::str::from_utf8(&bytes[start..]).unwrap_or("")
    }

    /// The start of the panic's message when it was a static string; `None`
    /// for a formatted one.
    pub fn message(&self) -> Option<&str> {
        let Message::Static(bytes, length) = &self.message else {
            return None;
        };
        let bytes = &bytes[..usize::from(*length)];
        // The kept prefix may end inside a UTF-8 sequence; drop that part.
        let end = (0..=bytes.len())
            .rev()
            .find(|&i| core::str::from_utf8(&bytes[..i]).is_ok())
            .unwrap_or(0);
        Some(core::str::from_utf8(&bytes[..end]).unwrap_or(""))
    }
}

impl core::fmt::Display for PanicRecord {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(
            f,
            "{}:{}:{} on hart {}",
            self.file(),
            self.line,
            self.column,
            self.hart
        )?;
        if let Some(pc) = self.interrupted_pc {
            write!(f, " in an interrupt of {pc:#010x}")?;
        }
        match self.message() {
            Some(message) => write!(f, ": {message}"),
            None => write!(f, " (formatted message not kept)"),
        }
    }
}

/// The retained words: a magic, the record and a checksum over both.
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
    message_length: u32,
    message: [u8; MESSAGE_BYTES],
    checksum: u32,
}

impl Slot {
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

    #[unsafe(link_section = ".flash.critical.text.panic")]
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

struct Retained(UnsafeCell<Slot>);

// SAFETY: the panic handler writes the slot on the panicking hart while the
// image stops; `take_previous` runs once at boot, before any panic can.
unsafe impl Sync for Retained {}

// Every bit pattern is a `Slot`; the NOLOAD section never loads the initializer.
#[unsafe(link_section = ".rtc_fast.persistent")]
static SLOT: Retained = Retained(UnsafeCell::new(Slot::EMPTY));

/// Take the record the previous boot's panic left, and clear it. Call once,
/// early at boot and before anything can panic; `None` when the previous
/// boot did not panic or the chip lost power.
pub fn take_previous() -> Option<PanicRecord> {
    // SAFETY: called once at boot, before any panic can write the slot.
    let slot = unsafe { &mut *SLOT.0.get() };
    let valid = slot.magic == MAGIC
        && slot.checksum == slot.sum()
        && slot.file_length as usize <= FILE_BYTES
        && slot.message_length as usize <= MESSAGE_BYTES + 1;
    let record = valid.then(|| PanicRecord {
        hart: slot.hart as u8,
        in_interrupt: slot.in_interrupt != 0,
        interrupted_pc: (slot.in_interrupt != 0).then_some(slot.interrupted_pc),
        line: slot.line,
        column: slot.column,
        file: slot.file,
        file_length: slot.file_length as u8,
        message: match slot.message_length {
            0 => Message::Formatted,
            length => Message::Static(slot.message, (length - 1) as u8),
        },
    });
    slot.magic = 0;
    record
}

/// Record `info` in the retained slot without formatting.
#[unsafe(link_section = ".flash.critical.text.panic")]
fn record<'a>(info: &'a core::panic::PanicInfo<'_>) -> PanicDetails<'a> {
    let hart: usize;
    let sp: usize;
    let mepc: usize;
    // SAFETY: CSR and register reads only.
    unsafe {
        core::arch::asm!(
            "csrr {hart}, mhartid",
            "mv {sp}, sp",
            "csrr {mepc}, mepc",
            hart = out(reg) hart,
            sp = out(reg) sp,
            mepc = out(reg) mepc,
            options(nomem, nostack),
        )
    };
    let in_interrupt = crate::stacks::on_interrupt_stack(hart, sp);
    // SAFETY: the other hart may hold any lock while this one panics, so the
    // slot is written without one; the image stops after this write.
    let slot = unsafe { &mut *SLOT.0.get() };
    let details = if in_interrupt {
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
    };
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
    details
}

#[cfg(feature = "panic-hook")]
unsafe extern "Rust" {
    /// The image's record of its own state after the platform's record. It
    /// formats nothing, like the entry: it runs in every context's stack
    /// bound.
    fn oer_platform_panic_hook(details: &PanicDetails<'_>);
}

#[panic_handler]
#[unsafe(link_section = ".flash.critical.text.panic")]
fn panic(info: &core::panic::PanicInfo<'_>) -> ! {
    let details = record(info);
    #[cfg(feature = "panic-hook")]
    // SAFETY: the image defines this function with the declared signature;
    // the link fails without it.
    unsafe {
        oer_platform_panic_hook(&details)
    };
    #[cfg(not(feature = "panic-hook"))]
    let _ = details;
    esp_hal::system::software_reset()
}

/// The last [`FILE_BYTES`] of `file`, zero-padded, and how many there are.
/// It copies through iterators, without an index whose bounds check could
/// fail: a panic while recording re-enters the handler, a call-graph cycle
/// that leaves every interrupt's stack bound unknown.
#[unsafe(link_section = ".flash.critical.text.panic")]
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
#[unsafe(link_section = ".flash.critical.text.panic")]
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
    use super::*;

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
}
