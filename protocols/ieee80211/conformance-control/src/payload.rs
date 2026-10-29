use oer_ieee80211_port::{
    conformance::{self, MpduState, catalog, outcomes},
    *,
};
use postcard_schema::Schema;
use serde::{Deserialize, Serialize};

/// Match an AP without putting its address or SSID in the HIL transcript.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct Prepare {
    pub channel: u8,
    pub country: [u8; 2],
    pub peer_sha256: [u8; 32],
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct Prepared {
    /// The diagnostic composition's optional aggregate admission capability.
    pub ampdu: bool,
    /// Basic-rate elements captured from the matched AP's beacon.
    pub basic_rates: [u8; 16],
    pub basic_rates_len: u8,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct RunCase(pub u8);
impl RunCase {
    pub fn case(self) -> Option<catalog::Case> {
        catalog::Case::all().nth(usize::from(self.0))
    }
}
/// Observable terminal classification, without physical signal measurements.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub enum End {
    Ack,
    NoResponse,
    Collision,
    AirDenied,
    HardwareTimeout,
    Withdrawn,
    Expired,
    CtsTimeout,
    Unsolicited,
    BlockAck,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct OutcomeTrace {
    pub admitted: [u8; 2],
    pub returned: [u8; 2],
    pub sent: bool,
    pub rate_kbps: u32,
    /// Last on-air rate reported by the terminal, independently of admission.
    pub observed_rate_kbps: Option<u32>,
    pub sequence: u16,
    pub publications: u16,
    pub transmissions: u16,
    pub end: End,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct CaseTrace {
    pub case: RunCase,
    pub trace: Trace,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
#[expect(
    clippy::large_enum_variant,
    reason = "bounded no_std traces retain their inline capture storage without allocation"
)]
pub enum Trace {
    Outcome(OutcomeTrace),
    Ownership(OwnershipTrace),
    Barrier(BarrierTrace),
    Configure(ConfigureTrace),
    Group(GroupTrace),
    Edca(EdcaTrace),
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct EdcaTrace {
    pub publications: u16,
    pub tables: u16,
    pub records: u16,
    pub draws: u16,
    pub admitted: [u8; 3],
    pub completed: [u8; 3],
    pub held_draws: Option<u16>,
}
pub fn edca_trace<D: AsRef<[conformance::edca::Draw]>>(
    raw: &conformance::edca::Trace<D>,
) -> Option<EdcaTrace> {
    let draws = u16::try_from(raw.draws.as_ref().len()).ok()?;
    Some(EdcaTrace {
        publications: draws,
        tables: draws.checked_mul(2)?,
        records: draws.checked_mul(8)?,
        draws,
        admitted: raw.admitted,
        completed: raw.completed,
        held_draws: raw.held_draws.map(u16::try_from).transpose().ok()?,
    })
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub enum GroupCompletion {
    Group { mutation: u8, owners: [u8; 2] },
    Transmit { ticket: u8, buffer: u32 },
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct GroupTrace {
    pub rollback: bool,
    pub admitted: [u8; 2],
    pub within_bound: bool,
    pub events: [Option<GroupCompletion>; 4],
    pub poisoned: bool,
    pub deadline_woke: bool,
    pub published: Option<[u32; 2]>,
    pub blocked_while_published: bool,
}
pub fn group_trace<
    E: AsRef<[conformance::group_keys::Completion]>,
    R: AsRef<[conformance::group_keys::Owners]>,
>(
    raw: &conformance::group_keys::ProgressTrace<E, R>,
    bound_micros: u64,
) -> Option<GroupTrace> {
    use conformance::group_keys::{Completion, Mutation};
    let mut events = core::array::from_fn(|_| None);
    for (index, event) in raw.events.as_ref().iter().enumerate() {
        *events.get_mut(index)? = Some(match event {
            Completion::Group { mutation, owners } => GroupCompletion::Group {
                mutation: match mutation {
                    Mutation::Install => 0,
                    Mutation::Replace => 1,
                    Mutation::Select => 2,
                    Mutation::Remove => 3,
                },
                owners: [owners.slot, owners.permission],
            },
            Completion::Transmit(tx) => GroupCompletion::Transmit {
                ticket: tx.ticket,
                buffer: tx.buffer.try_into().ok()?,
            },
        });
    }
    Some(GroupTrace {
        rollback: raw.rollback.is_some(),
        admitted: [raw.admitted.slot, raw.admitted.permission],
        within_bound: bound_micros != 0
            && raw
                .timing
                .completed_at
                .checked_duration_since(raw.timing.admitted_at)
                .is_some_and(|elapsed| elapsed.as_micros() <= bound_micros),
        events,
        poisoned: raw.poisoned,
        deadline_woke: raw.deadline_woke,
        published: match raw.published {
            Some(tx) => Some([u32::from(tx.ticket), tx.buffer.try_into().ok()?]),
            None => None,
        },
        blocked_while_published: raw.blocked_while_published,
    })
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct ConfigureTrace {
    pub admitted: [[u8; 2]; 2],
    pub returned: [Option<[u8; 2]>; 8],
    pub premature_drops: u32,
    pub extra_terminal: bool,
}
/// Normalize opaque allocation coordinates while preserving actual return order.
pub fn configure_trace<R: AsRef<[conformance::interface_commands::Identity]>>(
    trace: &conformance::interface_commands::Trace<R>,
) -> Option<ConfigureTrace> {
    use conformance::interface_commands::Identity;
    let identity = |row: Identity| -> Option<[u8; 2]> {
        Some([
            match row.role {
                VifRole::Station => 0,
                VifRole::AccessPoint => 1,
            },
            trace
                .admitted
                .iter()
                .position(|owner| owner.context == row.context)?
                .try_into()
                .ok()?,
        ])
    };
    let mut returned = [None; 8];
    for (index, row) in trace.returned.as_ref().iter().enumerate() {
        *returned.get_mut(index)? = Some(identity(*row)?);
    }
    Some(ConfigureTrace {
        admitted: [identity(trace.admitted[0])?, identity(trace.admitted[1])?],
        returned,
        premature_drops: trace.premature_drops.try_into().ok()?,
        extra_terminal: trace.extra_terminal,
    })
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct HeldSnapshot {
    pub publications: u16,
    pub events: u16,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub enum BarrierEvent {
    Tx {
        ticket: u8,
        sent: bool,
        end: End,
        withdraw: Option<u8>,
        transmissions: u16,
        rate_kbps: Option<u32>,
    },
    GateClosed,
    KeyRemoved,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct BarrierTrace {
    pub tickets: [u8; 2],
    pub before_request: HeldSnapshot,
    pub before_result: HeldSnapshot,
    pub publications_at_barrier: u16,
    pub events: [Option<BarrierEvent>; 8],
    pub retry_terminal: Option<BarrierEvent>,
}
fn barrier_event<A, D>(event: &conformance::BarrierEvent<A, D>) -> BarrierEvent {
    match event {
        conformance::BarrierEvent::GateClosed => BarrierEvent::GateClosed,
        conformance::BarrierEvent::KeyRemoved => BarrierEvent::KeyRemoved,
        conformance::BarrierEvent::Tx {
            ticket,
            state,
            outcome,
        } => {
            let (end, air, withdraw) = match outcome {
                TxOutcome::Ended(SingleEnd::Delivered { air, response }) => (
                    match response {
                        SingleResponse::Ack(_) => End::Ack,
                        SingleResponse::Unsolicited => End::Unsolicited,
                        SingleResponse::BlockAck { .. } => End::BlockAck,
                    },
                    Some(*air),
                    None,
                ),
                TxOutcome::Ended(SingleEnd::AckTimeout(air)) => (End::NoResponse, Some(*air), None),
                TxOutcome::Ended(SingleEnd::CtsTimeout(air)) => (End::CtsTimeout, *air, None),
                TxOutcome::Ended(SingleEnd::ContentionLimit(air)) => (End::Collision, *air, None),
                TxOutcome::Ended(SingleEnd::AirDenied { air, .. }) => (End::AirDenied, *air, None),
                TxOutcome::Withdrawn(cause) => (
                    End::Withdrawn,
                    None,
                    Some(match cause {
                        WithdrawCause::GateClosed => 0,
                        WithdrawCause::KeyRemoved => 1,
                        WithdrawCause::Disabled => 2,
                    }),
                ),
                TxOutcome::Indeterminate(_) => (End::HardwareTimeout, None, None),
                TxOutcome::Expired { .. } => (End::Expired, None, None),
            };
            BarrierEvent::Tx {
                ticket: *ticket,
                sent: *state == MpduState::Sent,
                end,
                withdraw,
                transmissions: air.map_or(0, |air| air.transmissions.get()),
                rate_kbps: air.and_then(|air| match air.rate {
                    PhyRate::Legacy(rate) => Some(rate.kbps()),
                    _ => None,
                }),
            }
        }
    }
}
pub fn barrier_trace<A, D, E: AsRef<[conformance::BarrierEvent<A, D>]>>(
    raw: &conformance::BarrierTrace<E, conformance::BarrierEvent<A, D>>,
) -> Option<BarrierTrace> {
    fn snapshot(raw: conformance::HeldSnapshot) -> Option<HeldSnapshot> {
        Some(HeldSnapshot {
            publications: raw.publications.try_into().ok()?,
            events: raw.events.try_into().ok()?,
        })
    }
    let mut events = core::array::from_fn(|_| None);
    for (index, event) in raw.events.as_ref().iter().enumerate() {
        *events.get_mut(index)? = Some(barrier_event(event));
    }
    Some(BarrierTrace {
        tickets: raw.tickets,
        before_request: snapshot(raw.before_request)?,
        before_result: snapshot(raw.before_result)?,
        publications_at_barrier: raw.publications_at_barrier.try_into().ok()?,
        events,
        retry_terminal: raw.retry_terminal.as_ref().map(barrier_event),
    })
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct OwnerResources {
    pub ticket: u8,
    pub buffer: u8,
    pub class: u8,
    /// Allocation ordinal, derived from the original owned bodies. None
    /// preserves an unknown/changed body instead of inventing its identity.
    pub body: Option<u8>,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct OwnerReturn {
    pub operation: u8,
    pub category: u8,
    pub class: u8,
    pub expected: OwnerResources,
    pub actual: OwnerResources,
    pub end: u8,
    pub body_pointer_matches: bool,
    pub transmissions: Option<u16>,
    pub outcome: u8,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct OwnershipTrace {
    pub returned: [Option<OwnerReturn>; 8],
    pub contexts_dropped: u16,
    pub before_teardown_dropped: u16,
    pub publications: u16,
}
/// Allocation numbers come from the actual body owners and their frees. Raw
/// pointer equality is recorded separately for each original/returned pair.
pub fn ownership_trace<R, E, P>(trace: conformance::Trace<R, E, P>) -> Option<OwnershipTrace>
where
    R: AsRef<[conformance::Returned]>,
    E: AsRef<[conformance::ReturnEvidence]>,
{
    use conformance::{Class, End, Resources};
    fn class(value: Class) -> u8 {
        match value {
            Class::Small => 0,
            Class::Large => 1,
        }
    }
    let rows = trace.returned.as_ref();
    let evidence = trace.terminal_evidence.as_ref();
    let resource = |value: Resources| OwnerResources {
        ticket: value.ticket,
        buffer: value.buffer,
        class: class(value.class),
        body: u8::try_from(value.body).ok(),
    };
    if rows.len() != evidence.len() {
        return None;
    }
    let mut returned = [None; 8];
    for (index, row) in rows.iter().enumerate() {
        *returned.get_mut(index)? = Some(OwnerReturn {
            operation: row.operation.id,
            category: row.operation.category as u8,
            class: class(row.operation.class),
            body_pointer_matches: evidence.get(index)?.pointer_matches(),
            transmissions: evidence.get(index)?.transmissions(),
            outcome: evidence.get(index)?.outcome(),
            expected: resource(row.expected),
            actual: resource(row.actual),
            end: match row.end {
                End::Ended => 0,
                End::Withdrawn => 1,
                End::Refused => 2,
            },
        });
    }
    Some(OwnershipTrace {
        returned,
        contexts_dropped: trace.contexts_dropped.try_into().ok()?,
        before_teardown_dropped: trace.before_teardown_dropped.try_into().ok()?,
        publications: trace.publications.try_into().ok()?,
    })
}
/// Project actual resource and terminal observations onto the port contract.
/// Signal measurements and clock coordinates are physical evidence, not
/// model-comparable values. Every contract field of the outcome is retained.
pub fn outcome_trace<A, B, D>(trace: &outcomes::Trace<A, B, D>) -> Option<OutcomeTrace> {
    let outcomes::Outcome::Single(outcome) = &trace.outcome else {
        return None;
    };
    let (end, air) = match outcome {
        TxOutcome::Ended(SingleEnd::Delivered {
            air,
            response: SingleResponse::Ack(_),
        }) => (End::Ack, Some(*air)),
        TxOutcome::Ended(SingleEnd::Delivered { .. }) => return None,
        TxOutcome::Ended(SingleEnd::AckTimeout(air)) => (End::NoResponse, Some(*air)),
        TxOutcome::Ended(SingleEnd::ContentionLimit(air)) => (End::Collision, *air),
        TxOutcome::Ended(SingleEnd::AirDenied { air, .. }) => (End::AirDenied, *air),
        TxOutcome::Indeterminate(_) => (End::HardwareTimeout, None),
        TxOutcome::Withdrawn(_) => (End::Withdrawn, None),
        TxOutcome::Expired { .. } => (End::Expired, None),
        _ => return None,
    };
    let PhyRate::Legacy(rate) = trace.rate else {
        return None;
    };
    Some(OutcomeTrace {
        admitted: [trace.admitted.ticket, trace.admitted.buffer],
        returned: [trace.returned.ticket, trace.returned.buffer],
        sent: trace.state == MpduState::Sent,
        rate_kbps: rate.kbps(),
        observed_rate_kbps: air.and_then(|air| match air.rate {
            PhyRate::Legacy(rate) => Some(rate.kbps()),
            _ => None,
        }),
        sequence: trace.sequence.get(),
        publications: u16::try_from(trace.publications).ok()?,
        transmissions: air.map_or(0, |air| air.transmissions.get()),
        end,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::num::NonZeroU16;
    use oer_ieee80211_port::{Air, PhyRate};
    use oer_ieee80211_wire::{phy::LegacyRate, sequence::SequenceNumber};

    #[test]
    fn projection_does_not_hide_a_different_terminal_rate_or_owner_state() {
        let expected = outcome_trace(&trace(LegacyRate::Ofdm6M, MpduState::Sent)).unwrap();
        let changed = outcome_trace(&trace(LegacyRate::Ofdm12M, MpduState::Sent)).unwrap();
        assert_eq!(expected.rate_kbps, changed.rate_kbps);
        assert_ne!(expected, changed);
        let fresh = outcome_trace(&trace(LegacyRate::Ofdm6M, MpduState::Fresh)).unwrap();
        assert!(!fresh.sent);
        assert_ne!(expected, fresh);
    }

    #[test]
    fn barrier_projection_compares_retry_outcomes_even_with_equal_publications() {
        let air = Air {
            transmissions: NonZeroU16::new(2).unwrap(),
            rate: PhyRate::Legacy(LegacyRate::Ofdm6M),
        };
        let mut raw = conformance::BarrierTrace {
            tickets: [0, 1],
            before_request: conformance::HeldSnapshot {
                publications: 1,
                events: 0,
            },
            before_result: conformance::HeldSnapshot {
                publications: 1,
                events: 0,
            },
            just_before_timeout: None,
            publications_at_barrier: 1,
            events: [conformance::BarrierEvent::<(), core::convert::Infallible>::GateClosed],
            retry_terminal: Some(conformance::BarrierEvent::Tx {
                ticket: 0,
                state: MpduState::Sent,
                outcome: TxOutcome::Ended(SingleEnd::AckTimeout(air)),
            }),
        };
        let expected = barrier_trace(&raw).unwrap();
        assert_eq!(
            expected.retry_terminal,
            Some(BarrierEvent::Tx {
                ticket: 0,
                sent: true,
                end: End::NoResponse,
                withdraw: None,
                transmissions: 2,
                rate_kbps: Some(6000),
            })
        );
        if let Some(conformance::BarrierEvent::Tx { outcome, .. }) = &mut raw.retry_terminal {
            *outcome = TxOutcome::Ended(SingleEnd::Delivered {
                air,
                response: SingleResponse::Ack(()),
            });
        }
        let ack = barrier_trace(&raw).unwrap();
        assert_ne!(
            expected, ack,
            "an ACK cannot masquerade as exhausted retries"
        );
        raw.retry_terminal = None;
        assert_ne!(
            expected,
            barrier_trace(&raw).unwrap(),
            "a discarded terminal is visible"
        );
    }

    #[test]
    fn ownership_projection_preserves_lifetime_numbers_and_pointer_mismatch() {
        use conformance::{End, OPERATIONS, Resources, ReturnEvidence, Returned};
        let resource = Resources {
            ticket: 0,
            buffer: 0,
            class: OPERATIONS[0].class,
            body: 0,
        };
        let project = |pointer, body| {
            ownership_trace(conformance::Trace {
                returned: [Returned {
                    operation: OPERATIONS[0],
                    expected: resource,
                    actual: Resources { body, ..resource },
                    end: End::Refused,
                }],
                contexts_dropped: 1,
                before_teardown_dropped: 1,
                publications: 0,
                terminal_evidence: [ReturnEvidence::refused(pointer)],
                publication_events: core::iter::empty::<
                    oer_ieee80211_port::conformance::PublicationEvent,
                >(),
            })
            .unwrap()
        };
        let original = project(true, 0);
        assert_eq!(original.returned[0].unwrap().actual.body, Some(0));
        assert_ne!(original, project(false, 0));
        assert_ne!(original, project(true, 1));
    }
    #[test]
    fn ownership_projection_keeps_publication_count_without_a_journal() {
        use conformance::{End, OPERATIONS, Resources, ReturnEvidence, Returned};
        let resource = Resources {
            ticket: 2,
            buffer: 0,
            class: OPERATIONS[2].class,
            body: 0,
        };
        let terminal: SingleOutcome<(), ()> = TxOutcome::Ended(SingleEnd::AckTimeout(Air {
            transmissions: NonZeroU16::MIN,
            rate: PhyRate::Legacy(LegacyRate::Ofdm6M),
        }));
        let trace = ownership_trace(conformance::Trace {
            returned: [Returned {
                operation: OPERATIONS[2],
                expected: resource,
                actual: resource,
                end: End::Ended,
            }],
            contexts_dropped: 1,
            before_teardown_dropped: 1,
            publications: 3,
            terminal_evidence: [ReturnEvidence::terminal(true, &terminal).unwrap()],
            publication_events: core::iter::empty::<
                oer_ieee80211_port::conformance::PublicationEvent,
            >(),
        })
        .unwrap();
        assert_eq!(trace.publications, 3);
        assert_eq!(trace.returned[0].unwrap().transmissions, Some(1));
    }
}
