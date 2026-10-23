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
pub enum Trace {
    Outcome(OutcomeTrace),
    Ownership(OwnershipTrace),
    Barrier(BarrierTrace),
    Configure(ConfigureTrace),
    Group(GroupTrace),
    Edca(EdcaTrace),
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct EdcaParameters {
    pub acm: bool,
    pub aifsn: u8,
    pub minimum: u8,
    pub maximum: u8,
    pub txop: u16,
    pub lifetime_us: u32,
}
pub fn edca_parameters(record: EdcaAcParameters) -> Option<EdcaParameters> {
    Some(EdcaParameters {
        acm: record.wmm.admission_control_mandatory,
        aifsn: record.wmm.aifsn,
        minimum: record.wmm.ecw_min,
        maximum: record.wmm.ecw_max,
        txop: record.wmm.txop_limit_units_32_us,
        lifetime_us: record.msdu_lifetime.as_micros().try_into().ok()?,
    })
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct EdcaHeader {
    pub count: u8,
    pub uapsd: bool,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct EdcaDraw {
    pub role: u8,
    pub category: u8,
    pub parameters: Option<EdcaParameters>,
    pub window: u16,
    pub backoff: u16,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct EdcaTrace {
    pub draws: [Option<EdcaDraw>; 4],
    pub installed: [[Option<EdcaParameters>; 4]; 2],
    pub headers: [Option<EdcaHeader>; 2],
    pub admitted: [u8; 3],
    pub completed: [u8; 3],
    pub held_draws: Option<u16>,
}
pub fn edca_trace<D: AsRef<[conformance::edca::Draw]>>(
    raw: &conformance::edca::Trace<D>,
) -> Option<EdcaTrace> {
    use oer_ieee80211_wire::qos::WmmAccessCategory;
    let mut trace = EdcaTrace {
        draws: [None; 4],
        installed: [[None; 4]; 2],
        headers: [None; 2],
        admitted: raw.admitted,
        completed: raw.completed,
        held_draws: raw.held_draws.map(u16::try_from).transpose().ok()?,
    };
    for (index, draw) in raw.draws.as_ref().iter().enumerate() {
        *trace.draws.get_mut(index)? = Some(EdcaDraw {
            role: draw.role as u8,
            category: draw.category as u8,
            parameters: Some(edca_parameters(draw.parameters)?),
            window: draw.window_slots,
            backoff: draw.backoff_slots,
        });
    }
    for (index, set) in raw.installed.iter().enumerate() {
        let wmm = set.wmm();
        *trace.headers.get_mut(index)? = Some(EdcaHeader {
            count: wmm.parameter_set_count,
            uapsd: wmm.uapsd,
        });
        for category in WmmAccessCategory::ALL {
            *trace.installed.get_mut(index)?.get_mut(category as usize)? =
                Some(edca_parameters(set.access_category(category))?);
        }
    }
    Some(trace)
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub enum GroupCompletion {
    Group { mutation: u8, owners: [u8; 2] },
    Transmit { ticket: u8, buffer: u32 },
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct GroupTrace {
    pub rollback: Option<GroupRollback>,
    pub admitted: [u8; 2],
    pub within_bound: bool,
    pub events: [Option<GroupCompletion>; 4],
    pub retained: [Option<[u8; 2]>; 4],
    pub poisoned: bool,
    pub deadline_woke: bool,
    pub published: Option<[u32; 2]>,
    pub blocked_while_published: bool,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct GroupRollback {
    pub previous_key_id: u8,
    pub attempted_key_id: u8,
    pub restored: bool,
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
    let mut retained = [None; 4];
    for (index, owners) in raw.retained.as_ref().iter().enumerate() {
        *retained.get_mut(index)? = Some([owners.slot, owners.permission]);
    }
    Some(GroupTrace {
        rollback: raw.rollback.map(|rollback| GroupRollback {
            previous_key_id: rollback.previous_key_id,
            attempted_key_id: rollback.attempted_key_id,
            restored: rollback.restored,
        }),
        admitted: [raw.admitted.slot, raw.admitted.permission],
        within_bound: bound_micros != 0
            && raw
                .timing
                .completed_at
                .checked_duration_since(raw.timing.admitted_at)
                .is_some_and(|elapsed| elapsed.as_micros() <= bound_micros),
        events,
        retained,
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
pub struct Frame {
    pub len: u8,
    pub bytes: [[u8; 32]; 2],
}
fn frame(bytes: &[u8]) -> Option<Frame> {
    if bytes.len() > 64 {
        return None;
    }
    let mut out = Frame {
        len: bytes.len().try_into().ok()?,
        bytes: [[0; 32]; 2],
    };
    for (index, (out, byte)) in out.bytes.iter_mut().flatten().zip(bytes).enumerate() {
        // The three physical MAC addresses are fixture coordinates. All other
        // encoded octets, including FC, Duration, SN, PN and payload, survive.
        *out = if (4..22).contains(&index) { 0 } else { *byte };
    }
    Some(out)
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct BarrierTrace {
    pub tickets: [u8; 2],
    pub before_request: HeldSnapshot,
    pub before_result: HeldSnapshot,
    pub publications_at_barrier: u16,
    pub events: [Option<BarrierEvent>; 8],
    pub first_frame: Frame,
    pub retried_frames: [Option<Frame>; 4],
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
pub fn barrier_trace<
    A,
    D,
    E: AsRef<[conformance::BarrierEvent<A, D>]>,
    F: AsRef<[u8]>,
    R: AsRef<[F]>,
>(
    raw: &conformance::BarrierTrace<E, F, R, conformance::BarrierEvent<A, D>>,
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
    let mut retries = core::array::from_fn(|_| None);
    for (index, bytes) in raw.retried_frames.as_ref().iter().enumerate() {
        *retries.get_mut(index)? = Some(frame(bytes.as_ref())?);
    }
    Some(BarrierTrace {
        tickets: raw.tickets,
        before_request: snapshot(raw.before_request)?,
        before_result: snapshot(raw.before_result)?,
        publications_at_barrier: raw.publications_at_barrier.try_into().ok()?,
        events,
        first_frame: frame(raw.first_frame.as_ref())?,
        retried_frames: retries,
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
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub enum OwnerPublication {
    Tx(u8),
    GateClosed,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct OwnershipTrace {
    pub returned: [Option<OwnerReturn>; 8],
    pub contexts_dropped: u16,
    pub before_teardown_dropped: u16,
    pub publications: u16,
    pub publication_events: [Option<OwnerPublication>; 8],
}
/// Allocation numbers come from the actual body owners and their frees. Raw
/// pointer equality is recorded separately for each original/returned pair.
pub fn ownership_trace<R, E, P>(trace: conformance::Trace<R, E, P>) -> Option<OwnershipTrace>
where
    R: AsRef<[conformance::Returned]>,
    E: AsRef<[conformance::ReturnEvidence]>,
    P: IntoIterator<Item = conformance::PublicationEvent>,
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
    let mut publication_events = [None; 8];
    let mut count = 0_usize;
    for (index, event) in trace.publication_events.into_iter().enumerate() {
        *publication_events.get_mut(index)? = Some(match event {
            conformance::PublicationEvent::Tx(ticket) => {
                count = count.checked_add(1)?;
                OwnerPublication::Tx(ticket)
            }
            conformance::PublicationEvent::GateClosed => OwnerPublication::GateClosed,
        });
    }
    if count != trace.publications {
        return None;
    }
    Some(OwnershipTrace {
        publication_events,
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

    fn trace(observed: LegacyRate, state: MpduState) -> outcomes::Trace<(), (), ()> {
        outcomes::Trace {
            admitted: outcomes::Identity {
                ticket: 0,
                buffer: 1,
            },
            returned: outcomes::Identity {
                ticket: 0,
                buffer: 1,
            },
            state,
            rate: PhyRate::Legacy(LegacyRate::Ofdm6M),
            sequence: SequenceNumber::from_sequence_control(0x1230),
            publications: 1,
            outcome: outcomes::Outcome::Single(TxOutcome::Ended(SingleEnd::AckTimeout(Air {
                transmissions: NonZeroU16::MIN,
                rate: PhyRate::Legacy(observed),
            }))),
        }
    }
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
            first_frame: [0; 40],
            retried_frames: [[0; 40]; 2],
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
        assert_eq!(expected.retried_frames, ack.retried_frames);
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
                publication_events: [],
            })
            .unwrap()
        };
        let original = project(true, 0);
        assert_eq!(original.returned[0].unwrap().actual.body, Some(0));
        assert_ne!(original, project(false, 0));
        assert_ne!(original, project(true, 1));
    }
    #[test]
    fn ownership_projection_preserves_publications_gate_and_terminal_count() {
        use PublicationEvent::{GateClosed, Tx};
        use conformance::{End, OPERATIONS, PublicationEvent, Resources, ReturnEvidence, Returned};
        let resource = Resources {
            ticket: 2,
            buffer: 0,
            class: OPERATIONS[2].class,
            body: 0,
        };
        let project = |count, events: &[PublicationEvent]| {
            let terminal: SingleOutcome<(), ()> = TxOutcome::Ended(SingleEnd::AckTimeout(Air {
                transmissions: NonZeroU16::new(count).unwrap(),
                rate: PhyRate::Legacy(LegacyRate::Ofdm6M),
            }));
            ownership_trace(conformance::Trace {
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
                publication_events: events.iter().copied(),
            })
        };
        let first = project(1, &[Tx(2), Tx(3), Tx(2), GateClosed]).unwrap();
        assert_eq!(
            first.publication_events,
            [
                Some(OwnerPublication::Tx(2)),
                Some(OwnerPublication::Tx(3)),
                Some(OwnerPublication::Tx(2)),
                Some(OwnerPublication::GateClosed),
                None,
                None,
                None,
                None
            ]
        );
        assert_eq!(first.returned[0].unwrap().transmissions, Some(1));
        assert_ne!(
            first,
            project(2, &[Tx(2), Tx(3), Tx(2), GateClosed]).unwrap()
        );
        assert_ne!(
            first,
            project(1, &[Tx(2), Tx(3), Tx(0), GateClosed]).unwrap()
        );
        assert_ne!(
            first,
            project(1, &[Tx(2), Tx(3), GateClosed, Tx(2)]).unwrap()
        );
        assert!(project(1, &[Tx(2), Tx(3), GateClosed]).is_none());
    }
}
