//! The station interface's data arenas on the model phase: answers from
//! the air's FIFO, the air's record, key custody and publication barriers.

use super::*;
use oer_protocols_ieee80211_contracts_port::arena::{
    ArenaFormat, ArenaPublication, AttemptEnd, FrameMask, PublishError, PublishRefused, TxArena,
};
use oer_protocols_ieee80211_contracts_port::{MpduProtection, MpduStartSpacing, PhaseRate};
use oer_protocols_ieee80211_wire::ccmp::{CcmpHeader, CcmpKeyId, CcmpPacketNumber};
use oer_protocols_ieee80211_wire::frame::FrameControlFields;
use oer_protocols_radio_port::{Disabled, Ended, Owner, PortBody};

/// A QoS data MPDU of sequence number `sequence`: 30 octets open, 40 with
/// a CCMP header of `packet_number`.
fn write_frame(sequence: u16, packet_number: Option<u64>, region: &mut [u8]) -> Option<u16> {
    let length = if packet_number.is_some() { 40 } else { 30 };
    let frame = region.get_mut(..length)?;
    frame.fill(0x5a);
    frame[0] = 0x88;
    frame[1] = if packet_number.is_some() { 0x41 } else { 0x01 };
    frame[4..10].copy_from_slice(&PEER);
    frame[10..16].copy_from_slice(&STATION);
    frame[16..22].copy_from_slice(&PEER);
    frame[22..24].copy_from_slice(&(sequence << 4).to_le_bytes());
    frame[24..26].fill(0);
    if let Some(packet_number) = packet_number {
        frame[26..34].copy_from_slice(
            &CcmpHeader::new(CcmpPacketNumber::new(packet_number)?, CcmpKeyId::PAIRWISE).encode(),
        );
    }
    u16::try_from(length).ok()
}

fn memory() -> ModelNetworkMemory {
    Box::new([0; ModelLarge::CAPACITY])
}

fn ht_rate<'p, 'id: 'p, Id: SlotIdentity + 'id>(
    phase: &mut ModelPhase<'p, 'id, ModelCaps, ModelBackend<Id>>,
) -> PhaseRate<'p, 'id, ModelCaps> {
    phase
        .rate(PhyRate::Ht(
            oer_protocols_ieee80211_wire::phy::HtRate::new(
                HtMcs::new(7).unwrap(),
                oer_protocols_ieee80211_wire::phy::PpduBandwidth::Mhz20,
                false,
            )
            .unwrap(),
        ))
        .unwrap()
}

fn publication<'p, 'id>(
    arena: oer_protocols_ieee80211_contracts_port::Arena,
    format: ArenaFormat<'p, 'id, ModelCaps>,
    encryption: MpduProtection,
) -> ArenaPublication<'p, 'id, ModelCaps> {
    ArenaPublication {
        arena,
        format,
        access_category: WmmAccessCategory::BestEffort,
        protection: Protection::None,
        encryption,
        power: SupportedPower::new(TxPower::Calibrated).unwrap(),
    }
}

/// The next arena completion of the phase's stream.
async fn arena_end<'p, 'id: 'p, Id: SlotIdentity + 'id>(
    phase: &mut ModelPhase<'p, 'id, ModelCaps, ModelBackend<Id>>,
) -> ModelArenaCompleted {
    loop {
        match phase.next_event().await {
            Ok(Ok(LowerMacEvent::Terminal(LowerMacTerminal::TxCompleted(TxCompleted::Arena(
                done,
            ))))) => return done,
            Ok(Ok(LowerMacEvent::Received { frame, .. })) => phase.recycle(frame),
            _ => panic!("the published arena completes"),
        }
    }
}

/// Answers from the air's FIFO, its record and the default answers.
struct Answers;

