use super::*;
use oer_protocols_ieee80211_contracts_port::{LayoutError, PoolSource};
// The fixture classifies port refusals; no station transition is involved.
#[derive(Debug, Eq, PartialEq)]
enum TxRefusal {
    Layout(LayoutError),
    Preparation(PreparationError),
}

fn frame<'p, 'id: 'p, Id: SlotIdentity + 'id>(
    phase: &ModelPhase<'p, 'id, ModelCaps, ModelBackend<Id>>,
    len: usize,
    protected_bit: bool,
) -> ClassFrame<'p, 'id, ModelSmall, ModelCaps> {
    let TxFrame::Individual { frame, plan } = data_frame(phase, protected_bit) else {
        unreachable!()
    };
    ClassFrame::new(TxFrame::Individual {
        frame: SizedFrame::new(*frame.header(), MpduLength::new(len).unwrap()).unwrap(),
        plan,
    })
    .ok()
    .unwrap()
}

type RefusedSingle<'id, Id> = (
    TxRefusal,
    Slot<'id, ModelTickets, ModelPool<Id, 1>>,
    ModelAnyBuffer<'id, ModelBackend<Id>>,
    Option<ModelBody>,
);
#[expect(
    clippy::result_large_err,
    reason = "a refusal returns the whole operation by value"
)]
fn try_source<'k, 'p, 'id: 'p + 'k, Id: SlotIdentity + 'id>(
    phase: &mut ModelPhase<'p, 'id, ModelCaps, ModelBackend<Id>>,
    ticket: Slot<'id, ModelTickets, ModelPool<Id, 1>>,
    source: PoolSource<'k, 'p, 'id, ModelTypes<ModelBackend<Id>>, ModelSmall, ModelCaps, Station>,
) -> Result<(), RefusedSingle<'id, Id>> {
    let checked = match source.check() {
        Ok(source) => <ModelTypes<ModelBackend<Id>> as TxBufferPool<ModelSmall>>::prepare::<
            Station,
            ModelCaps,
        >(source)
        .map_err(|refused| (TxRefusal::Preparation(refused.reason), refused.operation)),
        Err(refused) => Err((TxRefusal::Layout(refused.reason), refused.operation)),
    };
    let source = match checked {
        Ok(source) => source,
        Err((reason, operation)) => {
            let (buffer, body) = match operation {
                TxSource::Fresh { buffer, body, .. } => (ModelAnyBuffer::Small(buffer), body),
                TxSource::Retransmission { mpdu, .. } => (ModelAnyBuffer::SmallSent(mpdu), None),
            };
            return Err((reason, ticket, buffer, body));
        }
    };
    phase
        .station()
        .submit(
            (oer_protocols_ieee80211_contracts_port::Attempt {
                context: (),
                ticket,
                source,
                access_category: WmmAccessCategory::BestEffort,
                protection: Protection::None,
                power: SupportedPower::new(TxPower::Calibrated).unwrap(),
                coex_operation: oer_protocols_ieee80211_contracts_port::TxOperation::Data,
            })
            .admit(),
        )
        .expect("preparation does not poison the model");
    Ok(())
}

