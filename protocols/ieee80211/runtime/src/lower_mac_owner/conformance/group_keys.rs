//! Shared acceptance of GTK RX boundaries. A backend fixture captures actual
//! installed identities, counters, hardware writes and delivered events.
//! Delayed DMA/ISR/software RX is injected before each accepted mutation;
//! continuous ingress must remain enabled until its terminal is delivered.
//! The independent harness timer bounds setup, execution and teardown even
//! when a broken backend never emits an event. Measurements use the backend's
//! admission and completion instants, not the time the entire fixture ends.

use alloc::vec::Vec;
use embassy_futures::select::{Either, select};
use oer_time::{Duration, Timer};

/// The mutation whose terminal establishes the cut.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Case {
    InstallBeside,
    Replace,
    SelectRetained,
    SelectCurrent,
}

/// Observation of an actual installed owner, never an expected fixture ID.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Installation {
    pub identity: usize,
    pub slot: u8,
    pub counter: u64,
}

/// A fact delivered through the backend's public event stream.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Event {
    OldRx,
    Terminal(Installation),
    NewRx,
}

pub struct Trace {
    /// The target before submission for selection, old owner for replacement.
    pub original: Installation,
    /// Slot of the actual owner submitted with the command, including the
    /// vacant slot for InstallBeside. Capture it before transferring ownership.
    pub submitted_slot: u8,
    pub timing: Timing,
    pub events: Vec<Event>,
    /// Material writes performed by this operation only.
    pub writes: usize,
    pub inactive_material_accepted: bool,
    pub continuous_rx_at_terminal: bool,
}