impl Work for Answers {
    async fn work<'p, 'id: 'p, Id: SlotIdentity + 'id>(
        self,
        phase: &mut ModelPhase<'p, 'id, ModelCaps, ModelBackend<Id>>,
        _: ModelResources<'id, ModelBackend<Id>>,
    ) {
        let air = phase.held.air.clone();
        let single = phase.rate(PhyRate::Legacy(LegacyRate::Ofdm24M)).unwrap();
        let ht = ht_rate(phase);
        let aggregate = phase.station().ampdu_rate(ht).ok().unwrap();
        let [first, second] = {
            let mut station = phase.station();
            let mut arena = station.data_arena();
            let arenas = arena.arenas().unwrap();
            // Sequence numbers wrap inside the window.
            for sequence in [4094, 4095, 0] {
                arena
                    .intake(&arenas[0], memory(), |region| {
                        write_frame(sequence, None, region)
                    })
                    .unwrap();
            }
            arenas
        };
        // Bit n acknowledges position n, by the frames' sequence numbers.
        air.answer(AirAnswer::BlockAck(0b101));
        {
            let mut station = phase.station();
            let mut arena = station.data_arena();
            let format = ArenaFormat::Aggregate {
                rate: aggregate,
                spacing: MpduStartSpacing::new(0).unwrap(),
            };
            arena
                .publish(
                    publication(first, format, MpduProtection::Open),
                    air.monotonic_now(),
                )
                .unwrap();
            assert!(arena.on_air().is_some());
        }
        let done = arena_end(phase).await;
        let AttemptEnd::Answered {
            report, response, ..
        } = *done.end()
        else {
            panic!("the BlockAck answers the A-MPDU");
        };
        assert_eq!(response, ModelBlockAckEvidence::Measured(MODEL_ACK_SNR_DB));
        assert_eq!(report.start_sequence, SequenceNumber::new(4094).unwrap());
        assert_eq!(report.bitmap, 0b101);
        let sent = air.sent();
        assert_eq!(sent.len(), 3);
        assert!(sent.iter().all(|frame| frame.ticket.is_none()));
        assert!(sent.iter().all(|frame| frame.key.is_none()));
        assert_eq!(sent[1].plaintext, sent[1].frame);
        // The missing frame stays and goes again with its Retry bit; no
        // queued answer acknowledges every frame.
        let mut returned = 0;
        let first = {
            let mut station = phase.station();
            let mut arena = station.data_arena();
            let kept = arena
                .retain(done, FrameMask::from_bits(0b010).unwrap(), |_| {
                    returned += 1
                })
                .unwrap();
            assert_eq!(arena.len(kept.id()), 1);
            let format = ArenaFormat::Aggregate {
                rate: aggregate,
                spacing: MpduStartSpacing::new(0).unwrap(),
            };
            arena
                .publish(
                    publication(kept, format, MpduProtection::Open),
                    air.monotonic_now(),
                )
                .unwrap();
            arena.len(second.id())
        };
        assert_eq!(returned, 2);
        assert_eq!(first, 0);
        let done = arena_end(phase).await;
        let AttemptEnd::Answered { report, .. } = *done.end() else {
            panic!("the default BlockAck answers the retried A-MPDU");
        };
        assert_eq!(report.bitmap, 1);
        let retried = air.sent().pop().unwrap();
        assert!(FrameControlFields::from_bytes(*retried.plaintext.first_chunk().unwrap()).retry());
        // A single MPDU takes the default ACK; an unmeasured BlockAck of a
        // single MPDU is of the wrong shape.
        let first = {
            let mut station = phase.station();
            let mut arena = station.data_arena();
            let first = arena.release(done, drop).unwrap();
            arena
                .intake(&first, memory(), |region| write_frame(1, None, region))
                .unwrap();
            arena
                .publish(
                    publication(
                        first,
                        ArenaFormat::Single(once(single)),
                        MpduProtection::Open,
                    ),
                    air.monotonic_now(),
                )
                .unwrap();
            arena.len(second.id())
        };
        assert_eq!(first, 0);
        let done = arena_end(phase).await;
        assert!(matches!(done.end(), AttemptEnd::Acked { .. }));
        air.answer(AirAnswer::HardwareTimeout);
        {
            let mut station = phase.station();
            let mut arena = station.data_arena();
            let first = arena
                .retain(done, FrameMask::first(1).unwrap(), drop)
                .unwrap();
            arena
                .publish(
                    publication(
                        first,
                        ArenaFormat::Single(once(single)),
                        MpduProtection::Open,
                    ),
                    air.monotonic_now(),
                )
                .unwrap();
        }
        let done = arena_end(phase).await;
        assert!(matches!(done.end(), AttemptEnd::Aborted));
        air.answer(AirAnswer::BlockAckUnmeasured(1));
        let mut station = phase.station();
        let mut arena = station.data_arena();
        let first = arena
            .retain(done, FrameMask::first(1).unwrap(), drop)
            .unwrap();
        let refused = arena.publish(
            publication(
                first,
                ArenaFormat::Single(once(single)),
                MpduProtection::Open,
            ),
            air.monotonic_now(),
        );
        assert!(matches!(refused, Err(PublishError::Fault(ModelFault))));
        assert!(air.failed());
    }
}

