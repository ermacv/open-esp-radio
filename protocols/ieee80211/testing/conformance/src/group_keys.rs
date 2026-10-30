//! Shared acceptance of GTK RX boundaries. A backend fixture captures actual
//! installed identities, counters, hardware writes and delivered events.
//! Delayed DMA/ISR/software RX is injected before each accepted mutation;
//! continuous ingress must remain enabled until its terminal is delivered.
//! The independent harness timer bounds setup, execution and teardown even
//! when a broken backend never emits an event. Measurements use the backend's
//! admission and completion instants, not the time the entire fixture ends.

use super::{Environment, Execution, ModelOnlyReason, Report};
use oer_time::{Duration, Timer};

/// The mutation whose terminal establishes the cut.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Case {
    /// Install into a vacant slot beside the old active key.
    InstallBeside,
    /// Replace the target installation in its slot.
    Replace,
    /// Select a retained installation without reinstalling its material.
    SelectRetained,
    /// Select the currently active installation without a material write.
    SelectCurrent,
}

impl Case {
    /// Every receive-cut case.
    pub const ALL: [Self; 4] = [
        Self::InstallBeside,
        Self::Replace,
        Self::SelectRetained,
        Self::SelectCurrent,
    ];
    /// This fixture injects old-key RX into DMA, interrupt and software
    /// stages independently; normal air traffic cannot establish those cuts.
    pub const fn execution(self) -> Execution {
        Execution::ModelOnly(ModelOnlyReason::DelayedIngress)
    }
}

/// Observation of an actual installed owner, never an expected fixture ID.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Installation {
    /// Identity captured from the installed key owner.
    pub identity: usize,
    /// Actual key slot identity.
    pub slot: u8,
    /// Protocol-owned PN frontier retained across this installation.
    pub counter: u64,
}

/// A fact delivered through the backend's public event stream.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Event {
    /// A frame under the old active material delivered before the cut.
    OldRx,
    /// A mutation terminal carrying the resulting installation.
    Terminal(Installation),
    /// A frame under the new active material delivered after the cut.
    NewRx,
}

/// Captured receive-cut events and installation identities.
pub struct Trace<E> {
    /// The target before submission for selection, old owner for replacement.
    pub original: Installation,
    /// Slot of the actual owner submitted with the command, including the
    /// vacant slot for InstallBeside. Capture it before transferring ownership.
    pub submitted_slot: u8,
    /// Admission and completion instants captured on the backend clock.
    pub timing: Timing,
    /// Events captured from the actual backend event stream.
    pub events: E,
    /// Material writes performed by this operation only.
    pub writes: usize,
    /// Whether material inactive after the cut still admitted a frame.
    pub inactive_material_accepted: bool,
    /// Whether ingress remained enabled through the terminal.
    pub continuous_rx_at_terminal: bool,
}

