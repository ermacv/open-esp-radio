//! The stage-two handoff contract between a board and the staged runtime,
//! and the entry a secondary hart's launch hands the ROM.
//!
//! The bootstrap maps the board's PSRAM and copies stage two into it; stage
//! two runs from that mapping from its first instruction. Adopting it must
//! therefore neither reset nor remap PSRAM, and must happen once, on CPU0,
//! right after the runtime's entry, with interrupts disabled. A board states
//! that its adoption does exactly that by implementing
//! [`AdoptsBootstrapMapping`]; the runtime's handoff is the only caller,
//! with the one [`StageTwoHandoff`] it gives.
//!
//! A secondary hart enters only through the runtime's generated entry for it,
//! whose address the runtime alone mints as a [`SecondaryEntry`].

/// Proof that stage two runs on CPU0 right after the runtime's entry, with
/// interrupts disabled and the bootstrap's mapping intact: the moment the
/// board may adopt the mapping. The staged runtime gives it once per boot.
#[derive(Debug)]
pub struct StageTwoHandoff {
    _once: (),
}

impl StageTwoHandoff {
    /// The handoff's one proof.
    ///
    /// # Safety
    ///
    /// Call once per boot, on CPU0, before anything resets or remaps PSRAM
    /// and while interrupts are disabled: the staged runtime's entry does.
    #[allow(
        unsafe_code,
        reason = "minting the proof is the runtime entry's promise"
    )]
    #[doc(hidden)]
    pub const unsafe fn new() -> Self {
        Self { _once: () }
    }
}

/// A board's adoption of the bootstrap's PSRAM mapping.
///
/// # Safety
///
/// `adopt` takes over the mapping the bootstrap left without resetting or
/// remapping PSRAM, and leaves interrupts disabled.
#[allow(unsafe_code, reason = "the trait carries the board's adoption promise")]
pub unsafe trait AdoptsBootstrapMapping {
    /// The adopted mapping, as the board hands it to the image.
    type Mapping;

    /// Adopt the mapping at the handoff `handoff` proves.
    fn adopt(self, handoff: &StageTwoHandoff) -> Self::Mapping;
}

/// The generated entry of one secondary hart, as its launch writes it to the
/// hart's ROM boot word. Only the staged runtime's entry module mints one.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SecondaryEntry {
    hart: u8,
    address: u32,
}

impl SecondaryEntry {
    /// The entry of hart `H` at `address`. The boot hart has no secondary
    /// entry: naming it fails at monomorphization.
    ///
    /// ```compile_fail,E0080
    /// use oer_espressif_staged_layout::{boot::BOOT_HART, handoff::SecondaryEntry};
    /// const ENTRY: SecondaryEntry = unsafe { SecondaryEntry::new::<{ BOOT_HART as u8 }>(0x2f00_0000) };
    /// ```
    ///
    /// # Safety
    ///
    /// `address` is the staged runtime's generated entry of `hart`, linked in
    /// internal SRAM with boot-word bit 31 clear: it masks the hart's
    /// interrupts first, installs its own trap stub, reports its entry and
    /// waits for the launch's acknowledgement before it touches anything
    /// else, then installs its own memory attributes, global pointer and
    /// stack. The ROM may jump to it with any stack and trap vector.
    #[allow(
        unsafe_code,
        reason = "minting the entry is the runtime's generated-entry promise"
    )]
    pub const unsafe fn new<const H: u8>(address: u32) -> Self {
        const {
            if H as usize == crate::boot::BOOT_HART {
                panic!("a secondary entry names a secondary hart");
            }
        }
        Self { hart: H, address }
    }

    /// The hart the entry belongs to.
    pub const fn hart(self) -> u8 {
        self.hart
    }

    /// The entry's address, the value of the hart's ROM boot word.
    pub const fn address(self) -> u32 {
        self.address
    }
}