#[test]
fn arena_answers_come_from_the_air_fifo_and_its_frames_are_recorded_without_a_ticket() {
    let (backend, _) = backend();
    let _port_end = block_on(run_model(backend, Body(Answers)));
}

/// An unmeasured BlockAck delivers the A-MPDU with unmeasured evidence.
struct Unmeasured;

impl Work for Unmeasured {
    async fn work<'p, 'id: 'p, Id: SlotIdentity + 'id>(
        self,
        phase: &mut ModelPhase<'p, 'id, ModelCaps, ModelBackend<Id>>,
        _: ModelResources<'id, ModelBackend<Id>>,
    ) {
        let air = phase.held.air.clone();
        let ht = ht_rate(phase);
        let aggregate = phase.station().ampdu_rate(ht).ok().unwrap();
        air.answer(AirAnswer::BlockAckUnmeasured(u64::MAX));
        {
            let mut station = phase.station();
            let mut arena = station.data_arena();
            let [first, _] = arena.arenas().unwrap();
            for sequence in [7, 8] {
                arena
                    .intake(&first, memory(), |region| {
                        write_frame(sequence, None, region)
                    })
                    .unwrap();
            }
            let format = ArenaFormat::Aggregate {
                rate: aggregate,
                spacing: MpduStartSpacing::new(0).unwrap(),
            };
            arena
                .publish(
                    publication(first, format, MpduProtection::Open),
                    air.monotonic_now(),
                )
                .unwrap();
        }
        let done = arena_end(phase).await;
        let AttemptEnd::Answered {
            report, response, ..
        } = *done.end()
        else {
            panic!("the BlockAck answers the A-MPDU");
        };
        assert_eq!(response, ModelBlockAckEvidence::Unmeasured);
        assert_eq!(report.bitmap, 0b11);
        let mut station = phase.station();
        let mut arena = station.data_arena();
        drop(arena.release(done, drop).unwrap());
    }
}

#[test]
fn an_unmeasured_block_ack_answers_the_a_mpdu_without_snr() {
    let (backend, _) = backend();
    let _port_end = block_on(run_model(backend, Body(Unmeasured)));
}

/// Encryption needs the installed key, seals each frame under it and keeps
/// a use that the key's removal waits for until the arena empties.
struct KeyCustody;