/// One admission-to-completion measurement on the backend's monotonic clock.
#[derive(Clone, Copy, Debug)]
pub struct Timing {
    /// Monotonic instant recorded at command admission.
    pub admitted_at: oer_time::Instant,
    /// Monotonic instant recorded at command completion.
    pub completed_at: oer_time::Instant,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// Kind of accepted group-key command.
pub enum Mutation {
    /// Install a group key into an available slot.
    Install,
    /// Replace the target installation in its slot.
    Replace,
    /// Select an existing installed group key.
    Select,
    /// Remove an installed group key.
    Remove,
}
impl Mutation {
    /// Every group-key mutation exercised by the progress suite.
    pub const ALL: [Self; 4] = [Self::Install, Self::Replace, Self::Select, Self::Remove];
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// A command-stage, timeout or published-key-use case.
pub enum ProgressCase {
    /// Drain an accepted command, both before and after its hardware change.
    Disable {
        #[doc = "The accepted group-key mutation."]
        mutation: Mutation,
        #[doc = "Whether the hardware change was already applied at disable."]
        applied: bool,
    },
    /// Hold hardware progress through the command's declared deadline.
    Expire(Mutation),
    /// A published AP group transmission retains a use of the removed key.
    PublishedRemoval,
    /// Reject the new GTK write, restore the old entry, then fail-stop.
    ReplacementRollback,
}

impl ProgressCase {
    /// Every before/after-disable, command-expiry and published-key-use case.
    pub fn all() -> impl Iterator<Item = Self> {
        Mutation::ALL
            .into_iter()
            .flat_map(|mutation| {
                [
                    Self::Disable {
                        mutation,
                        applied: false,
                    },
                    Self::Disable {
                        mutation,
                        applied: true,
                    },
                    Self::Expire(mutation),
                ]
            })
            .chain([Self::PublishedRemoval, Self::ReplacementRollback])
    }
    /// A healthy hardware key command cannot be held past its bound on air.
    pub const fn execution(self) -> Execution {
        match self {
            Self::Expire(_) => Execution::ModelOnly(ModelOnlyReason::CommandStall),
            _ => Execution::Hardware,
        }
    }
}

/// A receive-cut or command-progress case of this suite.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Scenario {
    /// A receive-cut case.
    Boundary(Case),
    /// A command progress/custody case.
    Progress(ProgressCase),
}

impl Scenario {
    /// Execution requirements of the contained receive-cut or progress case.
    pub const fn execution(self) -> Execution {
        match self {
            Self::Boundary(case) => case.execution(),
            Self::Progress(case) => case.execution(),
        }
    }
}

/// Captured from the actual admitted/returned/quarantined owners.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Owners {
    /// Actual key slot identity.
    pub slot: u8,
    /// Actual group-command permission identity.
    pub permission: u8,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// Ticket and buffer identities of a published group MPDU.
pub struct TxOwners {
    /// Actual transmit ticket slot identity.
    pub ticket: u8,
    /// Identity of the actual buffer owner.
    pub buffer: usize,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// Actual terminal delivery observed by the progress fixture.
pub enum Completion {
    /// A group-key mutation terminal.
    Group {
        #[doc = "The accepted group-key mutation."]
        mutation: Mutation,
        #[doc = "Returned slot and permission identities."]
        owners: Owners,
    },
    /// The transmit terminal that releases a published key use.
    Transmit(TxOwners),
}

/// Observed hardware/model state before poisoned teardown erases the table.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Rollback {
    /// KeyID of the installed entry consumed by replacement.
    pub previous_key_id: u8,
    /// KeyID of the rejected replacement.
    pub attempted_key_id: u8,
    /// The old key publication completed after rejecting the new one.
    pub restored: bool,
}

/// Captured command progress, terminal order and quarantine custody.
pub struct ProgressTrace<E, R> {
    /// Actual internal rollback, independent of portable success terminals.
    pub rollback: Option<Rollback>,
    /// Slot and permission identities captured before command submission.
    pub admitted: Owners,
    /// Admission and completion instants captured on the backend clock.
    pub timing: Timing,
    /// Events captured from the actual backend event stream.
    pub events: E,
    /// Actual command/terminal/removal custody before poisoned teardown.
    pub retained: R,
    /// Whether the operation poisoned the installation.
    pub poisoned: bool,
    /// Measure the operation's registered waker, excluding the harness timer.
    pub deadline_woke: bool,
    /// Actual AP group MPDU retaining a use of the key, if any.
    pub published: Option<TxOwners>,
    /// No removal terminal while the captured AP publication remains in flight.
    pub blocked_while_published: bool,
}

/// Receive-cut fixtures inject all ingress stages in the model. Hardware
/// execution selects the progress cases without those injected cuts.
pub trait Harness {
    /// Storage for captured receive-boundary events.
    type Events: AsRef<[Event]>;
    /// Storage for actual command/transmit terminals.
    type Completions: AsRef<[Completion]>;
    /// Storage for actual quarantined owners.
    type Retained: AsRef<[Owners]>;
    /// The backend's actual LowerMacBackend::GROUP_COMMAND_BOUND.
    const GROUP_COMMAND_BOUND: Duration;
    /// Finite budget including fixture setup and teardown, on `check`'s timer.
    const SCENARIO_BOUND: Duration;
    /// Run a receive-cut case using actual backend events and key owners.
    fn exercise(&mut self, case: Case) -> impl core::future::Future<Output = Trace<Self::Events>>;
    /// Run a command progress/custody case using actual backend owners.
    fn exercise_progress(
        &mut self,
        case: ProgressCase,
    ) -> impl core::future::Future<Output = ProgressTrace<Self::Completions, Self::Retained>>;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// A shared group-key contract violation.
pub enum Violation {
    /// Receive events did not bracket the mutation terminal in the required order.
    RxOrder,
    /// A returned installation or slot differs from the submitted owner.
    Identity,
    /// Selecting an installation changed its protocol-owned PN frontier.
    Counter,
    /// The command performed an incorrect number of material writes.
    Reinstallation,
    /// Old inactive key material still admitted receive traffic.
    InactiveMaterial,
    /// Ingress was disabled before the mutation terminal.
    UnboundedIngress,
    /// The operation violated its declared admission-to-completion bound.
    CommandBound,
    /// The independent fixture deadline expired.
    HarnessTimeout,
    /// Accepted owners were lost, duplicated or returned through the wrong path.
    Custody,
    /// Expiry failed to wake the command's registered waiter.
    DeadlineWake,
    /// Published AP group use and removal were observed in the wrong order.
    Publication,
}

/// Run shared receive-cut and progress cases under an independent fixture timer.
pub async fn check<H: Harness>(
    harness: &mut H,
    timer: &impl Timer,
    environment: Environment,
) -> Result<Report, (Scenario, Violation)> {
    let mut report = Report::default();
    for case in Case::ALL {
        if !report.includes(environment, case.execution()) {
            continue;
        }
        let trace = bounded(harness.exercise(case), timer, H::SCENARIO_BOUND)
            .await
            .map_err(|why| (Scenario::Boundary(case), why))?;
        verify(case, trace, H::GROUP_COMMAND_BOUND)
            .map_err(|why| (Scenario::Boundary(case), why))?;
        report.passed();
    }
    for case in ProgressCase::all() {
        if !report.includes(environment, case.execution()) {
            continue;
        }
        let trace = bounded(harness.exercise_progress(case), timer, H::SCENARIO_BOUND)
            .await
            .map_err(|why| (Scenario::Progress(case), why))?;
        verify_progress(case, trace, H::GROUP_COMMAND_BOUND)
            .map_err(|why| (Scenario::Progress(case), why))?;
        report.passed();
    }
    Ok(report)
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
    let mut operation = core::pin::pin!(operation);
    let mut timeout = core::pin::pin!(timer.wait_until(deadline));
    core::future::poll_fn(|cx| {
        use core::task::Poll;
        if let Poll::Ready(trace) = operation.as_mut().poll(cx) {
            return Poll::Ready(if timer.now() <= deadline {
                Ok(trace)
            } else {
                Err(Violation::HarnessTimeout)
            });
        }
        match timeout.as_mut().poll(cx) {
            Poll::Ready(()) => Poll::Ready(Err(Violation::HarnessTimeout)),
            Poll::Pending => Poll::Pending,
        }
    })
    .await
}

/// Check actual receive-cut observations against the group-key contract.
pub fn verify<E: AsRef<[Event]>>(
    case: Case,
    trace: Trace<E>,
    bound: Duration,
) -> Result<(), Violation> {
    verify_timing(trace.timing, bound)?;
    let [
        Event::OldRx,
        Event::OldRx,
        Event::OldRx,
        Event::OldRx,
        Event::Terminal(returned),
        Event::NewRx,
    ] = trace.events.as_ref()
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

/// Check command bounds, terminal order, deadline wake and owner custody.
pub fn verify_progress<E: AsRef<[Completion]>, R: AsRef<[Owners]>>(
    case: ProgressCase,
    trace: ProgressTrace<E, R>,
    bound: Duration,
) -> Result<(), Violation> {
    verify_timing(trace.timing, bound)?;
    if case != ProgressCase::ReplacementRollback && trace.rollback.is_some() {
        return Err(Violation::Reinstallation);
    }
    match case {
        ProgressCase::ReplacementRollback => {
            if !trace.poisoned
                || !trace.events.as_ref().is_empty()
                || trace.retained.as_ref() != [trace.admitted]
            {
                return Err(Violation::Custody);
            }
            let Some(rollback) = trace.rollback else {
                return Err(Violation::Reinstallation);
            };
            if !rollback.restored
                || rollback.previous_key_id > 3
                || rollback.attempted_key_id > 3
                || rollback.previous_key_id == rollback.attempted_key_id
            {
                return Err(Violation::Reinstallation);
            }
        }
        ProgressCase::Expire(_) => {
            if !trace.poisoned
                || !trace.events.as_ref().is_empty()
                || trace.retained.as_ref() != [trace.admitted]
            {
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
                || !trace.retained.as_ref().is_empty()
                || trace.events.as_ref()
                    != [Completion::Group {
                        mutation,
                        owners: trace.admitted,
                    }]
            {
                return Err(Violation::Custody);
            }
        }
        ProgressCase::PublishedRemoval => {
            if trace.poisoned || !trace.retained.as_ref().is_empty() {
                return Err(Violation::Custody);
            }
            let Some(published) = trace.published else {
                return Err(Violation::Publication);
            };
            if !trace.blocked_while_published
                || trace.events.as_ref()
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
    use alloc::vec::Vec;
    fn at(elapsed: u64) -> oer_time::Instant {
        let clock: oer_time_virtual::VirtualClock = oer_time_virtual::VirtualClock::new();
        clock
            .advance(Duration::from_micros(elapsed))
            .expect("fixture elapsed time fits its virtual clock")
    }

    #[test]
    fn rollback_requires_restored_material_quarantined_owners_and_no_success_terminal() {
        for fault in 0..5 {
            let admitted = Owners {
                slot: 0,
                permission: 0,
            };
            let mut trace = ProgressTrace {
                rollback: Some(Rollback {
                    previous_key_id: 1,
                    attempted_key_id: 2,
                    restored: fault != 1,
                }),
                admitted,
                timing: Timing {
                    admitted_at: at(0),
                    completed_at: at(1),
                },
                events: alloc::vec![],
                retained: alloc::vec![admitted],
                poisoned: fault != 2,
                deadline_woke: false,
                published: None,
                blocked_while_published: false,
            };
            if fault == 3 {
                trace.events.push(Completion::Group {
                    mutation: Mutation::Replace,
                    owners: admitted,
                });
            }
            if fault == 4 {
                trace.retained.clear();
            }
            assert_eq!(
                verify_progress(
                    ProgressCase::ReplacementRollback,
                    trace,
                    Duration::from_micros(10)
                )
                .is_ok(),
                fault == 0
            );
        }
    }

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
                    admitted_at: at(100),
                    completed_at: at(completed_at),
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
            type Events = Vec<Event>;
            type Completions = Vec<Completion>;
            type Retained = Vec<Owners>;
            const GROUP_COMMAND_BOUND: Duration = Duration::from_millis(1);
            const SCENARIO_BOUND: Duration = Duration::from_millis(10);
            async fn exercise(&mut self, _: Case) -> Trace<Self::Events> {
                core::future::pending().await
            }
            async fn exercise_progress(
                &mut self,
                _: ProgressCase,
            ) -> ProgressTrace<Self::Completions, Self::Retained> {
                core::future::pending().await
            }
        }
        let timer = oer_time_virtual::SkipClock::new();
        let mut harness = Never;
        let mut check = core::pin::pin!(check(&mut harness, &timer, Environment::Model));
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
                rollback: None,
                admitted,
                timing: Timing {
                    admitted_at: at(0),
                    completed_at: at(10),
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
                    admitted_at: at(0),
                    completed_at: at(1),
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