struct Boundaries;
impl Work for Boundaries {
    async fn work<'p, 'id: 'p, Id: SlotIdentity + 'id>(
        self,
        phase: &mut ModelPhase<'p, 'id, ModelCaps, ModelBackend<Id>>,
        resources: ModelResources<'id, ModelBackend<Id>>,
    ) {
        let station_association = ModelKeys::<ModelBackend<Id>, Station>::begin_association(
            resources.station_association,
            oer_protocols_ieee80211_wire::security::Spp::NotNegotiated,
        );

        let mut terminal_access = resources.terminal_access;
        let ModelResources {
            mut tickets,
            mut small,
            station_pairwise: [slot, _],
            ..
        } = resources;
        let key = installed_station_pairwise(
            phase,
            &station_association,
            slot,
            IndividualAddress::new(PEER).unwrap(),
            Ccmp128Key::new(&[7; 16]),
        )
        .await
        .expect("the PTK installation terminal")
        .into_core_key();
        let mut ticket = tickets.remove(0);
        let mut buffer = small.remove(0);
        for (len, body_len, protected_bit, keyed, refusal) in [
            (
                40,
                16,
                true,
                true,
                Some(TxRefusal::Layout(LayoutError::BodyOverlap)),
            ),
            (
                31,
                0,
                true,
                true,
                Some(TxRefusal::Layout(LayoutError::CcmpHeaderTooShort)),
            ),
            (
                40,
                17,
                false,
                false,
                Some(TxRefusal::Layout(LayoutError::BodyOverlap)),
            ),
            // The bit is now derived solely from TxProtection in preparation.
            (40, 0, true, false, None),
            (40, 0, false, true, None),
            (32, 0, true, true, None),
            (40, 8, true, true, None),
            (40, 16, false, false, None),
        ] {
            buffer.memory.fill(0xa5);
            let pointer = buffer.memory.as_ptr();
            let slot = buffer.slot.index();
            let ticket_index = ticket.index();
            let body = ModelBody(alloc::vec![0x5a; body_len]);
            let body_pointer = body.0.as_ptr();
            let before = phase.held.commands.items.inspect().len();
            let pn_before = key.last();
            let frame = frame(phase, len, protected_bit);
            let result = try_source(
                phase,
                ticket,
                TxSource::Fresh {
                    buffer,
                    frame,
                    body: Some(body),
                    protection: if keyed {
                        TxProtection::Ccmp(key.numbered())
                    } else {
                        TxProtection::Open
                    },
                },
            );
            if let Some(reason) = refusal {
                let Err((returned_reason, returned_ticket, returned, body)) = result else {
                    panic!("typed refusal")
                };
                assert_eq!(returned_reason, reason);
                assert_eq!(
                    key.last(),
                    pn_before + u64::from(keyed),
                    "layout refusal never allocates beyond the core-supplied PN"
                );
                assert_eq!(phase.held.commands.items.inspect().len(), before);
                assert!(phase.held.events.is_empty(), "no synthetic terminal");
                ticket = returned_ticket;
                let ModelAnyBuffer::Small(returned) = returned else {
                    unreachable!()
                };
                buffer = returned;
                assert!(buffer.memory.iter().all(|byte| *byte == 0xa5));
                assert_eq!(body.unwrap().0.as_ptr(), body_pointer);
            } else {
                assert!(result.is_ok());
                let (returned_ticket, returned_buffer) =
                    completion(phase, &mut terminal_access).await;
                ticket = returned_ticket;
                let ModelAnyBuffer::SmallSent(mpdu) = returned_buffer else {
                    unreachable!()
                };
                assert_eq!(mpdu.body().unwrap().0.as_ptr(), body_pointer);
                assert_eq!(
                    &mpdu.buffer.memory[len - body_len..len],
                    &[0x5a; 16][..body_len]
                );
                assert_eq!(mpdu.packet_number(), keyed.then_some(pn_before + 1));
                assert_eq!(mpdu.buffer.memory[1] & 0x40 != 0, keyed);
                let (returned, body) =
                    <ModelTypes<ModelBackend<Id>> as TxBufferPool<ModelSmall>>::renew(mpdu);
                assert_eq!(body.unwrap().0.as_ptr(), body_pointer);
                buffer = returned;
            }
            assert_eq!(ticket.index(), ticket_index);
            assert_eq!(buffer.slot.index(), slot);
            assert_eq!(buffer.memory.as_ptr(), pointer);
        }
    }
}
#[test]
fn body_and_ccmp_boundaries_are_checked_before_admission_or_packet_number_allocation() {
    let (backend, _) = backend();
    let _port_end = block_on(run_model(backend, Body(Boundaries)));
}

/// A legal body provider can select a different immutable slice on a later
/// call. The layout must retain the view it sized until that view is copied.
struct ChangingBody {
    reads: Rc<Cell<usize>>,
    drops: Rc<Cell<usize>>,
    later_length: usize,
}
impl TxBody for ChangingBody {
    fn bytes(&self) -> &[u8] {
        let reads = self.reads.get();
        self.reads.set(reads + 1);
        if reads == 0 {
            &[0x5a; 8]
        } else {
            &[0x3c; 16][..self.later_length]
        }
    }
}
impl Drop for ChangingBody {
    fn drop(&mut self) {
        self.drops.set(self.drops.get() + 1);
    }
}

