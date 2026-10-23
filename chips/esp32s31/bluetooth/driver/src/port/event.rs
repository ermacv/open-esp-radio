//! Packet accounting after a natural terminal or a confirmed host stop.
use super::Fault;
use oer_esp32s31_hal::bluetooth::engine::DtmSchedulerItemCompletionStatus as Status;

pub(super) enum Finish {
    Natural,
    Stopped,
}
pub(super) fn successful(status: Status, finish: Finish) -> Result<bool, Fault> {
    match (status, finish) {
        (Status::Zero, _) => Ok(true),
        (Status::Aborted, _) | (Status::NonZero(_), Finish::Stopped) => Ok(false),
        (Status::NonZero(_), Finish::Natural) => Err(Fault::DtmStatus),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::num::NonZeroU32;
    #[test]
    fn a_natural_hardware_failure_is_not_a_packet() {
        assert_eq!(
            successful(
                Status::NonZero(NonZeroU32::new(7).expect("nonzero")),
                Finish::Natural
            ),
            Err(Fault::DtmStatus)
        );
    }
    #[test]
    fn a_confirmed_stop_reclaims_the_event_without_counting_it() {
        assert_eq!(
            successful(
                Status::NonZero(NonZeroU32::new(7).expect("nonzero")),
                Finish::Stopped
            ),
            Ok(false)
        );
        assert_eq!(successful(Status::Aborted, Finish::Stopped), Ok(false));
        assert_eq!(successful(Status::Zero, Finish::Stopped), Ok(true));
    }
}
