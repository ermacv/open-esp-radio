//! Timing gate for the AP aggregate owner. Only `FinishAbort` permits detach.

use super::{AccessPointDatapathError, AggregateTxServiceEvent, ApAmpduError, WifiTxWake};

/// Reuse one deadline for publication and abort settling; never retain a timer
/// future alongside the DMA transaction.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum AggregateServicePhase {
    Published(oer_time::Instant),
    AbortSettling(oer_time::Instant),
    ResetRequired,
    /// The aggregate's agreement ended and its missing MPDUs leave as
    /// ordinary frames; the ordinary owner's deadline and completion drive it.
    Unaggregating,
    /// The retained aggregate waits for the answer to its BlockAckReq; the
    /// ordinary owner's deadline and completion drive it.
    RequestingBlockAck,
}

#[derive(Debug, PartialEq)]
pub(super) enum AggregateServiceAction {
    Wait,
    Observe(AggregateTxServiceEvent),
    FinishAbort,
}

impl AggregateServicePhase {
    pub(super) fn deadline(self) -> oer_time::Instant {
        match self {
            Self::Published(deadline) | Self::AbortSettling(deadline) => deadline,
            // Never: these phases wait for an event, not a time.
            Self::ResetRequired | Self::Unaggregating | Self::RequestingBlockAck => {
                oer_time::Instant::from_micros(u64::MAX)
            }
        }
    }

    pub(super) fn after_abort(now: oer_time::Instant) -> Result<Self, AccessPointDatapathError> {
        now.checked_add(oer_time::Duration::from_micros(16))
            .map(Self::AbortSettling)
            .ok_or(AccessPointDatapathError::Aggregate(
                ApAmpduError::DeadlineOverflow,
            ))
    }

    pub(super) fn action(
        self,
        wake: WifiTxWake,
        now: impl FnOnce() -> Result<oer_time::Instant, AccessPointDatapathError>,
    ) -> Result<AggregateServiceAction, AccessPointDatapathError> {
        match self {
            Self::ResetRequired => Err(AccessPointDatapathError::Aggregate(
                ApAmpduError::DeadlineOverflow,
            )),
            // Serviced through the ordinary owner before any aggregate action.
            Self::Unaggregating | Self::RequestingBlockAck => Ok(AggregateServiceAction::Wait),
            Self::AbortSettling(deadline) => {
                // The abort already happened. Repeated/conflicting IRQs and
                // late completion cannot bypass the physical settle interval.
                Ok(if now()? < deadline {
                    AggregateServiceAction::Wait
                } else {
                    AggregateServiceAction::FinishAbort
                })
            }
            Self::Published(deadline) => {
                let mut event = AggregateTxServiceEvent::classify(wake).map_err(|error| {
                    AccessPointDatapathError::Aggregate(ApAmpduError::ConflictingInterruptEvents(
                        error.events,
                    ))
                })?;
                if event == AggregateTxServiceEvent::ExecutorDeadline && now()? < deadline {
                    // Still observe actual completion; do not manufacture a timeout.
                    event = AggregateTxServiceEvent::Pending;
                }
                Ok(AggregateServiceAction::Observe(event))
            }
        }
    }
}

#[cfg(test)]
mod tests;
