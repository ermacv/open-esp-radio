use super::*;
use oer_protocols_ieee80211_contracts_port::arena::{
    Arena, ArenaFormat, ArenaId, ArenaPublication, AttemptEnd, FrameMask, IntakeRefused,
    PublishError, PublishRefused, TxArena,
};
use oer_protocols_ieee80211_contracts_port::{
    AmpduRate, MpduProtection, MpduStartSpacing, PhaseRate,
};
use oer_protocols_ieee80211_testing_conformance::{
    Environment,
    arena::{self as suite, Answer, Fixture, Published, check},
};
use oer_protocols_ieee80211_wire::frame::{FrameControlFields, sequence_control};

/// A QoS data MPDU of 30 octets whose sequence number is its tag.
fn write_tagged(tag: u8, region: &mut [u8]) -> Option<u16> {
    let frame = region.get_mut(..30)?;
    frame.fill(0);
    frame[0] = 0x88;
    frame[22..24].copy_from_slice(&(u16::from(tag) << 4).to_le_bytes());
    Some(30)
}

fn tag_of(frame: &[u8]) -> Option<u8> {
    let sequence = SequenceNumber::from_sequence_control(sequence_control(frame)?);
    u8::try_from(sequence.get()).ok()
}

fn memory() -> ModelNetworkMemory {
    Box::new([0; ModelLarge::CAPACITY])
}

struct ArenaFixture<'p, 'id> {
    port: ModelArena<ModelCaps>,
    single: PhaseRate<'p, 'id, ModelCaps>,
    aggregate: AmpduRate<'p, 'id, ModelCaps>,
    micros: u64,
}

impl<'p, 'id: 'p> ArenaFixture<'p, 'id> {
    fn publication(
        &self,
        arena: Arena,
        format: ArenaFormat<'p, 'id, ModelCaps>,
    ) -> ArenaPublication<'p, 'id, ModelCaps> {
        ArenaPublication {
            arena,
            format,
            access_category: WmmAccessCategory::BestEffort,
            protection: Protection::None,
            encryption: MpduProtection::Open,
            power: SupportedPower::new(TxPower::Calibrated).unwrap(),
        }
    }
}

impl<'p, 'id: 'p> Fixture<'p, 'id> for ArenaFixture<'p, 'id> {
    type Port = ModelArena<ModelCaps>;
    fn port(&mut self) -> &mut ModelArena<ModelCaps> {
        &mut self.port
    }
    fn memory(&mut self, tag: u8) -> Option<ModelNetworkMemory> {
        let mut memory = memory();
        write_tagged(tag, memory.as_mut_slice())?;
        Some(memory)
    }
    fn write(tag: u8, region: &mut [u8]) -> Option<u16> {
        write_tagged(tag, region)
    }
    fn tag(memory: &ModelNetworkMemory) -> Option<u8> {
        tag_of(memory.as_slice())
    }
    fn single(&self, arena: Arena) -> ArenaPublication<'p, 'id, ModelCaps> {
        self.publication(arena, ArenaFormat::Single(self.single))
    }
    fn aggregate(&self, arena: Arena) -> ArenaPublication<'p, 'id, ModelCaps> {
        self.publication(
            arena,
            ArenaFormat::Aggregate {
                rate: self.aggregate,
                spacing: MpduStartSpacing::new(0).unwrap(),
            },
        )
    }
    fn answer(&mut self, answer: Answer) {
        let answer = match answer {
            Answer::BlockAck(received) => {
                let on_air = self.port.on_air().unwrap();
                ModelArenaAnswer::BlockAck(self.port.block_ack(on_air, received).unwrap())
            }
            Answer::Ack => ModelArenaAnswer::Ack,
            Answer::NoResponse => ModelArenaAnswer::NoResponse,
            Answer::ProtectionFailure => ModelArenaAnswer::ProtectionFailure,
            Answer::Fault => ModelArenaAnswer::Fault,
        };
        self.port.answer(answer);
    }
    fn events(&mut self) {}
    fn now(&mut self) -> Instant {
        self.micros += 1;
        Instant::from_micros(self.micros)
    }
    fn published(&self) -> Option<Published> {
        let last = self.port.published().last()?;
        let mut published = Published {
            tags: [0; oer_protocols_ieee80211_contracts_port::ARENA_FRAMES as usize],
            count: u8::try_from(last.frames.len()).ok()?,
            retried: FrameMask::EMPTY,
            aggregate: last.aggregate.is_some(),
        };
        for ((frame, tag), index) in last
            .frames
            .iter()
            .zip(published.tags.iter_mut())
            .zip(FrameMask::first(published.count)?)
        {
            *tag = tag_of(frame)?;
            if FrameControlFields::from_bytes(*frame.first_chunk::<2>()?).retry() {
                published.retried.insert(index);
            }
        }
        Some(published)
    }
}

