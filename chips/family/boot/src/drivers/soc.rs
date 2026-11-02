//! The existing SoC-init boundary, before watchdog takeover and brownout.
use super::{access_control, cores, entropy};
use crate::wait::{BootDeadline, WaitFailure};
use oer_time::{Clock, Duration};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SocFailure {
    Deadline(WaitFailure),
}

/// Preserve containment → filters → TRNG and the single initial deadline check.
pub fn initialize<R, C>(
    registers: &mut R,
    profile: &entropy::Profile<R::Noise, R::Cutoff>,
    deadline: &BootDeadline<'_, C>,
) -> Result<(), SocFailure>
where
    R: cores::Registers + access_control::Registers + entropy::Registers,
    C: Clock + ?Sized,
{
    if deadline.remaining() == Duration::ZERO {
        return Err(SocFailure::Deadline(WaitFailure::DeadlineExceeded(
            "soc-init",
        )));
    }
    cores::contain(registers);
    access_control::open(registers);
    entropy::start(registers, profile);
    Ok(())
}
