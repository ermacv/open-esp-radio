//! LP-fast selection and its named switch settle.
use crate::{
    Access, BootFailure,
    wait::{BootDeadline, BootWait},
};
use oer_portable_clock::Clock;

/// The chip-selected source and the vendor's switch settle.
pub struct LpFastProfile<S> {
    pub source: S,
    pub settle: BootWait,
}

/// One selector write per call; the source's names belong to the chip.
pub trait LpFastRegisters {
    type Source: Copy;
    fn write_source(&mut self, access: Access<'_>, source: Self::Source);
}

/// Select, then hold the switch's settle, as the vendor's
/// `rtc_clk_fast_src_set` writes and waits: the selector is not read back.
/// The selector is written even when the common deadline cannot admit the settle.
pub fn select_lp_fast<R: LpFastRegisters, C: Clock + ?Sized>(
    registers: &mut R,
    profile: LpFastProfile<R::Source>,
    deadline: &BootDeadline<'_, C>,
) -> Result<(), BootFailure> {
    registers.write_source(Access::new(), profile.source);
    deadline.hold(profile.settle)?;
    Ok(())
}