/// A test body over the phase's rates for a single MPDU and for an A-MPDU.
trait WithRates {
    fn run<'p, 'id: 'p>(
        self,
        single: PhaseRate<'p, 'id, ModelCaps>,
        aggregate: AmpduRate<'p, 'id, ModelCaps>,
    );
}

struct Rates<R>(R);

impl<R: WithRates> Work for Rates<R> {
    async fn work<'p, 'id: 'p, Id: SlotIdentity + 'id>(
        self,
        phase: &mut ModelPhase<'p, 'id, ModelCaps, ModelBackend<Id>>,
        _: ModelResources<'id, ModelBackend<Id>>,
    ) {
        let single = phase.rate(PhyRate::Legacy(LegacyRate::Ofdm24M)).unwrap();
        let ht = phase
            .rate(PhyRate::Ht(
                oer_protocols_ieee80211_wire::phy::HtRate::new(
                    HtMcs::new(7).unwrap(),
                    oer_protocols_ieee80211_wire::phy::PpduBandwidth::Mhz20,
                    false,
                )
                .unwrap(),
            ))
            .unwrap();
        let aggregate = phase.station().ampdu_rate(ht).ok().unwrap();
        self.0.run(single, aggregate);
    }
}

fn with_rates(run: impl WithRates) {
    let (backend, _) = backend();
    let _port_end = block_on(run_model(backend, Body(Rates(run))));
}

fn fixture<'p, 'id: 'p>(
    single: PhaseRate<'p, 'id, ModelCaps>,
    aggregate: AmpduRate<'p, 'id, ModelCaps>,
) -> ArenaFixture<'p, 'id> {
    ArenaFixture {
        port: ModelArena::new(),
        single,
        aggregate,
        micros: 0,
    }
}

#[test]
fn the_model_passes_every_arena_case() {
    struct Case;
    impl WithRates for Case {
        fn run<'p, 'id: 'p>(
            self,
            single: PhaseRate<'p, 'id, ModelCaps>,
            aggregate: AmpduRate<'p, 'id, ModelCaps>,
        ) {
            let report = check(|| fixture(single, aggregate), Environment::Model).unwrap();
            assert_eq!(usize::from(report.passed), suite::Case::ALL.len());
            assert_eq!(report.model_only, 0);
            let report = check(|| fixture(single, aggregate), Environment::Hardware).unwrap();
            assert_eq!((report.passed, report.model_only), (5, 3));
        }
    }
    with_rates(Case);
}

