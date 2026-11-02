//! LP-fast selection and its named switch settle.
use crate::{
    Access, BootFailure, ClockFailure,
    wait::{BootDeadline, BootWait},
};
use oer_time::Clock;

/// The chip-selected source and the vendor's switch settle.
pub struct LpFastProfile<S> {
    pub source: S,
    pub settle: BootWait,
}

/// One selector write or read per call; codes belong to the chip.
pub trait LpFastRegisters {
    type Source: Copy;
    fn write_source(&mut self, access: Access<'_>, source: Self::Source);
    fn source(&self, access: Access<'_>) -> u32;
    fn source_code(source: Self::Source) -> u32;
}

/// Select, settle, then require an identical readback, without fallback.
/// The selector is written even when the common deadline cannot admit the settle.
pub fn select_lp_fast<R: LpFastRegisters, C: Clock + ?Sized>(
    registers: &mut R,
    profile: LpFastProfile<R::Source>,
    deadline: &BootDeadline<'_, C>,
) -> Result<(), BootFailure> {
    registers.write_source(Access::new(), profile.source);
    deadline.hold(profile.settle)?;
    let observed = registers.source(Access::new());
    let expected = R::source_code(profile.source);
    if observed != expected {
        return Err(ClockFailure::LpFastReadback { expected, observed }.into());
    }
    Ok(())
}
