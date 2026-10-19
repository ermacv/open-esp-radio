//! The stage-two handoff contract between a board and the staged runtime.
//!
//! The bootstrap maps the board's PSRAM and copies stage two into it; stage
//! two runs from that mapping from its first instruction. Adopting it must
//! therefore neither reset nor remap PSRAM, and must happen once, on CPU0,
//! right after the runtime's entry, with interrupts disabled. A board states
//! that its adoption does exactly that by implementing
//! [`AdoptsBootstrapMapping`]; the runtime's handoff is the only caller,
//! with the one [`StageTwoHandoff`] it gives.

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