struct BodyView;
impl Work for BodyView {
    async fn work<'p, 'id: 'p, Id: SlotIdentity + 'id>(
        self,
        phase: &mut ModelPhase<'p, 'id, ModelCaps, ModelBackend<Id>>,
        resources: ModelResources<'id, ModelBackend<Id>>,
    ) {
        let ModelResources { mut small, .. } = resources;
        let mut buffer = small.remove(0);
        let slot = buffer.slot.index();
        let memory = buffer.memory.as_ptr();
        for later_length in [0, 4, 8, 16] {
            buffer.memory.fill(0xa5);
            let reads = Rc::new(Cell::new(0));
            let drops = Rc::new(Cell::new(0));
            let body = Some(ChangingBody {
                reads: reads.clone(),
                drops: drops.clone(),
                later_length,
            });
            let (returned, body, layout) = {
                let source: TxSource<_, (), _, _, (), _> = TxSource::Fresh {
                    buffer,
                    frame: frame(phase, 40, false),
                    body,
                    protection: TxProtection::Open,
                };
                let checked = source.check().ok().unwrap();
                let oer_protocols_ieee80211_contracts_port::CheckedTxSource::Fresh(source) =
                    checked
                else {
                    panic!("fresh source stays fresh before backend preparation");
                };
                let (source, layout) = source.into_parts();
                (source.buffer, source.body, layout)
            };
            buffer = returned;
            assert_eq!(layout.body_start(), 32);
            assert_eq!(&buffer.memory[32..40], &[0x5a; 8]);
            assert_eq!(reads.get(), 1, "size and copy use the same borrowed view");
            assert_eq!(buffer.slot.index(), slot);
            assert_eq!(buffer.memory.as_ptr(), memory);
            assert!(buffer.memory[40..].iter().all(|byte| *byte == 0xa5));
            assert_eq!(drops.get(), 0, "checking retains the body's owner");
            drop(body);
            assert_eq!(drops.get(), 1, "cancellation returns the sole body owner");
            assert!(phase.held.events.is_empty(), "no TX was submitted");
        }
    }
}

#[test]
fn a_single_source_copies_exactly_the_body_view_whose_length_it_checked() {
    let (backend, _) = backend();
    let _port_end = block_on(run_model(backend, Body(BodyView)));
}