impl Work for KeyCustody {
    async fn work<'p, 'id: 'p, Id: SlotIdentity + 'id>(
        self,
        phase: &mut ModelPhase<'p, 'id, ModelCaps, ModelBackend<Id>>,
        resources: ModelResources<'id, ModelBackend<Id>>,
    ) {
        let air = phase.held.air.clone();
        let single = phase.rate(PhyRate::Legacy(LegacyRate::Ofdm24M)).unwrap();
        let station_association = ModelKeys::<ModelBackend<Id>, Station>::begin_association(
            resources.station_association,
            oer_protocols_ieee80211_wire::security::Spp::NotNegotiated,
        );
        let [slot, _] = resources.station_pairwise;
        let [first, _] = {
            let mut station = phase.station();
            let mut arena = station.data_arena();
            let arenas = arena.arenas().unwrap();
            arena
                .intake(&arenas[0], memory(), |region| {
                    write_frame(3, Some(17), region)
                })
                .unwrap();
            arenas
        };
        let format = ArenaFormat::Single(once(single));
        let first = {
            let mut station = phase.station();
            let mut arena = station.data_arena();
            let refused = arena.publish(
                publication(first, format, MpduProtection::Ccmp),
                air.monotonic_now(),
            );
            let Err(PublishError::Refused {
                publication,
                reason: PublishRefused::NoKey,
            }) = refused
            else {
                panic!("encryption without an installed key is refused");
            };
            publication.arena
        };
        let key = installed_station_pairwise(
            phase,
            &station_association,
            slot,
            IndividualAddress::new(PEER).unwrap(),
            Ccmp128Key::new(&[7; 16]),
        )
        .await
        .expect("the PTK installation terminal");
        // A no-ACK answer keeps the frame in the arena, under its key.
        air.answer(AirAnswer::NoResponse);
        {
            let mut station = phase.station();
            let mut arena = station.data_arena();
            arena
                .publish(
                    publication(first, format, MpduProtection::Ccmp),
                    air.monotonic_now(),
                )
                .unwrap();
        }
        let done = arena_end(phase).await;
        assert!(matches!(done.end(), AttemptEnd::NoResponse(_)));
        let sent = air.sent();
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].key, Some((0, true)));
        assert_eq!(sent[0].packet_number, Some(17));
        assert_ne!(sent[0].frame, sent[0].plaintext);
        assert_eq!(sent[0].frame.len(), sent[0].plaintext.len() + 8);
        let first = {
            let mut station = phase.station();
            let mut arena = station.data_arena();
            arena
                .retain(done, FrameMask::first(1).unwrap(), drop)
                .unwrap()
        };
        let (association, port_key) = (station_association, key);
        phase.station().remove_pairwise_key(association, port_key);
        for _ in 0..8 {
            yield_now().await;
        }
        assert!(
            phase.held.events.is_empty(),
            "the removal waits for the retained frame"
        );
        {
            let mut station = phase.station();
            let mut arena = station.data_arena();
            let mut returned = 0;
            arena.withdraw(&first, |_| returned += 1).unwrap();
            assert_eq!(returned, 1);
        }
        let removed = loop {
            match phase.next_event().await {
                Ok(Ok(LowerMacEvent::Terminal(LowerMacTerminal::KeyRemoved(_)))) => break true,
                Ok(Ok(LowerMacEvent::Received { frame, .. })) => phase.recycle(frame),
                _ => break false,
            }
        };
        assert!(removed, "the emptied arena returns the key's last use");
    }
}

#[test]
fn an_encrypted_arena_holds_the_installed_key_until_it_empties() {
    let (backend, _) = backend();
    let _port_end = block_on(run_model(backend, Body(KeyCustody)));
}

/// A closing gate and a pending station configuration refuse publication
/// as busy; a closing gate waits for the arena on the air.
struct Barriers;

impl Work for Barriers {
    async fn work<'p, 'id: 'p, Id: SlotIdentity + 'id>(
        self,
        phase: &mut ModelPhase<'p, 'id, ModelCaps, ModelBackend<Id>>,
        resources: ModelResources<'id, ModelBackend<Id>>,
    ) {
        let air = phase.held.air.clone();
        let single = phase.rate(PhyRate::Legacy(LegacyRate::Ofdm24M)).unwrap();
        let format = ArenaFormat::Single(once(single));
        let [first, second] = {
            let mut station = phase.station();
            let mut arena = station.data_arena();
            let arenas = arena.arenas().unwrap();
            for arena_right in &arenas {
                arena
                    .intake(arena_right, memory(), |region| write_frame(5, None, region))
                    .unwrap();
            }
            arenas
        };
        // The air keeps the result of the published arena.
        air.hold_completion(WmmAccessCategory::BestEffort, true);
        {
            let mut station = phase.station();
            let mut arena = station.data_arena();
            arena
                .publish(
                    publication(first, format, MpduProtection::Open),
                    air.monotonic_now(),
                )
                .unwrap();
        }
        phase.close_gate(resources.gate);
        let second = {
            let mut station = phase.station();
            let mut arena = station.data_arena();
            let Err(PublishError::Refused {
                publication,
                reason: PublishRefused::Busy,
            }) = arena.publish(
                publication(second, format, MpduProtection::Open),
                air.monotonic_now(),
            )
            else {
                panic!("a closing gate refuses publication");
            };
            publication.arena
        };
        for _ in 0..8 {
            yield_now().await;
        }
        assert!(
            phase.held.events.is_empty(),
            "the gate waits for the arena on the air"
        );
        air.hold_completion(WmmAccessCategory::BestEffort, false);
        let done = arena_end(phase).await;
        let closed = loop {
            match phase.next_event().await {
                Ok(Ok(LowerMacEvent::Terminal(LowerMacTerminal::GateClosed(closed)))) => {
                    break closed;
                }
                Ok(Ok(LowerMacEvent::Received { frame, .. })) => phase.recycle(frame),
                _ => panic!("the gate closes after the arena"),
            }
        };
        let _open = phase.open_gate(closed);
        let mut station = phase.station();
        let mut arena = station.data_arena();
        drop(arena.release(done, drop).unwrap());
        arena
            .publish(
                publication(second, format, MpduProtection::Open),
                air.monotonic_now(),
            )
            .unwrap();
    }
}