#[test]
fn encryption_needs_the_pairwise_key() {
    struct Case;
    impl WithRates for Case {
        fn run<'p, 'id: 'p>(
            self,
            single: PhaseRate<'p, 'id, ModelCaps>,
            aggregate: AmpduRate<'p, 'id, ModelCaps>,
        ) {
            let mut fixture = fixture(single, aggregate);
            let [first, _] = fixture.port.arenas().unwrap();
            fixture
                .port
                .intake(&first, memory(), |region| write_tagged(1, region))
                .unwrap();
            let mut publication = fixture.single(first);
            publication.encryption = MpduProtection::Ccmp;
            let Err(PublishError::Refused {
                publication,
                reason: PublishRefused::NoKey,
            }) = fixture.port.publish(publication, Instant::from_micros(1))
            else {
                panic!("an encrypted publication without a key is refused");
            };
            fixture.port.set_key(true);
            // The frame's Protected bit contradicts the encryption.
            let Err(PublishError::Refused {
                publication,
                reason: PublishRefused::Protection,
            }) = fixture.port.publish(publication, Instant::from_micros(2))
            else {
                panic!("an unprotected frame is not encrypted");
            };
            let first = publication.arena;
            fixture.port.withdraw(&first, drop).unwrap();
            fixture
                .port
                .intake(&first, memory(), |region| {
                    let length = write_tagged(1, region)?;
                    region[1] |= 0x40;
                    Some(length)
                })
                .unwrap();
            let mut publication = fixture.single(first);
            publication.encryption = MpduProtection::Ccmp;
            fixture
                .port
                .publish(publication, Instant::from_micros(3))
                .unwrap();
            assert_eq!(fixture.port.published()[0].encryption, MpduProtection::Ccmp);
        }
    }
    with_rates(Case);
}

#[test]
fn an_unanswered_publication_is_aborted_at_its_deadline() {
    struct Case;
    impl WithRates for Case {
        fn run<'p, 'id: 'p>(
            self,
            single: PhaseRate<'p, 'id, ModelCaps>,
            aggregate: AmpduRate<'p, 'id, ModelCaps>,
        ) {
            let mut fixture = fixture(single, aggregate);
            let [first, _] = fixture.port.arenas().unwrap();
            fixture
                .port
                .intake(&first, memory(), |region| write_tagged(1, region))
                .unwrap();
            let start = Instant::from_micros(100);
            let publication = fixture.aggregate(first);
            fixture.port.publish(publication, start).unwrap();
            let deadline = fixture.port.deadline().unwrap();
            assert_eq!(deadline, start.checked_add(MODEL_ARENA_TIMEOUT).unwrap());
            let before = deadline.checked_sub(Duration::from_micros(1)).unwrap();
            assert!(fixture.port.service((), before).unwrap().is_none());
            let done = fixture.port.service((), deadline).unwrap().unwrap();
            assert_eq!(*done.end(), AttemptEnd::Aborted);
            assert!(done.end().may_have_reached_air());
            // An aborted frame may have reached the air: it goes again as a retry.
            let first = fixture
                .port
                .retain(done, FrameMask::first(1).unwrap(), |_| panic!("kept"))
                .unwrap();
            let publication = fixture.aggregate(first);
            fixture.port.publish(publication, deadline).unwrap();
            assert!(fixture.published().unwrap().retried.bits() == 1);
        }
    }
    with_rates(Case);
}

#[test]
fn a_mismatched_answer_quarantines_the_model() {
    struct Case;
    impl WithRates for Case {
        fn run<'p, 'id: 'p>(
            self,
            single: PhaseRate<'p, 'id, ModelCaps>,
            aggregate: AmpduRate<'p, 'id, ModelCaps>,
        ) {
            let mut fixture = fixture(single, aggregate);
            let [first, second] = fixture.port.arenas().unwrap();
            assert!(fixture.port.arenas().is_none());
            fixture
                .port
                .intake(&first, memory(), |region| write_tagged(1, region))
                .unwrap();
            let publication = fixture.aggregate(first);
            fixture
                .port
                .publish(publication, Instant::from_micros(1))
                .unwrap();
            fixture.port.answer(ModelArenaAnswer::Ack);
            assert_eq!(
                fixture.port.service((), Instant::from_micros(2)).err(),
                Some(ModelArenaFault::MismatchedAnswer)
            );
            assert_eq!(
                fixture.port.fault(),
                Some(ModelArenaFault::MismatchedAnswer)
            );
            let Err((_, IntakeRefused::Quarantined)) =
                fixture
                    .port
                    .intake(&second, memory(), |region| write_tagged(2, region))
            else {
                panic!("a quarantined model refuses intake");
            };
            assert_eq!(fixture.port.len(ArenaId::First), 1);
        }
    }
    with_rates(Case);
}