struct RetryIdentity;
impl Work for RetryIdentity {
    async fn work<'p, 'id: 'p, Id: SlotIdentity + 'id>(
        self,
        phase: &mut ModelPhase<'p, 'id, ModelCaps, ModelBackend<Id>>,
        resources: ModelResources<'id, ModelBackend<Id>>,
    ) {
        let station_association = ModelKeys::<ModelBackend<Id>, Station>::begin_association(
            resources.station_association,
            oer_protocols_ieee80211_wire::security::Spp::NotNegotiated,
        );

        let mut terminal_access = resources.terminal_access;
        let ModelResources {
            mut tickets,
            mut small,
            station_pairwise: [slot, _],
            ..
        } = resources;
        let key = installed_station_pairwise(
            phase,
            &station_association,
            slot,
            IndividualAddress::new(PEER).unwrap(),
            Ccmp128Key::new(&[7; 16]),
        )
        .await
        .expect("the PTK installation terminal")
        .into_core_key();
        let body = ModelBody(alloc::vec![0x66; 8]);
        let body_pointer = body.0.as_ptr();
        let source = TxSource::Fresh {
            buffer: small.remove(0),
            frame: frame(phase, 40, true),
            body: Some(body),
            protection: TxProtection::Ccmp(key.numbered()),
        };
        submit_frame::<_, ModelSmall>(phase, tickets.remove(0), source);
        let (ticket, buffer) = completion(phase, &mut terminal_access).await;
        let ModelAnyBuffer::SmallSent(mpdu) = buffer else {
            unreachable!()
        };
        let before = mpdu.buffer.memory[..40].to_vec();
        let pn = mpdu.packet_number();
        let plan = RetryPlan::new(RetryStage {
            rate: phase.rate(PhyRate::Legacy(LegacyRate::Ofdm24M)).unwrap(),
            transmissions: NonZeroU8::MIN,
            protection: Protection::None,
        });
        submit_frame::<_, ModelSmall>(
            phase,
            ticket,
            TxSource::Retransmission {
                mpdu,
                plan: RetransmissionPlan::Individual(plan),
            },
        );
        let (ticket, buffer) = completion(phase, &mut terminal_access).await;
        let ModelAnyBuffer::SmallSent(mpdu) = buffer else {
            unreachable!()
        };
        let mut expected = before;
        expected[1] |= 0x08;
        assert_eq!(&mpdu.buffer.memory[..40], expected.as_slice());
        assert_eq!(mpdu.packet_number(), pn);
        assert_eq!(mpdu.body().unwrap().0.as_ptr(), body_pointer);
        let index = key.identity.0.index;
        phase
            .station()
            .remove_pairwise_key(station_association, key.into_backend());
        let (slot, station_association) = loop {
            if let Ok(Ok(LowerMacEvent::Terminal(LowerMacTerminal::KeyRemoved(
                Removed::Station(RemovedKey::Pairwise { slot, association }),
            )))) = phase.next_event().await
            {
                break (slot, association);
            }
        };
        let replacement = installed_station_pairwise(
            phase,
            &station_association,
            slot,
            IndividualAddress::new(PEER).unwrap(),
            Ccmp128Key::new(&[7; 16]),
        )
        .await
        .expect("the PTK installation terminal")
        .into_core_key();
        assert_eq!(replacement.identity.0.index, index);
        let TxFrame::Individual { plan, .. } = data_frame(phase, true) else {
            unreachable!()
        };
        let Err((reason, ticket, returned, body)) = try_source(
            phase,
            ticket,
            TxSource::Retransmission {
                mpdu,
                plan: RetransmissionPlan::Individual(plan),
            },
        ) else {
            panic!("retired key refuses preparation")
        };
        assert_eq!(
            reason,
            TxRefusal::Preparation(PreparationError::RetransmissionKey)
        );
        assert!(body.is_none());
        let ModelAnyBuffer::SmallSent(mpdu) = returned else {
            unreachable!()
        };
        assert_eq!(mpdu.packet_number(), pn);
        assert_eq!(mpdu.body().unwrap().0.as_ptr(), body_pointer);
        assert_eq!(replacement.last(), 0);
        assert!(phase.held.events.is_empty());
        let _ticket = ticket;
        assert!(phase.held.events.is_empty());
    }
}
#[test]
fn retry_owns_its_mpdu_and_body_allows_a_new_rate_and_refuses_a_reinstalled_key() {
    let (backend, _) = backend();
    let _port_end = block_on(run_model(backend, Body(RetryIdentity)));
}

