use super::*;
use alloc::collections::BTreeSet;
use oer_ieee80211_lower_mac::association::RxBlockAckPool;
use oer_ieee80211_lower_mac::{AggregateAttempt, MpduStartSpacing};

struct Window {
    sequences: &'static [u16],
    acknowledged: u64,
    refused: Option<u16>,
}

impl Work for Window {
    async fn work<'p, 'id: 'p>(
        self,
        phase: &mut ModelPhase<'p, 'id>,
        resources: ModelResources<'id>,
    ) {
        let mut terminal_access = resources.terminal_access;
        let ModelResources {
            mut tickets,
            mut small,
            ..
        } = resources;
        let air = phase.held.air.clone();
        let rate = phase
            .rate(PhyRate::Ht(
                oer_ieee80211_mac::phy::HtRate::new(
                    HtMcs::new(7).unwrap(),
                    oer_ieee80211_mac::phy::PpduBandwidth::Mhz20,
                    false,
                )
                .unwrap(),
            ))
            .unwrap();
        let mut station = phase.station();
        let rate = station.ampdu_rate(rate).ok().unwrap();
        let aggregate = station.aggregate(
            IndividualAddress::new(PEER).unwrap(),
            Tid::new(0).unwrap(),
            rate,
            TxProtection::Open,
        );
        let buffer = small.remove(0);
        let mut indices = alloc::vec![buffer.slot.index()];
        let mut aggregate = push_subframe(
            aggregate,
            buffer,
            subframe_frame(self.sequences[0]),
            Some(ModelBody(alloc::vec![0x5a])),
        );
        for &sequence in &self.sequences[1..] {
            let buffer = small.remove(0);
            indices.push(buffer.slot.index());
            aggregate = push_subframe(aggregate, buffer, subframe_frame(sequence), None);
        }
        if let Some(sequence) = self.refused {
            let buffer = small.remove(0);
            let refused_index = buffer.slot.index();
            let operation = CheckedAggregate::new(
                aggregate,
                SubframeSource::Fresh {
                    buffer,
                    frame: ClassSubframe::new(subframe_frame(sequence)).ok().unwrap(),
                    body: Some(ModelBody(alloc::vec![0x7a])),
                },
            )
            .ok()
            .unwrap();
            let Err(AggregateRefused {
                reason,
                aggregate: returned,
                subframe,
            }) = <ModelTypes as TxBufferPool<ModelSmall>>::push_aggregate(operation)
            else {
                panic!("out-of-window subframe was accepted")
            };
            assert_eq!(reason, PreparationError::BlockAckWindow);
            let SubframeSource::Fresh { buffer, body, .. } = subframe else {
                panic!()
            };
            assert_eq!(buffer.slot.index(), refused_index);
            assert_eq!(body.unwrap().0, [0x7a]);
            assert!(air.sent().is_empty(), "refusal must precede publication");
            aggregate = returned;
        }
        air.answer(AirAnswer::BlockAck(self.acknowledged));
        let ticket = tickets.remove(0);
        let ticket_index = ticket.index();
        station
            .submit_ampdu(
                (AggregateAttempt {
                    context: (),
                    ticket,
                    aggregate,
                    protection: Protection::None,
                    power: SupportedPower::new(TxPower::Calibrated).unwrap(),
                    coex: SupportedCoex::new(CoexPriority::Normal).unwrap(),
                    min_mpdu_start_spacing: MpduStartSpacing::new(0).unwrap(),
                })
                .admit(),
            )
            .unwrap();
        let (ticket, subframes, report) = aggregate_completion(phase, &mut terminal_access).await;
        let buffers: Vec<_> = subframes
            .into_iter()
            .map(|mpdu| {
                let (buffer, body) = mpdu.into_parts();
                assert!(body.is_none());
                buffer
            })
            .collect();
        assert_eq!(ticket.index(), ticket_index);
        let report = report.unwrap();
        let expected: BTreeSet<u16> = self
            .sequences
            .iter()
            .enumerate()
            .filter(|(index, _)| self.acknowledged & (1 << index) != 0)
            .map(|(_, &sequence)| sequence)
            .collect();
        // Independent oracle: decode bits to the finite set of wire sequence
        // numbers; do not repeat the model's forward-distance calculation.
        let observed: BTreeSet<u16> = (0..64)
            .filter(|bit| report.bitmap & (1 << bit) != 0)
            .map(|bit| (report.start_sequence.get() + bit) % 4096)
            .collect();
        assert_eq!(observed, expected);
        assert_eq!(buffers.len(), indices.len());
        for (index, (buffer, original_index)) in buffers.into_iter().zip(indices).enumerate() {
            let ModelAnyBuffer::SmallSent(sent) = buffer else {
                panic!()
            };
            let (buffer, body) = <ModelTypes as TxBufferPool<ModelSmall>>::renew(sent);
            assert_eq!(buffer.slot.index(), original_index);
            if index == 0 {
                assert_eq!(body.unwrap().0, [0x5a]);
            } else {
                assert!(body.is_none());
            }
        }
    }
}

#[test]
fn bitmap_matches_acknowledged_sequence_set_across_wrap_and_reordering() {
    for (sequences, acknowledged) in [
        (&[0, 63][..], 0b11),
        (&[4095, 0, 62][..], 0b110),
        (&[40, 43, 103, 41][..], 0b1010),
        (&[40, 103, 41, 43][..], 0b1100),
    ] {
        let (backend, _) = backend();
        let _port_end = block_on(run_port(
            backend,
            Body(Window {
                sequences,
                acknowledged,
                refused: None,
            }),
        ));
    }
}

