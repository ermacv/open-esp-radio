//! Shared acceptance of GTK RX boundaries. A backend fixture captures actual
//! installed identities, counters, hardware writes and delivered events.
//! Delayed DMA/ISR/software RX is injected before each accepted mutation;
//! continuous ingress must remain enabled until its terminal is delivered.
//! Measurements use the backend's admission and completion instants.

use super::{Execution, ModelOnlyReason};
use oer_portable_time::Duration;

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
    pub admitted_at: oer_portable_time::Instant,
    /// Monotonic instant recorded at command completion.
    pub completed_at: oer_portable_time::Instant,
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
    /// Every group-key mutation.
    pub const ALL: [Self; 4] = [Self::Install, Self::Replace, Self::Select, Self::Remove];
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

#[cfg(test)]
mod tests {
    use super::*;
    fn at(elapsed: u64) -> oer_portable_time::Instant {
        let clock: oer_portable_time_virtual::VirtualClock =
            oer_portable_time_virtual::VirtualClock::new();
        clock
            .advance(Duration::from_micros(elapsed))
            .expect("fixture elapsed time fits its virtual clock")
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