struct PreparedRemoval {
    cancel: bool,
}
impl Work for PreparedRemoval {
    async fn work<'p, 'id: 'p, Id: SlotIdentity + 'id>(
        self,
        phase: &mut ModelPhase<'p, 'id, ModelCaps, ModelBackend<Id>>,
        resources: ModelResources<'id, ModelBackend<Id>>,
    ) {
        let station_association = ModelKeys::<ModelBackend<Id>, Station>::begin_association(
            resources.station_association,
            oer_protocols_ieee80211_wire::security::Spp::NotNegotiated,
        );

        let mut terminal_access = resources.terminal_access;
        let ModelResources {
            mut tickets,
            mut small,
            station_pairwise: [slot, _],
            ..
        } = resources;
        let key = installed_station_pairwise(
            phase,
            &station_association,
            slot,
            IndividualAddress::new(PEER).unwrap(),
            Ccmp128Key::new(&[7; 16]),
        )
        .await
        .expect("the PTK installation terminal")
        .into_core_key();
        submit(phase, tickets.remove(0), small.remove(0), Some(&key));
        let (ticket, buffer) = completion(phase, &mut terminal_access).await;
        let ModelAnyBuffer::SmallSent(mpdu) = buffer else {
            unreachable!()
        };
        let pn = mpdu.packet_number();
        let pointer = mpdu.buffer.memory.as_ptr();
        let TxFrame::Individual { plan, .. } = data_frame(phase, true) else {
            unreachable!()
        };
        let prepared = <ModelTypes<ModelBackend<Id>> as TxBufferPool<ModelSmall>>::prepare::<
            Station,
            ModelCaps,
        >(
            (TxSource::Retransmission {
                mpdu,
                plan: RetransmissionPlan::Individual(plan),
            })
            .check()
            .ok()
            .unwrap(),
        )
        .ok()
        .unwrap();
        let air = phase.held.air.clone();
        let sent_before = air.sent().len();
        phase
            .station()
            .remove_pairwise_key(station_association, key.into_backend());
        yield_now().await;
        assert!(
            phase.held.events.is_empty(),
            "removal waits for the externally held preparation"
        );
        if self.cancel {
            let (buffer, body) = prepared.into_parts();
            assert!(body.is_none());
            let ModelAnyBuffer::SmallSent(mpdu) = buffer else {
                unreachable!()
            };
            assert_eq!(mpdu.packet_number(), pn);
            assert_eq!(mpdu.buffer.memory.as_ptr(), pointer);
        } else {
            phase
                .station()
                .submit(
                    (oer_protocols_ieee80211_contracts_port::Attempt {
                        context: (),
                        ticket,
                        source: prepared,
                        access_category: WmmAccessCategory::BestEffort,
                        protection: Protection::None,
                        power: SupportedPower::new(TxPower::Calibrated).unwrap(),
                        coex_operation: oer_protocols_ieee80211_contracts_port::TxOperation::Data,
                    })
                    .admit(),
                )
                .unwrap();
            let Ok(Ok(LowerMacEvent::Terminal(LowerMacTerminal::TxCompleted(
                TxCompleted::Single(terminal),
            )))) = phase.next_event().await
            else {
                panic!("terminal before removal")
            };
            let oer_protocols_ieee80211_contracts_port::TerminalParts {
                outcome: completion,
                payload,
                ..
            } = terminal.into_parts(&mut terminal_access);
            let (buffer, _) = payload.into_parts();

            assert_eq!(completion, TxOutcome::Withdrawn(WithdrawCause::KeyRemoved));
            let ModelAnyBuffer::SmallSent(mpdu) = buffer else {
                unreachable!()
            };
            assert_eq!(mpdu.packet_number(), pn);
            assert_eq!(mpdu.buffer.memory.as_ptr(), pointer);
        }
        let Ok(Ok(LowerMacEvent::Terminal(LowerMacTerminal::KeyRemoved(_)))) =
            phase.next_event().await
        else {
            panic!("the preparation no longer refers to the key")
        };
        assert_eq!(air.sent().len(), sent_before, "retired key never publishes");
    }
}
#[test]
fn key_removal_waits_for_prepared_retransmission_until_cancellation_or_its_terminal() {
    for cancel in [true, false] {
        let (backend, _) = backend();
        let _port_end = block_on(run_model(backend, Body(PreparedRemoval { cancel })));
    }
}

struct RetryRole;
impl Work for RetryRole {
    async fn work<'p, 'id: 'p, Id: SlotIdentity + 'id>(
        self,
        phase: &mut ModelPhase<'p, 'id, ModelCaps, ModelBackend<Id>>,
        resources: ModelResources<'id, ModelBackend<Id>>,
    ) {
        let mut terminal_access = resources.terminal_access;
        let ModelResources {
            mut tickets,
            mut small,
            ..
        } = resources;
        let source = <ModelTypes<ModelBackend<Id>> as TxBufferPool<ModelSmall>>::prepare::<
            AccessPoint,
            ModelCaps,
        >(
            (TxSource::Fresh {
                buffer: small.remove(0),
                frame: frame(phase, 40, false),
                body: None,
                protection: TxProtection::Open,
            })
            .check()
            .ok()
            .unwrap(),
        )
        .ok()
        .unwrap();
        phase
            .access_point()
            .submit(
                (oer_protocols_ieee80211_contracts_port::Attempt {
                    context: (),
                    ticket: tickets.remove(0),
                    source,
                    access_category: WmmAccessCategory::BestEffort,
                    protection: Protection::None,
                    power: SupportedPower::new(TxPower::Calibrated).unwrap(),
                    coex_operation: oer_protocols_ieee80211_contracts_port::TxOperation::Data,
                })
                .admit(),
            )
            .unwrap();
        let (_ticket, buffer) = completion(phase, &mut terminal_access).await;
        let ModelAnyBuffer::SmallSent(mpdu) = buffer else {
            unreachable!()
        };
        let before = mpdu.buffer.memory.to_vec();
        let TxFrame::Individual { plan, .. } = data_frame(phase, false) else {
            unreachable!()
        };
        let refused = <ModelTypes<ModelBackend<Id>> as TxBufferPool<ModelSmall>>::prepare::<
            Station,
            ModelCaps,
        >(
            (TxSource::Retransmission {
                mpdu,
                plan: RetransmissionPlan::Individual(plan),
            })
            .check()
            .ok()
            .unwrap(),
        )
        .err()
        .unwrap();
        assert_eq!(refused.reason, PreparationError::RetransmissionKind);
        let TxSource::Retransmission { mpdu, .. } = refused.operation else {
            unreachable!()
        };
        assert_eq!(mpdu.buffer.memory.as_ref(), before.as_slice());
        assert!(phase.held.events.is_empty());
    }
}
#[test]
fn a_sent_mpdu_cannot_change_its_interface_role_during_retransmission() {
    let (backend, _) = backend();
    let _port_end = block_on(run_model(backend, Body(RetryRole)));
}