#[test]
fn a_closing_gate_refuses_publication_and_waits_for_the_arena_on_the_air() {
    let (backend, _) = backend();
    let _port_end = block_on(run_model(backend, Body(Barriers)));
}

/// A disable drains a published arena whose result the air holds: it ends
/// aborted, and its completion reaches the disable's terminal sink.
struct DrainAborts(Rc<Cell<bool>>);

impl<Id: SlotIdentity> PortBody<Id, ModelBackend<Id>> for DrainAborts {
    type Output = ();
    async fn run<'a, 'id>(
        self,
        owner: Owner<'a, 'id, Id, ModelBackend<Id>, Disabled>,
        _: ModelResources<'id, ModelBackend<Id>>,
    ) -> Ended<Id, ModelBackend<Id>, ()>
    where
        ModelBackend<Id>: 'id,
        Id: 'id,
        'id: 'a,
    {
        let Ok(mut enabled) = owner.enable().await else {
            panic!("the model enables");
        };
        {
            let mut phase = enabled.phase();
            let air = phase.held.air.clone();
            let single = phase.rate(PhyRate::Legacy(LegacyRate::Ofdm24M)).unwrap();
            air.hold_completion(WmmAccessCategory::BestEffort, true);
            let mut station = phase.station();
            let mut arena = station.data_arena();
            let [first, _] = arena.arenas().unwrap();
            arena
                .intake(&first, memory(), |region| write_frame(9, None, region))
                .unwrap();
            arena
                .publish(
                    publication(
                        first,
                        ArenaFormat::Single(once(single)),
                        MpduProtection::Open,
                    ),
                    air.monotonic_now(),
                )
                .unwrap();
        }
        let aborted = self.0;
        let mut sink = |terminal| {
            if let LowerMacTerminal::TxCompleted(TxCompleted::Arena(done)) = terminal {
                aborted.set(matches!(done.end(), AttemptEnd::Aborted));
            }
            core::ops::ControlFlow::Continue(())
        };
        match enabled.disable(None, &mut sink).await {
            Ok(disabled) => Ended::uninstall(disabled, (), ()),
            Err(poisoned) => Ended::fail_stop(poisoned.fail_stop(), ()),
        }
    }
}

#[test]
fn a_disable_aborts_the_arena_on_the_air_and_returns_its_completion() {
    let aborted = Rc::new(Cell::new(false));
    let (backend, _) = backend();
    let _port_end = block_on(run_model(backend, DrainAborts(aborted.clone())));
    assert!(aborted.get());
}

/// A single MPDU's plan of one transmission at `rate`.
fn once<R>(rate: R) -> oer_protocols_ieee80211_contracts_port::RetryPlan<R> {
    oer_protocols_ieee80211_contracts_port::RetryPlan::new(
        oer_protocols_ieee80211_contracts_port::RetryStage {
            rate,
            transmissions: core::num::NonZeroU8::MIN,
            protection: oer_protocols_ieee80211_contracts_port::Protection::None,
        },
    )
}