/// One admission-to-completion measurement on the backend's monotonic clock.
#[derive(Clone, Copy, Debug)]
pub struct Timing {
    pub admitted_at: oer_time::Instant,
    pub completed_at: oer_time::Instant,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Mutation {
    Install,
    Replace,
    Select,
    Remove,
}
impl Mutation {
    pub const ALL: [Self; 4] = [Self::Install, Self::Replace, Self::Select, Self::Remove];
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProgressCase {
    /// Drain an accepted command, both before and after its hardware change.
    Disable { mutation: Mutation, applied: bool },
    /// Hold hardware progress through the command's declared deadline.
    Expire(Mutation),
    /// A published AP group transmission retains a use of the removed key.
    PublishedRemoval,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Scenario {
    Boundary(Case),
    Progress(ProgressCase),
}

/// Captured from the actual admitted/returned/quarantined owners.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Owners {
    pub slot: u8,
    pub permission: u8,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TxOwners {
    pub ticket: u8,
    pub buffer: usize,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Completion {
    Group { mutation: Mutation, owners: Owners },
    Transmit(TxOwners),
}

pub struct ProgressTrace {
    pub admitted: Owners,
    pub timing: Timing,
    pub events: Vec<Completion>,
    /// Actual command/terminal/removal custody before poisoned teardown.
    pub retained: Vec<Owners>,
    pub poisoned: bool,
    /// Measure the operation's registered waker, excluding the harness timer.
    pub deadline_woke: bool,
    pub published: Option<TxOwners>,
    /// No removal terminal while the captured AP publication remains in flight.
    pub blocked_while_published: bool,
}

/// A hardware fixture must inject all ingress stages, not just prepend a
/// synthetic event before a command acknowledgement.
pub trait Harness {
    /// The backend's actual LowerMacBackend::GROUP_COMMAND_BOUND.
    const GROUP_COMMAND_BOUND: Duration;
    /// Finite budget including fixture setup and teardown, on `check`'s timer.
    const SCENARIO_BOUND: Duration;
    fn exercise(&mut self, case: Case) -> impl core::future::Future<Output = Trace>;
    fn exercise_progress(
        &mut self,
        case: ProgressCase,
    ) -> impl core::future::Future<Output = ProgressTrace>;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Violation {
    RxOrder,
    Identity,
    Counter,
    Reinstallation,
    InactiveMaterial,
    UnboundedIngress,
    CommandBound,
    HarnessTimeout,
    Custody,
    DeadlineWake,
    Publication,
}

pub async fn check<H: Harness>(
    harness: &mut H,
    timer: &impl Timer,
) -> Result<(), (Scenario, Violation)> {
    for case in [
        Case::InstallBeside,
        Case::Replace,
        Case::SelectRetained,
        Case::SelectCurrent,
    ] {
        let trace = bounded(harness.exercise(case), timer, H::SCENARIO_BOUND)
            .await
            .map_err(|why| (Scenario::Boundary(case), why))?;
        verify(case, trace, H::GROUP_COMMAND_BOUND)
            .map_err(|why| (Scenario::Boundary(case), why))?;
    }
    for case in Mutation::ALL
        .into_iter()
        .flat_map(|mutation| {
            [
                ProgressCase::Disable {
                    mutation,
                    applied: false,
                },
                ProgressCase::Disable {
                    mutation,
                    applied: true,
                },
                ProgressCase::Expire(mutation),
            ]
        })
        .chain([ProgressCase::PublishedRemoval])
    {
        let trace = bounded(harness.exercise_progress(case), timer, H::SCENARIO_BOUND)
            .await
            .map_err(|why| (Scenario::Progress(case), why))?;
        verify_progress(case, trace, H::GROUP_COMMAND_BOUND)
            .map_err(|why| (Scenario::Progress(case), why))?;
    }
    Ok(())
}

async fn bounded<T>(
    operation: impl core::future::Future<Output = T>,
    timer: &impl Timer,
    bound: Duration,
) -> Result<T, Violation> {
    let deadline = timer
        .now()
        .checked_add(bound)
        .ok_or(Violation::HarnessTimeout)?;
    match select(operation, timer.wait_until(deadline)).await {
        Either::First(trace) if timer.now() <= deadline => Ok(trace),
        Either::First(_) | Either::Second(()) => Err(Violation::HarnessTimeout),
    }
}

fn verify(case: Case, trace: Trace, bound: Duration) -> Result<(), Violation> {
    verify_timing(trace.timing, bound)?;
    let [
        Event::OldRx,
        Event::OldRx,
        Event::OldRx,
        Event::OldRx,
        Event::Terminal(returned),
        Event::NewRx,
    ] = trace.events.as_slice()
    else {
        return Err(Violation::RxOrder);
    };
    let selected = matches!(case, Case::SelectRetained | Case::SelectCurrent);
    let identity_matches = match case {
        Case::InstallBeside => {
            returned.identity != trace.original.identity
                && trace.submitted_slot != trace.original.slot
        }
        Case::Replace => {
            returned.identity != trace.original.identity
                && trace.submitted_slot == trace.original.slot
        }
        Case::SelectRetained | Case::SelectCurrent => {
            returned.identity == trace.original.identity
                && trace.submitted_slot == trace.original.slot
        }
    };
    if !identity_matches || returned.slot != trace.submitted_slot {
        return Err(Violation::Identity);
    }
    if selected && returned.counter != trace.original.counter {
        return Err(Violation::Counter);
    }
    if trace.writes != usize::from(!selected) {
        return Err(Violation::Reinstallation);
    }
    if trace.inactive_material_accepted {
        return Err(Violation::InactiveMaterial);
    }
    if !trace.continuous_rx_at_terminal {
        return Err(Violation::UnboundedIngress);
    }
    Ok(())
}

fn verify_timing(timing: Timing, bound: Duration) -> Result<(), Violation> {
    if bound == Duration::ZERO
        || timing
            .completed_at
            .checked_duration_since(timing.admitted_at)
            .is_none_or(|elapsed| elapsed > bound)
    {
        return Err(Violation::CommandBound);
    }
    Ok(())
}

fn verify_progress(
    case: ProgressCase,
    trace: ProgressTrace,
    bound: Duration,
) -> Result<(), Violation> {
    verify_timing(trace.timing, bound)?;
    match case {
        ProgressCase::Expire(_) => {
            if !trace.poisoned || !trace.events.is_empty() || trace.retained != [trace.admitted] {
                return Err(Violation::Custody);
            }
            if trace.timing.admitted_at.checked_add(bound) != Some(trace.timing.completed_at) {
                return Err(Violation::CommandBound);
            }
            if !trace.deadline_woke {
                return Err(Violation::DeadlineWake);
            }
        }
        ProgressCase::Disable { mutation, .. } => {
            if trace.poisoned
                || !trace.retained.is_empty()
                || trace.events
                    != [Completion::Group {
                        mutation,
                        owners: trace.admitted,
                    }]
            {
                return Err(Violation::Custody);
            }
        }
        ProgressCase::PublishedRemoval => {
            if trace.poisoned || !trace.retained.is_empty() {
                return Err(Violation::Custody);
            }
            let Some(published) = trace.published else {
                return Err(Violation::Publication);
            };
            if !trace.blocked_while_published
                || trace.events
                    != [
                        Completion::Transmit(published),
                        Completion::Group {
                            mutation: Mutation::Remove,
                            owners: trace.admitted,
                        },
                    ]
            {
                return Err(Violation::Publication);
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_valid_rx_cut_after_the_command_bound_is_rejected() {
        for completed_at in [110, 111] {
            let original = Installation {
                identity: 7,
                slot: 2,
                counter: 73,
            };
            let trace = Trace {
                original,
                submitted_slot: 2,
                timing: Timing {
                    admitted_at: oer_time::Instant::from_micros(100),
                    completed_at: oer_time::Instant::from_micros(completed_at),
                },
                events: alloc::vec![
                    Event::OldRx,
                    Event::OldRx,
                    Event::OldRx,
                    Event::OldRx,
                    Event::Terminal(original),
                    Event::NewRx
                ],
                writes: 0,
                inactive_material_accepted: false,
                continuous_rx_at_terminal: true,
            };
            assert_eq!(
                verify(Case::SelectCurrent, trace, Duration::from_micros(10)),
                if completed_at == 110 {
                    Ok(())
                } else {
                    Err(Violation::CommandBound)
                }
            );
        }
    }

    #[test]
    fn a_backend_without_a_terminal_cannot_hang_the_shared_suite() {
        struct Never;
        impl Harness for Never {
            const GROUP_COMMAND_BOUND: Duration = Duration::from_millis(1);
            const SCENARIO_BOUND: Duration = Duration::from_millis(10);
            async fn exercise(&mut self, _: Case) -> Trace {
                core::future::pending().await
            }
            async fn exercise_progress(&mut self, _: ProgressCase) -> ProgressTrace {
                core::future::pending().await
            }
        }
        let timer = oer_time_virtual::SkipClock::new();
        let mut harness = Never;
        let mut check = core::pin::pin!(check(&mut harness, &timer));
        assert_eq!(
            core::future::Future::poll(
                check.as_mut(),
                &mut core::task::Context::from_waker(core::task::Waker::noop())
            ),
            core::task::Poll::Ready(Err((
                Scenario::Boundary(Case::InstallBeside),
                Violation::HarnessTimeout
            )))
        );
    }

    #[test]
    fn expiry_requires_a_deadline_wake_and_exact_poisoned_custody() {
        for fault in 0..5 {
            let admitted = Owners {
                slot: 2,
                permission: 0,
            };
            let mut trace = ProgressTrace {
                admitted,
                timing: Timing {
                    admitted_at: oer_time::Instant::EPOCH,
                    completed_at: oer_time::Instant::from_micros(10),
                },
                events: Vec::new(),
                retained: alloc::vec![admitted],
                poisoned: true,
                deadline_woke: true,
                published: None,
                blocked_while_published: false,
            };
            let expected = match fault {
                0 => {
                    trace.deadline_woke = false;
                    Err(Violation::DeadlineWake)
                }
                1 => {
                    trace.retained.clear();
                    Err(Violation::Custody)
                }
                2 => {
                    trace.retained.push(admitted);
                    Err(Violation::Custody)
                }
                3 => {
                    trace.events.push(Completion::Group {
                        mutation: Mutation::Install,
                        owners: admitted,
                    });
                    Err(Violation::Custody)
                }
                _ => Ok(()),
            };
            assert_eq!(
                verify_progress(
                    ProgressCase::Expire(Mutation::Install),
                    trace,
                    Duration::from_micros(10)
                ),
                expected
            );
        }
    }

    #[test]
    fn installation_beside_requires_a_new_identity_in_the_submitted_vacant_slot() {
        let original = Installation {
            identity: 7,
            slot: 0,
            counter: 73,
        };
        for (returned, expected) in [
            (original, Err(Violation::Identity)),
            (
                Installation {
                    identity: 8,
                    slot: 0,
                    counter: 0,
                },
                Err(Violation::Identity),
            ),
            (
                Installation {
                    identity: 7,
                    slot: 2,
                    counter: 0,
                },
                Err(Violation::Identity),
            ),
            (
                Installation {
                    identity: 8,
                    slot: 1,
                    counter: 0,
                },
                Err(Violation::Identity),
            ),
            (
                Installation {
                    identity: 8,
                    slot: 2,
                    counter: 0,
                },
                Ok(()),
            ),
        ] {
            let trace = Trace {
                original,
                submitted_slot: 2,
                timing: Timing {
                    admitted_at: oer_time::Instant::EPOCH,
                    completed_at: oer_time::Instant::from_micros(1),
                },
                events: alloc::vec![
                    Event::OldRx,
                    Event::OldRx,
                    Event::OldRx,
                    Event::OldRx,
                    Event::Terminal(returned),
                    Event::NewRx
                ],
                writes: 1,
                inactive_material_accepted: false,
                continuous_rx_at_terminal: true,
            };
            assert_eq!(
                verify(Case::InstallBeside, trace, Duration::from_micros(1)),
                expected
            );
        }
    }
}