struct MissingLoan;
impl Work for MissingLoan {
    async fn work<'p, 'id: 'p, Id: SlotIdentity + 'id>(
        self,
        phase: &mut ModelPhase<'p, 'id, ModelCaps, ModelBackend<Id>>,
        mut resources: ModelResources<'id, ModelBackend<Id>>,
    ) {
        let ticket = resources.tickets.remove(0);
        let mut buffer = resources.small.remove(0);
        let original_loan = buffer.loan.take();
        let slot = buffer.slot.index();
        buffer.memory.fill(0x5a);
        let pointer = buffer.memory.as_ptr();
        let body = ModelBody(alloc::vec![0x71; 16]);
        let body_pointer = body.0.as_ptr();
        let result = try_source(
            phase,
            ticket,
            TxSource::Fresh {
                buffer,
                frame: frame(phase, 40, false),
                body: Some(body),
                protection: TxProtection::Open,
            },
        );
        let Err((reason, ticket, returned, body)) = result else {
            panic!("missing body loan is refused before admission")
        };
        assert_eq!(
            reason,
            TxRefusal::Preparation(PreparationError::MissingBodyLoan)
        );
        let ModelAnyBuffer::Small(mut buffer) = returned else {
            panic!("original class")
        };
        assert_eq!(buffer.slot.index(), slot);
        assert_eq!(buffer.memory.as_ptr(), pointer);
        assert!(buffer.memory[40..].iter().all(|byte| *byte == 0x5a));
        assert_eq!(body.as_ref().unwrap().0.as_ptr(), body_pointer);
        assert!(phase.held.commands.is_empty());
        assert!(phase.held.events.is_empty());
        buffer.loan = original_loan;
        try_source(
            phase,
            ticket,
            TxSource::Fresh {
                buffer,
                frame: frame(phase, 40, false),
                body,
                protection: TxProtection::Open,
            },
        )
        .ok()
        .unwrap();
        let (_, returned) = completion(phase, &mut resources.terminal_access).await;
        let ModelAnyBuffer::SmallSent(mut mpdu) = returned else {
            panic!("published sent MPDU")
        };
        let original_loan = mpdu.buffer.loan.take();
        let plan = match data_frame(phase, false) {
            TxFrame::Individual { plan, .. } => RetransmissionPlan::Individual(plan),
            _ => panic!("individual frame"),
        };
        let checked = TxSource::Retransmission { mpdu, plan }
            .check()
            .ok()
            .unwrap();
        let Err(refused) = <ModelTypes<ModelBackend<Id>> as TxBufferPool<ModelSmall>>::prepare::<
            Station,
            ModelCaps,
        >(checked) else {
            panic!("missing retry loan is refused")
        };
        assert_eq!(refused.reason, PreparationError::MissingBodyLoan);
        let TxSource::Retransmission { mut mpdu, .. } = refused.operation else {
            panic!("same sent owner")
        };
        assert_eq!(mpdu.buffer.memory.as_ptr(), pointer);
        assert_eq!(mpdu.buffer.slot.index(), slot);
        mpdu.buffer.loan = original_loan;
        assert!(phase.held.commands.is_empty());
        assert!(phase.held.events.is_empty());
    }
}
#[test]
fn missing_body_loans_refuse_fresh_and_retry_with_all_owners_unchanged() {
    let (world, air) = backend();
    let _report = block_on(run_model(world, Body(MissingLoan)));
    assert!(air.sent().len() == 1);
    assert!(!air.failed());
}