#[test]
fn intake_refuses_a_declined_or_oversized_frame_with_its_memory() {
    let mut port = ModelArena::<ModelCaps>::new();
    let [first, _] = TxArena::arenas(&mut port).unwrap();
    let Err((_, IntakeRefused::Declined)) = TxArena::intake(&mut port, &first, memory(), |_| None)
    else {
        panic!("a declined frame is refused");
    };
    let Err((_, IntakeRefused::Length)) = TxArena::intake(&mut port, &first, memory(), |region| {
        u16::try_from(region.len() + 1).ok()
    }) else {
        panic!("a frame longer than its memory is refused");
    };
    let Err((_, IntakeRefused::Length)) = TxArena::intake(&mut port, &first, memory(), |_| Some(0))
    else {
        panic!("an empty frame is refused");
    };
    assert_eq!(TxArena::len(&port, ArenaId::First), 0);
    let index = TxArena::intake(&mut port, &first, memory(), |region| {
        write_tagged(3, region)
    })
    .unwrap();
    assert_eq!(index.get(), 0);
    let mut returned = Vec::new();
    TxArena::withdraw(&mut port, &first, |memory| {
        returned.push(tag_of(memory.as_slice()))
    })
    .unwrap();
    assert_eq!(returned, [Some(3)]);
}

#[test]
fn an_aggregate_past_the_length_limit_is_refused_and_a_single_carries_position_0() {
    struct Case;
    impl WithRates for Case {
        fn run<'p, 'id: 'p>(
            self,
            single: PhaseRate<'p, 'id, ModelCaps>,
            aggregate: AmpduRate<'p, 'id, ModelCaps>,
        ) {
            let mut fixture = fixture(single, aggregate);
            fixture.port.set_length_limit(Some(59));
            let [first, _] = fixture.port.arenas().unwrap();
            for tag in [1, 2] {
                fixture
                    .port
                    .intake(&first, memory(), |region| write_tagged(tag, region))
                    .unwrap();
            }
            let publication = fixture.aggregate(first);
            let Err(PublishError::Refused {
                publication,
                reason: PublishRefused::Length,
            }) = fixture.port.publish(publication, Instant::from_micros(1))
            else {
                panic!("two 30-octet MPDUs exceed 59 octets");
            };
            // A single MPDU carries position 0 alone; the limit is an A-MPDU's.
            let first = publication.arena;
            let publication = fixture.single(first);
            fixture
                .port
                .publish(publication, Instant::from_micros(2))
                .unwrap();
            assert_eq!(fixture.published().unwrap().tags(), [1]);
            fixture.port.answer(ModelArenaAnswer::NoResponse);
            let done = fixture
                .port
                .service((), Instant::from_micros(3))
                .unwrap()
                .unwrap();
            assert_eq!(done.frames(), 2);
            let first = fixture
                .port
                .retain(done, FrameMask::first(2).unwrap(), |_| panic!("kept"))
                .unwrap();
            fixture.port.set_length_limit(None);
            let publication = fixture.aggregate(first);
            fixture
                .port
                .publish(publication, Instant::from_micros(4))
                .unwrap();
            // Only the published frame went out before: only it is a retry.
            let published = fixture.published().unwrap();
            assert_eq!(
                (published.tags(), published.retried.bits()),
                (&[1, 2][..], 0b01)
            );
        }
    }
    with_rates(Case);
}
