//! Admission for returning a drained timer task without losing pending work.

/// Why the timer task cannot return its queue and hardware owner yet.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ControllerModemTimerRetirementError<E> {
    /// Software work, expiration publication or owner rearm is retained by the task.
    TaskActive,
    /// A scheduled software deadline still has an owner.
    QueuePending,
    /// The durable ISR-to-task handoff has not been acquired.
    WorkerPending,
    /// A published expiration still awaits its consumer.
    ExpirationPending,
    /// Stable storage could not return an ISR-ready owner with routes inactive.
    Storage(E),
}

pub(crate) fn retire_when_drained<Owner, Error>(
    task_idle: bool,
    queue_empty: bool,
    worker_pending: bool,
    event_pending: bool,
    take: impl FnOnce() -> Result<Owner, Error>,
) -> Result<Owner, ControllerModemTimerRetirementError<Error>> {
    use ControllerModemTimerRetirementError as Rejection;
    if !task_idle {
        return Err(Rejection::TaskActive);
    }
    if !queue_empty {
        return Err(Rejection::QueuePending);
    }
    if worker_pending {
        return Err(Rejection::WorkerPending);
    }
    if event_pending {
        return Err(Rejection::ExpirationPending);
    }
    take().map_err(Rejection::Storage)
}

#[cfg(test)]
mod tests {
    use crate::modem_timer_retirement::*;

    #[test]
    fn every_outstanding_obligation_preserves_hardware_storage() {
        for (idle, empty, wake, event, expected) in [
            (
                false,
                true,
                false,
                false,
                ControllerModemTimerRetirementError::TaskActive,
            ),
            (
                true,
                false,
                false,
                false,
                ControllerModemTimerRetirementError::QueuePending,
            ),
            (
                true,
                true,
                true,
                false,
                ControllerModemTimerRetirementError::WorkerPending,
            ),
            (
                true,
                true,
                false,
                true,
                ControllerModemTimerRetirementError::ExpirationPending,
            ),
        ] {
            assert_eq!(
                retire_when_drained(idle, empty, wake, event, || -> Result<(), ()> {
                    panic!("pending software obligations must not take hardware ownership")
                }),
                Err(expected)
            );
        }
    }

    #[test]
    fn storage_rejection_is_retryable_without_synthetic_owner() {
        assert_eq!(
            retire_when_drained::<u32, _>(true, true, false, false, || Err("routes live")),
            Err(ControllerModemTimerRetirementError::Storage("routes live"))
        );
        let owner = std::boxed::Box::new(107);
        let identity = core::ptr::from_ref(&*owner);
        let returned =
            retire_when_drained(true, true, false, false, || Ok::<_, ()>(owner)).unwrap();
        assert_eq!(identity, core::ptr::from_ref(&*returned));
    }
}