#[test]
fn aggregate_does_not_alias_blockack_bit63() {
    for (start, refused) in [
        (&[0][..], 64),
        (&[4095][..], 63),
        (&[0][..], 4095),
        (&[40][..], 39),
        (&[40, 43][..], 43),
    ] {
        let (backend, _) = backend();
        let _port_end = block_on(run_port(
            backend,
            Body(Window {
                sequences: start,
                acknowledged: 1,
                refused: Some(refused),
            }),
        ));
    }
}

struct RefusedSent(u16);
impl Work for RefusedSent {
    async fn work<'p, 'id: 'p>(
        self,
        phase: &mut ModelPhase<'p, 'id>,
        resources: ModelResources<'id>,
    ) {
        let mut terminal_access = resources.terminal_access;
        let ModelResources {
            mut tickets,
            mut small,
            ..
        } = resources;
        let rate = phase
            .rate(PhyRate::Ht(
                oer_ieee80211_mac::phy::HtRate::new(
                    HtMcs::new(7).unwrap(),
                    oer_ieee80211_mac::phy::PpduBandwidth::Mhz20,
                    false,
                )
                .unwrap(),
            ))
            .unwrap();
        let mut station = phase.station();
        let rate = station.ampdu_rate(rate).ok().unwrap();
        let aggregate = station.aggregate(
            IndividualAddress::new(PEER).unwrap(),
            Tid::new(0).unwrap(),
            rate,
            TxProtection::Open,
        );
        let original = small.remove(0);
        let index = original.slot.index();
        let aggregate = push_subframe(
            aggregate,
            original,
            subframe_frame(40),
            Some(ModelBody(alloc::vec![0x7a])),
        );
        station
            .submit_ampdu(
                (AggregateAttempt {
                    context: (),
                    ticket: tickets.remove(0),
                    aggregate,
                    protection: Protection::None,
                    power: SupportedPower::new(TxPower::Calibrated).unwrap(),
                    coex: SupportedCoex::new(CoexPriority::Normal).unwrap(),
                    min_mpdu_start_spacing: MpduStartSpacing::new(0).unwrap(),
                })
                .admit(),
            )
            .unwrap();
        let (_, subframes, _) = aggregate_completion(phase, &mut terminal_access).await;
        let mut buffers: Vec<_> = subframes
            .into_iter()
            .map(|mpdu| {
                let (buffer, body) = mpdu.into_parts();
                assert!(body.is_none());
                buffer
            })
            .collect();
        let ModelAnyBuffer::SmallSent(mpdu) = buffers.remove(0) else {
            panic!()
        };
        let mut station = phase.station();
        let aggregate = station.aggregate(
            IndividualAddress::new(PEER).unwrap(),
            Tid::new(0).unwrap(),
            rate,
            TxProtection::Open,
        );
        let aggregate = push_subframe(aggregate, small.remove(0), subframe_frame(self.0), None);
        let operation = CheckedAggregate::new(aggregate, SubframeSource::Sent(mpdu))
            .ok()
            .unwrap();
        let Err(AggregateRefused {
            reason,
            aggregate,
            subframe,
        }) = <ModelTypes as TxBufferPool<ModelSmall>>::push_aggregate(operation)
        else {
            panic!("duplicate or lower-than-head Sent is refused")
        };
        assert_eq!(reason, PreparationError::BlockAckWindow);
        assert_eq!(aggregate.subframes(), 1);
        let SubframeSource::Sent(mpdu) = subframe else {
            panic!("refusal preserves Sent")
        };
        let (buffer, body) = <ModelTypes as TxBufferPool<ModelSmall>>::renew(mpdu);
        assert_eq!(buffer.slot.index(), index);
        assert_eq!(body.unwrap().0, [0x7a]);
        assert_eq!(
            phase.held.air.sent().len(),
            1,
            "no second publication on refusal"
        );
    }
}

#[test]
fn a_duplicate_or_out_of_window_sent_subframe_returns_its_owner_and_body() {
    for head in [40, 41] {
        let (backend, _) = backend();
        let _port_end = block_on(run_port(backend, Body(RefusedSent(head))));
    }
}

/// Every Block Ack slot the pool mints carries a running agreement: the
/// pool's length is the capacity the backend applies, with no smaller table
/// behind it.
struct EverySlotApplied<'a>(&'a ModelAir);

impl Work for EverySlotApplied<'_> {
    async fn work<'p, 'id: 'p>(
        self,
        phase: &mut ModelPhase<'p, 'id>,
        resources: ModelResources<'id>,
    ) {
        let agreement = |tid| {
            SupportedBlockAck::new(
                IndividualAddress::new(PEER).unwrap(),
                Tid::new(tid).unwrap(),
                SequenceNumber::new(0).unwrap(),
                32,
            )
            .unwrap()
        };
        let mut running = Vec::new();
        for (tid, slot) in (0..).zip(resources.block_acks) {
            running.push(phase.station().add_block_ack(slot, agreement(tid)));
        }
        yield_now().await;
        let applied = self.0.receive_agreements();
        assert_eq!(
            applied.len(),
            <<ModelTypes as PhaseTypes<'id>>::BlockAckPool as RxBlockAckPool>::LEN
        );
        let tids: BTreeSet<_> = applied
            .iter()
            .map(|(_, agreement)| agreement.tid.get())
            .collect();
        assert_eq!(
            tids.len(),
            applied.len(),
            "each slot keeps its own agreement"
        );
        for agreement in running {
            let _slot = phase.station().remove_block_ack(agreement);
        }
    }
}

#[test]
fn every_minted_block_ack_slot_carries_an_applied_agreement() {
    let (backend, air) = backend();
    let _port_end = block_on(run_port(backend, Body(EverySlotApplied(&air))));
}
