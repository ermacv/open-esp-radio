//! The S31 data port's lifecycle over the model device: the station's data
//! arenas from intake to settlement and across a stop and a reinstall, the
//! start and stop bounds, a refused or held retune, a device fault's
//! quarantine and the installation's own resources.
use super::*;
use oer_chips_esp32s31_radio_client::{
    Failure, PhyFault, Poison, PoisonCause, PoisonOrigin, RequestKind,
};
use oer_portable_memory::StableDmaBacking as _;
use oer_portable_time::Duration;
use oer_protocols_ieee80211_contracts_port::arena::{
    AttemptEnd, FrameMask, PublishError, PublishRefused,
};
use oer_protocols_ieee80211_contracts_port::{CommandFailed, StartFailure, SupportedBlockAck};
use oer_protocols_ieee80211_wire::phy::{
    FecCoding, HeGiLtf, HeMcs, HeRate, HtMcs, HtRate, PpduBandwidth, SpatialStreams,
};
use oer_protocols_ieee80211_wire::qos::WmmAccessCategory;

/// An arena's completion as the port's turn hands it out.
type Completed<'id, Id> = oer_protocols_ieee80211_contracts_port::PortCompleted<'id, Port<'id, Id>>;

/// A QoS Data MPDU of 30 octets to the BSS whose sequence number is `tag`.
fn tagged(tag: u8) -> impl FnOnce(&mut [u8]) -> Option<u16> {
    move |region| {
        let length = write_frame(region)?;
        region
            .get_mut(22..24)?
            .copy_from_slice(&(u16::from(tag) << 4).to_le_bytes());
        Some(length)
    }
}

/// [`tagged`] with the Protected bit set, for a CCMP publication.
fn protected(tag: u8) -> impl FnOnce(&mut [u8]) -> Option<u16> {
    move |region| {
        let length = tagged(tag)(region)?;
        *region.get_mut(1)? |= 0x40;
        Some(length)
    }
}

/// The MPDU a returned memory holds.
fn frame_of(memory: &mut TxBacking<512>) -> Vec<u8> {
    memory.stable_dma_region().into_mut_slice()[crate::ordinary_tx::TX_METADATA_SIZE..].to_vec()
}

/// The sequence number a returned memory's MPDU carries, as [`tagged`]
/// wrote it.
fn tag_of(memory: &mut TxBacking<512>) -> u8 {
    let frame = frame_of(memory);
    (u16::from_le_bytes([frame[22], frame[23]]) >> 4) as u8
}

/// Whether a returned memory's MPDU carries the Retry bit.
fn retried(memory: &mut TxBacking<512>) -> bool {
    let frame = frame_of(memory);
    FrameControl::new(u16::from_le_bytes([frame[0], frame[1]])).is_some_and(|c| c.retry())
}

fn memory() -> TxBacking<512> {
    crate::mac::tx::test_support::model_backing().expect("model memory")
}

fn publication<'p, 'id>(arena: Arena, format: ArenaFormat<'p, Caps>) -> ArenaPublication<'p, Caps> {
    ArenaPublication {
        arena,
        format,
        access_category: WmmAccessCategory::BestEffort,
        protection: Protection::None,
        encryption: MpduProtection::Open,
        power: SupportedPower::new(TxPower::Calibrated).expect("power"),
    }
}

/// A plan of `transmissions` at `rate`.
fn plan<R>(rate: R, transmissions: u8) -> RetryPlan<R> {
    RetryPlan::new(RetryStage {
        rate,
        transmissions: core::num::NonZeroU8::new(transmissions).expect("transmissions"),
        protection: Protection::None,
    })
}

fn ht(mcs: u8) -> PhyRate {
    PhyRate::Ht(
        HtRate::new(HtMcs::new(mcs).expect("MCS"), PpduBandwidth::Mhz20, false).expect("HT rate"),
    )
}

fn he() -> PhyRate {
    PhyRate::He(
        HeRate::new(
            HeMcs::new(7).expect("MCS"),
            SpatialStreams::new(1).expect("streams"),
            PpduBandwidth::Mhz20,
            HeGiLtf::Ltf2xGi800Ns,
            FecCoding::Ldpc,
            false,
        )
        .expect("HE rate"),
    )
}

fn spacing() -> MpduStartSpacing {
    MpduStartSpacing::new(0).expect("spacing")
}

/// The memories a settlement returned: each one's tag and Retry bit, in
/// order.
#[derive(Default)]
struct Returned(Vec<(u8, bool)>);
impl Returned {
    fn take(&mut self) -> impl FnMut(TxBacking<512>) + '_ {
        |mut memory| {
            let tag = tag_of(&mut memory);
            let retry = retried(&mut memory);
            self.0.push((tag, retry));
        }
    }
    fn tags(&self) -> Vec<u8> {
        self.0.iter().map(|(tag, _)| *tag).collect()
    }
}

impl<'id, Id: radio::SlotIdentity + 'id> Fixture<'id, Id> {
    fn poll_start_on(
        &mut self,
        wifi: oer_protocols_ieee80211_wire::channel::Ghz2_4Channel,
    ) -> Poll<Result<(), StartRefused<Fault>>> {
        let waker = self.waker();
        let now = self.now();
        self.port
            .poll_start(&mut Context::from_waker(&waker), supported(wifi), now)
    }

    fn poll_stop_now(&mut self) -> Poll<Result<(), StopUnconfirmed<Fault>>> {
        let waker = self.waker();
        let now = self.now();
        self.port.poll_stop(&mut Context::from_waker(&waker), now)
    }

    /// Stop the port, serving a turn and moving time on by `step` between
    /// polls: its answer and the arena completions its turns handed out.
    fn stop_keeping(
        &mut self,
        step: Duration,
    ) -> (Result<(), StopUnconfirmed<Fault>>, Vec<Completed<'id, Id>>) {
        let mut ended = Vec::new();
        for _ in 0..64 {
            match self.poll_stop_now() {
                Poll::Ready(result) => return (result, ended),
                Poll::Pending => {
                    ended.extend(self.turn());
                    self.advance(step);
                }
            }
        }
        panic!("the stop ends");
    }

    /// Serve turns until the published arena ends.
    fn arena_end(&mut self) -> Completed<'id, Id> {
        for _ in 0..16 {
            if let Some(done) = self.turn() {
                return done;
            }
        }
        panic!("the arena ends");
    }

    /// Fill `arena` with one frame per tag.
    fn fill(&mut self, arena: &Arena, tags: &[u8], write: fn(u8) -> Box<WriteFrame>) {
        let mut arenas = self.port.arena();
        for &tag in tags {
            arenas
                .intake(arena, memory(), write(tag))
                .unwrap_or_else(|_| panic!("intake of {tag}"));
        }
    }

    /// The two arenas' rights, once.
    fn arenas(&mut self) -> [Arena; 2] {
        let mut arena = self.port.arena();
        let rights = arena.arenas().expect("the arenas");
        assert!(arena.arenas().is_none(), "the rights are minted once");
        rights
    }

    /// Publish `arena` as one single MPDU of `transmissions` at OFDM 24 Mb/s.
    fn publish_single(&mut self, arena: Arena, transmissions: u8) {
        let now = self.now();
        let mut arenas = self.port.arena();
        let rate = arenas
            .rate(PhyRate::Legacy(LegacyRate::Ofdm24M))
            .expect("a single rate");
        arenas
            .publish(
                publication(arena, ArenaFormat::Single(plan(rate, transmissions))),
                now,
            )
            .unwrap_or_else(|_| panic!("published"));
    }

    /// Hold a single-MPDU arena on the air through its cancel.
    fn cancel_on_the_air(&mut self) -> Arena {
        self.start();
        let [first, second] = self.arenas();
        self.fill(&first, &[10], plain);
        self.hold_air(true);
        self.observed.borrow_mut().taken_by_air = true;
        self.publish_single(first, 3);
        assert!(self.turn().is_none(), "the queue holds the frame");
        let watchdog = self.port.arena().deadline().expect("the watchdog");
        self.advance(watchdog.saturating_duration_since(self.now()));
        assert!(self.turn().is_none(), "the abort settles first");
        self.advance(crate::ordinary_tx::TX_ABORT_SETTLE);
        assert!(self.turn().is_none(), "the PPDU stays with the queue");
        assert_eq!(self.port.fault(), None);
        second
    }

    /// Publish `arena` as an A-MPDU at `rate`, under `encryption`.
    fn publish_aggregate(
        &mut self,
        arena: Arena,
        rate: PhyRate,
        encryption: MpduProtection,
    ) -> Result<(), (Arena, PublishRefused)> {
        let now = self.now();
        let mut arenas = self.port.arena();
        let rate = arenas.rate(rate).expect("a rate on the channel");
        let rate = arenas
            .ampdu_rate(rate)
            .unwrap_or_else(|_| panic!("an aggregate rate"));
        let aggregate = ArenaFormat::Aggregate {
            rate,
            spacing: spacing(),
        };
        match arenas.publish(
            ArenaPublication {
                encryption,
                ..publication(arena, aggregate)
            },
            now,
        ) {
            Ok(()) => Ok(()),
            Err(PublishError::Refused {
                publication,
                reason,
            }) => Err((publication.arena, reason)),
            Err(fault) => panic!("publication fault {fault:?}"),
        }
    }

    /// A station MPDU carrying `body`, admitted by the port.
    fn mpdu_with_body(
        &mut self,
        body: Payload,
    ) -> Option<<Port<'id, Id> as DataPort<'id>>::Admitted> {
        let buffer = self.buffers.pop()?;
        let ticket = self.tickets.pop()?;
        let rate = self.port.rate(PhyRate::Legacy(LegacyRate::Ofdm6M))?;
        let header = HeaderSpec::new(
            FrameControl::new(oer_protocols_ieee80211_wire::frame::DATA_TO_DS).expect("control"),
            0,
            IndividualAddress::new(BSSID).expect("receiver"),
            STATION,
            BSSID,
            0x4560,
            None,
            None,
            None,
        )
        .expect("header");
        let frame = ClassFrame::new(TxFrame::Individual {
            frame: SizedFrame::new(
                AckHeader::new(header).expect("ACK"),
                MpduLength::new(64).expect("length"),
            )
            .expect("frame"),
            plan: plan(rate, 1),
        })
        .ok()?;
        let source = TxSource::Fresh {
            buffer,
            frame,
            body: Some(body),
            protection: TxProtection::Open,
        }
        .check()
        .ok()?;
        let source =
            <Family<Id> as TxBufferPool<'id, BufferClass<512>>>::prepare::<Station, Caps>(source)
                .ok()?;
        let attempt = Attempt {
            context: Tag(92, None),
            ticket,
            source,
            access_category: WmmAccessCategory::BestEffort,
            protection: Protection::None,
            power: SupportedPower::new(TxPower::Calibrated).expect("power"),
            coex_operation: TxOperation::Data,
        }
        .admit();
        Some(self.port.admit(attempt))
    }

    fn submit_now(&mut self, attempt: <Port<'id, Id> as DataPort<'id>>::Admitted) {
        let now = self.now();
        self.port
            .submit(attempt, now, &mut self.outbox)
            .unwrap()
            .expect("submitted");
    }

    /// Close the gate and take it back from its terminal.
    fn close_gate(&mut self) -> GateClosed<'id, radio::Child<radio::Root<Id>, 1>> {
        let gate = self.gate.take().expect("gate");
        assert!(matches!(
            self.run(PortCommand::CloseGate(gate)),
            Ok(CommandProgress::Done)
        ));
        let at = self
            .outbox
            .terminals
            .iter()
            .position(|terminal| matches!(terminal, LowerMacTerminal::GateClosed(_)))
            .expect("the gate closes");
        match self.outbox.terminals.remove(at) {
            LowerMacTerminal::GateClosed(gate) => gate,
            _ => panic!("the gate's terminal"),
        }
    }
}

/// An arena frame's writer.
type WriteFrame = dyn FnOnce(&mut [u8]) -> Option<u16>;

fn plain(tag: u8) -> Box<WriteFrame> {
    Box::new(tagged(tag))
}

fn encrypted(tag: u8) -> Box<WriteFrame> {
    Box::new(protected(tag))
}

/// Retire the stopped fixture's port and install its next installation:
/// the fixture's transmit buffers and terminals go home first. A refusal
/// names its reason and the port's fault.
pub(super) fn reinstall<'id, Id: radio::SlotIdentity + 'id>(
    fixture: Fixture<'id, Id>,
    installations: &mut Installations<Id, InstallationPools>,
) -> Result<Fixture<'id, Id>, (ReinstallRefused, Option<Fault>)> {
    let Fixture {
        mut port,
        outbox,
        observed,
        timer,
        buffers,
        tickets,
        ..
    } = fixture;
    drop((outbox, buffers, tickets));
    match port.reinstall(installations, 18) {
        Ok((commands, resources)) => Ok(Fixture::new(
            PortInstallation {
                port,
                commands,
                resources,
            },
            observed,
            timer,
        )),
        Err(refused) => Err((refused, port.fault())),
    }
}

#[test]
fn an_aggregate_arena_ends_as_the_turns_completion_settles_and_the_stopped_port_reinstalls() {
    with_installations(|mut fixture, installations| {
        fixture.start();
        fixture.observed.borrow_mut().block_ack = Some((10, 0b111, true));
        let [first, _second] = fixture.arenas();
        fixture.fill(&first, &[10, 11, 12], plain);
        fixture
            .publish_aggregate(first, ht(7), MpduProtection::Open)
            .expect("published");
        let done = fixture.arena_end();
        assert!(matches!(done.end(), AttemptEnd::Answered { .. }));
        let mut returned = Returned::default();
        let _first = fixture
            .port
            .arena()
            .release(done, returned.take())
            .expect("released");
        assert_eq!(returned.tags(), [10, 11, 12], "every memory back in order");
        assert_eq!(fixture.observed.borrow().publications, 1, "one PPDU");
        fixture.stop();
        assert!(
            reinstall(fixture, installations).is_ok(),
            "idle, empty arenas give their chains back"
        );
    });
}

/// Publish `tags` as an aggregate of the arena `arena`, answered whole, and
/// settle it.
fn aggregate_round<'id>(
    fixture: &mut Fixture<'id, radio::LocalId<'id>>,
    arena: Arena,
    tags: &[u8],
) -> Arena {
    fixture.observed.borrow_mut().block_ack = Some((10, 0b111, true));
    fixture.fill(&arena, tags, plain);
    fixture
        .publish_aggregate(arena, ht(7), MpduProtection::Open)
        .expect("published");
    let done = fixture.arena_end();
    let mut returned = Returned::default();
    fixture
        .port
        .arena()
        .release(done, returned.take())
        .expect("released")
}

#[test]
fn each_aggregate_of_the_same_values_draws_its_own_backoff() {
    with_fixture(|fixture| {
        fixture.start();
        let [first, second] = fixture.arenas();
        let _first = aggregate_round(fixture, first, &[10, 11, 12]);
        let _second = aggregate_round(fixture, second, &[13, 14, 15]);
        let observed = fixture.observed.borrow();
        let draws = observed.draws.borrow();
        assert_eq!(draws.len(), 2, "one draw per publication");
        assert_eq!(
            observed.ht_contention.last().map(|(backoff, _)| *backoff),
            draws.last().map(|draw| draw.backoff_slots),
            "the second program carries the second draw"
        );
    });
}

#[test]
fn an_edca_change_reaches_the_aggregate_after_it() {
    use oer_protocols_ieee80211_testing_conformance::edca::parameters;
    with_fixture(|fixture| {
        fixture.start();
        let [first, second] = fixture.arenas();
        let _first = aggregate_round(fixture, first, &[10, 11, 12]);
        let table = parameters(VifRole::AccessPoint, false);
        let edca = SupportedEdca::new(table).expect("EDCA");
        assert!(matches!(
            fixture.run(PortCommand::SetEdca(edca)),
            Ok(CommandProgress::Done)
        ));
        let _second = aggregate_round(fixture, second, &[13, 14, 15]);
        let wanted = table.access_category(WmmAccessCategory::BestEffort);
        assert_eq!(
            fixture
                .observed
                .borrow()
                .ht_contention
                .last()
                .map(|(_, aifsn)| *aifsn),
            Some(wanted.wmm.aifsn.get()),
            "the program follows the table the command gave"
        );
    });
}

#[test]
fn a_closed_gate_refuses_an_arena_and_its_frames_are_withdrawn() {
    with_installations(|mut fixture, installations| {
        fixture.start();
        let [first, _second] = fixture.arenas();
        fixture.fill(&first, &[10, 11, 12], plain);
        let gate = fixture.close_gate();
        let (first, reason) = fixture
            .publish_aggregate(first, ht(7), MpduProtection::Open)
            .expect_err("a closed gate publishes nothing");
        assert_eq!(reason, PublishRefused::Busy);
        let mut returned = Returned::default();
        fixture
            .port
            .arena()
            .withdraw(&first, returned.take())
            .expect("withdrawn");
        assert_eq!(returned.tags(), [10, 11, 12]);
        assert_eq!(fixture.observed.borrow().publications, 0);
        fixture.gate = Some(fixture.port.open_gate(gate));
        fixture.stop();
        assert!(reinstall(fixture, installations).is_ok());
    });
}

#[test]
fn a_single_mpdu_waits_while_an_arena_holds_its_access_category() {
    with_fixture(|fixture| {
        fixture.start();
        let [first, _second] = fixture.arenas();
        fixture.fill(&first, &[10], plain);
        fixture.hold_air(true);
        fixture.publish_single(first, 1);
        let attempt = fixture.station_mpdu().expect("attempt");
        fixture.submit_now(attempt);
        for _ in 0..8 {
            assert!(fixture.turn().is_none());
        }
        assert_eq!(
            fixture.observed.borrow().publications,
            1,
            "the MPDU waits behind the arena on the air"
        );
        fixture.hold_air(false);
        let mut order = Vec::new();
        let mut ended = None;
        for _ in 0..16 {
            if let Some(done) = fixture.turn() {
                order.push("arena");
                ended = Some(done);
            }
            if fixture.take_completed() != 0 {
                order.push("single");
            }
            if order.len() == 2 {
                break;
            }
        }
        assert_eq!(order, ["arena", "single"]);
        assert_eq!(fixture.observed.borrow().publications, 2);
        let done = ended.expect("the arena ended");
        assert!(matches!(done.end(), AttemptEnd::Acked { .. }));
        let mut returned = Returned::default();
        let _first = fixture
            .port
            .arena()
            .release(done, returned.take())
            .expect("released");
        assert_eq!(returned.tags(), [10]);
    });
}

#[test]
fn a_single_arena_takes_each_status_history_to_its_frames_retry_bit() {
    for (statuses, retry) in RETRY_HISTORIES {
        with_fixture(|fixture| {
            fixture.start();
            fixture.observed.borrow_mut().statuses = statuses.to_vec();
            let [first, _second] = fixture.arenas();
            fixture.fill(&first, &[10, 11], plain);
            fixture.publish_single(first, 3);
            let done = fixture.arena_end();
            assert!(
                matches!(done.end(), AttemptEnd::Acked { .. }),
                "{statuses:?}"
            );
            assert_eq!(
                fixture.observed.borrow().publications,
                statuses.len(),
                "{statuses:?}"
            );
            let mut returned = Returned::default();
            let _first = fixture
                .port
                .arena()
                .release(done, returned.take())
                .expect("released");
            assert_eq!(
                returned.0,
                [(10, retry), (11, false)],
                "only the frame the plan sent changes: {statuses:?}"
            );
        });
    }
}

#[test]
fn a_retained_single_arena_keeps_its_retry_history_into_its_next_publication() {
    // A plan that ran out leaves the frame in the arena, and its settlement
    // decides for the next publication, which then delivers it.
    let histories: [(&[(u8, u8)], bool); 6] = [
        (&[ACK_TIMEOUT], true),
        (&[COLLISION], false),
        (&[CTS_TIMEOUT], false),
        (&[COLLISION, ACK_TIMEOUT], true),
        (&[ACK_TIMEOUT, COLLISION], true),
        (&[ACK_TIMEOUT, CTS_TIMEOUT], true),
    ];
    for (statuses, retry) in histories {
        with_fixture(|fixture| {
            fixture.start();
            let mut all = statuses.to_vec();
            all.push(SUCCESS);
            fixture.observed.borrow_mut().statuses = all;
            let [first, _second] = fixture.arenas();
            fixture.fill(&first, &[10], plain);
            let transmissions = u8::try_from(statuses.len()).expect("a short plan");
            fixture.publish_single(first, transmissions);
            let done = fixture.arena_end();
            let mask = FrameMask::from_bits(0b1).expect("mask");
            let first = fixture
                .port
                .arena()
                .retain(done, mask, drop)
                .unwrap_or_else(|_| panic!("retained"));
            fixture.publish_single(first, 1);
            let done = fixture.arena_end();
            assert!(
                matches!(done.end(), AttemptEnd::Acked { .. }),
                "{statuses:?}"
            );
            let mut returned = Returned::default();
            let _first = fixture
                .port
                .arena()
                .release(done, returned.take())
                .expect("released");
            assert_eq!(returned.0, [(10, retry)], "{statuses:?}");
        });
    }
}

#[test]
fn a_single_arena_runs_its_retry_plan_until_an_ack_and_returns_its_frame_retried() {
    with_fixture(|fixture| {
        fixture.start();
        // AckTimeout, then success.
        fixture.observed.borrow_mut().statuses = std::vec![(5, 0), (0, 0)];
        let [first, _second] = fixture.arenas();
        fixture.fill(&first, &[10], plain);
        fixture.publish_single(first, 3);
        let done = fixture.arena_end();
        assert!(matches!(done.end(), AttemptEnd::Acked { .. }));
        assert_eq!(
            done.air().map(|air| air.transmissions.get()),
            Some(2),
            "both transmissions reached the air"
        );
        assert_eq!(
            fixture.observed.borrow().publications,
            2,
            "the plan sent it again"
        );
        let mut returned = Returned::default();
        let _first = fixture
            .port
            .arena()
            .release(done, returned.take())
            .expect("released");
        assert_eq!(returned.0, [(10, true)], "the retransmission carried Retry");
    });
}

#[test]
fn a_stop_ends_a_published_arena_through_its_timeout_abort_and_hands_its_end_to_the_caller() {
    with_installations(|mut fixture, installations| {
        fixture.start();
        let [first, _second] = fixture.arenas();
        fixture.fill(&first, &[10], plain);
        // No completion comes: the hardware's TX-timeout edge starts the
        // abort that ends the arena.
        fixture.observed.borrow_mut().timeout_edge = true;
        fixture.publish_single(first, 1);
        let (stopped, mut ended) = fixture.stop_keeping(crate::ordinary_tx::TX_ABORT_SETTLE);
        assert_eq!(
            stopped,
            Ok(()),
            "the stop confirms once the arena is off the air"
        );
        assert_eq!(fixture.observed.borrow().publications, 1);
        assert_eq!(ended.len(), 1, "the arena's end reached the caller");
        let done = ended.pop().expect("the arena ended");
        assert!(matches!(done.end(), AttemptEnd::Aborted));
        let mut returned = Returned::default();
        let _first = fixture
            .port
            .arena()
            .release(done, returned.take())
            .expect("a stopped port settles its arena");
        assert_eq!(returned.tags(), [10]);
        assert!(reinstall(fixture, installations).is_ok());
    });
}

#[test]
fn a_publication_refused_before_it_took_anything_restarts_into_a_working_port() {
    with_installations(|mut fixture, installations| {
        fixture.start();
        let (association, _, slot) = fixture.association();
        let (install, _) = fixture.commands.install_pairwise(
            &association,
            slot,
            IndividualAddress::new(BSSID).expect("peer"),
            group_key(3),
        );
        assert!(matches!(fixture.run(install), Ok(CommandProgress::Done)));
        let key = match fixture.outbox.terminals.pop() {
            Some(LowerMacTerminal::PairwiseKeyInstalled(InstalledPairwise::Station(key))) => key,
            _ => panic!("the installation terminal returns the key"),
        };
        let attempt = fixture.protected_mpdu(&key).expect("attempt");
        drop(key);
        fixture.port.uninstall_keys_for_test();
        let now = fixture.now();
        let _ = fixture
            .port
            .submit(attempt, now, &mut fixture.outbox)
            .unwrap();
        assert_eq!(
            fixture.port.fault(),
            Some(Fault::Unpublished(crate::port::Unpublished::Key))
        );

        assert!(fixture.port.recover_for_stop());
        // The key is still missing: the next service withdraws the MPDU
        // instead of publishing it, so the refusal cannot recur.
        let _ = fixture.turn();
        assert_eq!(fixture.port.fault(), None);
        assert_eq!(fixture.take_completed(), 1, "the MPDU came back withdrawn");
        let attempt = fixture.mpdu(None).expect("attempt");
        let now = fixture.now();
        assert_eq!(
            fixture
                .port
                .submit(attempt, now, &mut fixture.outbox)
                .unwrap()
                .err(),
            Some(oer_protocols_ieee80211_contracts_port::SubmitRefusal::Stopping)
        );
        assert_eq!(fixture.take_completed(), 1, "a new one is refused back");
        // The stop's bound starts with the stop, however late it comes.
        fixture.advance(crate::port::COMMAND_BOUND);
        fixture.advance(crate::port::COMMAND_BOUND);
        fixture.stop();
        assert_eq!(fixture.port.fault(), None);

        let mut second = reinstall(fixture, installations)
            .ok()
            .expect("every owner came home");
        second.start();
        let attempt = second.mpdu(None).expect("attempt");
        let now = second.now();
        second
            .port
            .submit(attempt, now, &mut second.outbox)
            .unwrap()
            .ok()
            .expect("the new installation publishes");
        let _ = second.turn();
        assert_eq!(second.take_completed(), 1, "and its frame completes");
        assert_eq!(second.port.fault(), None);
        second.stop();
    });
}

#[test]
fn a_publication_refused_past_its_attachment_ends_withdrawn_with_every_owner_and_no_fault() {
    with_installations(|mut fixture, installations| {
        fixture.start();
        let (association, _, slot) = fixture.association();
        let (install, _) = fixture.commands.install_pairwise(
            &association,
            slot,
            IndividualAddress::new(BSSID).expect("peer"),
            group_key(3),
        );
        assert!(matches!(fixture.run(install), Ok(CommandProgress::Done)));
        let key = match fixture.outbox.terminals.pop() {
            Some(LowerMacTerminal::PairwiseKeyInstalled(InstalledPairwise::Station(key))) => key,
            _ => panic!("the installation terminal returns the key"),
        };
        let attempt = fixture.protected_mpdu(&key).expect("attempt");
        drop(key);
        let before = fixture.port.admissions_for_test();
        assert_eq!(before.1, Some(0));
        fixture.port.refuse_values_for_test();
        let now = fixture.now();
        assert!(
            fixture
                .port
                .submit(attempt, now, &mut fixture.outbox)
                .unwrap()
                .is_ok()
        );
        assert_eq!(fixture.port.fault(), None, "an own refusal is no fault");
        assert_eq!(
            fixture.port.admissions_for_test(),
            before,
            "the ticket and the key's use came back"
        );
        assert!(matches!(
            fixture.outbox.terminals.last(),
            Some(LowerMacTerminal::TxCompleted(TxCompleted::Single(completed)))
                if matches!(
                    completed.outcome(),
                    TxOutcome::Withdrawn(WithdrawCause::Unpublishable)
                )
        ));
        assert_eq!(fixture.take_completed(), 1, "the MPDU came back withdrawn");
        fixture.stop();

        // The retire refuses a descriptor that is missing or still holds
        // its buffer.
        let mut second = reinstall(fixture, installations)
            .ok()
            .expect("the descriptor and the buffer came home");
        second.start();
        let attempt = second.mpdu(None).expect("attempt");
        let now = second.now();
        second
            .port
            .submit(attempt, now, &mut second.outbox)
            .unwrap()
            .ok()
            .expect("the new installation publishes");
        let _ = second.turn();
        assert_eq!(second.take_completed(), 1, "and its frame completes");
        assert_eq!(second.port.fault(), None);
        second.stop();
    });
}

#[test]
fn a_frame_left_in_an_arena_refuses_the_reinstall_as_not_retired_until_withdrawn() {
    with_installations(|mut fixture, installations| {
        fixture.start();
        let [first, _second] = fixture.arenas();
        fixture.fill(&first, &[10], plain);
        fixture.stop();
        assert_eq!(fixture.observed.borrow().publications, 0);
        // The retire checks see the frame: a handled refusal, no fault.
        assert_eq!(
            fixture.port.reinstall(installations, 18).err(),
            Some(ReinstallRefused::NotRetired)
        );
        assert_eq!(fixture.port.fault(), None);
        assert!(
            fixture.port.release().is_none(),
            "the release runs the same checks"
        );
        assert!(!fixture.observed.borrow().detached);
        // Once the caller takes its frame back, the chains go home.
        let mut returned = Returned::default();
        fixture
            .port
            .arena()
            .withdraw(&first, returned.take())
            .expect("a stopped port withdraws its unpublished arena");
        assert_eq!(returned.tags(), [10]);
        assert!(reinstall(fixture, installations).is_ok());
    });
}

#[test]
fn a_retained_aggregate_goes_again_with_the_retry_bit_and_only_its_missing_frames() {
    with_fixture(|fixture| {
        fixture.start();
        let [first, _second] = fixture.arenas();
        fixture.fill(&first, &[10, 11, 12], plain);
        fixture
            .publish_aggregate(first, ht(0), MpduProtection::Open)
            .expect("published");
        // The BlockAck acknowledges the middle frame alone.
        fixture.observed.borrow_mut().block_ack = Some((10, 0b010, true));
        let done = fixture.arena_end();
        assert!(matches!(done.end(), AttemptEnd::Answered { .. }));
        let mut returned = Returned::default();
        let first = fixture
            .port
            .arena()
            .retain(
                done,
                FrameMask::from_bits(0b101).expect("mask"),
                returned.take(),
            )
            .unwrap_or_else(|_| panic!("retained"));
        assert_eq!(returned.tags(), [11]);
        assert_eq!(fixture.port.arena().len(first.id()), 2);
        fixture
            .publish_aggregate(first, ht(0), MpduProtection::Open)
            .expect("republished");
        fixture.observed.borrow_mut().block_ack = Some((10, 0b101, true));
        let done = fixture.arena_end();
        assert_eq!(done.frames(), 2);
        let mut returned = Returned::default();
        let _first = fixture
            .port
            .arena()
            .release(done, returned.take())
            .expect("released");
        assert_eq!(
            returned.0,
            [(10, true), (12, true)],
            "the kept frames went again as retries, in order"
        );
        assert_eq!(fixture.observed.borrow().publications, 2);
    });
}

#[test]
fn a_pairwise_removal_waits_until_the_last_arena_frame_its_key_encrypted_leaves() {
    with_fixture(|fixture| {
        fixture.start();
        let (association, _, slot) = fixture.association();
        let (install, _) = fixture.commands.install_pairwise(
            &association,
            slot,
            IndividualAddress::new(BSSID).expect("peer"),
            group_key(7),
        );
        assert!(matches!(fixture.run(install), Ok(CommandProgress::Done)));
        let key = match fixture.outbox.terminals.pop() {
            Some(LowerMacTerminal::PairwiseKeyInstalled(InstalledPairwise::Station(key))) => key,
            _ => panic!("the installation terminal returns the key"),
        };
        let [first, _second] = fixture.arenas();
        fixture.fill(&first, &[20, 21], encrypted);
        // An open publication of protected frames is refused.
        let (first, reason) = fixture
            .publish_aggregate(first, ht(0), MpduProtection::Open)
            .expect_err("the encryption contradicts the frames");
        assert_eq!(reason, PublishRefused::Protection);
        fixture
            .publish_aggregate(first, ht(0), MpduProtection::Ccmp)
            .expect("published");
        fixture.observed.borrow_mut().block_ack = Some((20, 0b01, true));
        let done = fixture.arena_end();
        let first = fixture
            .port
            .arena()
            .retain(done, FrameMask::from_bits(0b10).expect("mask"), drop)
            .unwrap_or_else(|_| panic!("retained"));
        // The retained frame keeps the key: its removal is held.
        let now = fixture.now();
        let removal = match fixture
            .port
            .command(
                PortCommand::RemovePairwise { association, key },
                now,
                &mut fixture.outbox,
            )
            .unwrap()
        {
            Ok(CommandProgress::Held(removal)) => removal,
            _ => panic!("the removal waits for the arena"),
        };
        fixture
            .publish_aggregate(first, ht(0), MpduProtection::Ccmp)
            .expect("the retained frame goes under its key");
        fixture.observed.borrow_mut().block_ack = Some((21, 0b1, true));
        let done = fixture.arena_end();
        let mut returned = Returned::default();
        let first = fixture
            .port
            .arena()
            .release(done, returned.take())
            .expect("released");
        assert_eq!(returned.tags(), [21]);
        fixture
            .port
            .arena()
            .withdraw(&first, drop)
            .expect("the arena is empty");
        // The last use ended: the removal runs.
        assert!(matches!(fixture.run(removal), Ok(CommandProgress::Done)));
        assert!(matches!(
            fixture.outbox.terminals.pop(),
            Some(LowerMacTerminal::KeyRemoved(Removed::Station(
                RemovedKey::Pairwise { .. }
            )))
        ));
    });
}

#[test]
fn an_aggregate_past_its_rate_limit_is_refused_and_goes_as_a_single_of_position_0() {
    with_fixture(|fixture| {
        fixture.start();
        let [first, _second] = fixture.arenas();
        // Sixteen MPDUs of 480 octets pass the HT MCS 0 ceiling.
        let tags: Vec<u8> = (30..46).collect();
        fixture.fill(&first, &tags, |tag| -> Box<WriteFrame> {
            Box::new(move |region: &mut [u8]| {
                tagged(tag)(region)?;
                Some(480)
            })
        });
        let (first, reason) = fixture
            .publish_aggregate(first, ht(0), MpduProtection::Open)
            .expect_err("the port holds the rate's octet limit");
        assert_eq!(reason, PublishRefused::Length);
        assert_eq!(fixture.observed.borrow().publications, 0);
        fixture.publish_single(first, 1);
        let done = fixture.arena_end();
        assert_eq!(fixture.observed.borrow().publications, 1, "one program");
        assert_eq!(done.frames(), 16);
        assert!(matches!(done.end(), AttemptEnd::Acked { .. }));
        let mut returned = Returned::default();
        let _first = fixture
            .port
            .arena()
            .release(done, returned.take())
            .expect("released");
        assert_eq!(returned.tags(), tags);
    });
}

#[test]
fn an_he_rate_publishes_an_he_aggregate_program_and_its_block_ack_ends_it() {
    with_installations(|mut fixture, _| {
        fixture.start();
        let now = fixture.now();
        assert!(matches!(
            fixture
                .port
                .command(
                    PortCommand::SetBssColor(BssColor::new(0x2a).expect("color")),
                    now,
                    &mut fixture.outbox,
                )
                .unwrap(),
            Ok(CommandProgress::Done)
        ));
        fixture.turn();
        let [first, _second] = fixture.arenas();
        fixture.fill(&first, &[40, 41, 42], plain);
        fixture
            .publish_aggregate(first, he(), MpduProtection::Open)
            .expect("published");
        fixture.observed.borrow_mut().block_ack = Some((40, 0b111, true));
        let done = fixture.arena_end();
        assert!(matches!(done.end(), AttemptEnd::Answered { .. }));
        let mut returned = Returned::default();
        let _first = fixture
            .port
            .arena()
            .release(done, returned.take())
            .expect("released");
        assert_eq!(returned.tags(), [40, 41, 42]);
        fixture.stop();
        let Fixture {
            mut port, buffers, ..
        } = fixture;
        drop(buffers);
        let device = port
            .release()
            .expect("the stopped port releases its device");
        assert_eq!(device.hardware.he.len(), 1, "one HE SU program");
        assert!(device.hardware.ht.is_empty());
    });
}

/// The port's MAC after `settings`, each a `SetHe` command it ran.
fn he_after(
    settings: &[oer_protocols_ieee80211_contracts_port::StationHe],
) -> Option<oer_chips_esp32s31_hal::types::MacHe20Association> {
    let mut held = None;
    with_installations(|mut fixture, _| {
        fixture.start();
        for &he in settings {
            let now = fixture.now();
            assert!(matches!(
                fixture
                    .port
                    .command(PortCommand::SetHe(he), now, &mut fixture.outbox)
                    .unwrap(),
                Ok(CommandProgress::Done)
            ));
            fixture.turn();
        }
        fixture.stop();
        let Fixture {
            mut port, buffers, ..
        } = fixture;
        drop(buffers);
        let device = port
            .release()
            .expect("the stopped port releases its device");
        held = Some(device.hardware.he_association);
    });
    held.expect("the installation ran")
}

#[test]
fn the_port_installs_an_he_connection_and_association_and_ends_them() {
    use oer_protocols_ieee80211_contracts_port::{He20Association, HeBssColor, StationHe};
    let peer = oer_protocols_ieee80211_wire::he::He20PeerState {
        capability_prefix: [0; 24],
        packet_padding_eight_us: 1,
        default_packet_extension_duration: 2,
        bss_color: 9,
        bss_color_enabled: true,
        partial_bss_color: true,
        basic_mcs_nss_map: 0xfffc,
        rts_threshold: Some(100),
        max_co_hosted_bssid_indicator: Some(2),
        extended_range_single_user_disabled: false,
    };
    let bss = StationHe::Bss(
        HeBssColor::of(&oer_protocols_ieee80211_wire::he::He20Operation {
            bss_color: 9,
            bss_color_enabled: true,
            partial_bss_color: true,
            basic_mcs_nss_map: 0xfffc,
        })
        .expect("a color"),
    );
    let association = StationHe::He20(He20Association::new(&peer, 7, 0).expect("in range"));
    let installed = he_after(&[bss, association]).expect("installed");
    assert_eq!(installed.association_id.get(), 7);
    assert_eq!(installed.peer.packet_padding_duration.get(), 8);
    assert_eq!(installed.peer.max_co_hosted_bssid_indicator, Some(2));
    // HE, then its end, then HE again: each command's state, in order.
    assert_eq!(he_after(&[bss, association, StationHe::Off]), None);
    assert_eq!(
        he_after(&[bss, association, StationHe::Off, bss, association]),
        Some(installed)
    );
}

#[test]
fn a_completion_without_effect_keeps_the_arena_watchdog_whose_expiry_cancels_the_arena() {
    with_fixture(|fixture| {
        fixture.start();
        let [first, _second] = fixture.arenas();
        fixture.fill(&first, &[50, 51], plain);
        // A diagnostic-only completion: it ends nothing.
        fixture.observed.borrow_mut().statuses = std::vec![(1, 0)];
        fixture.observed.borrow_mut().block_ack = Some((0, 0, false));
        fixture
            .publish_aggregate(first, ht(0), MpduProtection::Open)
            .expect("published");
        let watchdog = fixture.port.arena().deadline().expect("the watchdog");
        assert!(fixture.turn().is_none(), "the completion ends nothing");
        assert_eq!(fixture.observed.borrow().publications, 1);
        assert_eq!(
            fixture.port.arena().deadline(),
            Some(watchdog),
            "the watchdog is not postponed"
        );
        assert!(fixture.port.deadline().is_some_and(|at| at <= watchdog));
        fixture.advance(watchdog.saturating_duration_since(fixture.now()));
        assert!(fixture.turn().is_none(), "the abort settles first");
        fixture.advance(crate::ordinary_tx::TX_ABORT_SETTLE);
        let done = fixture.turn().expect("the cancel ends the arena");
        assert!(matches!(done.end(), AttemptEnd::Aborted));
        assert_eq!(fixture.port.fault(), None);
        let mut returned = Returned::default();
        let _first = fixture
            .port
            .arena()
            .release(done, returned.take())
            .expect("released");
        assert_eq!(returned.tags(), [50, 51]);
    });
}

#[test]
fn an_arena_the_cancel_finds_on_the_air_ends_by_its_completion() {
    with_fixture(|fixture| {
        let _second = fixture.cancel_on_the_air();
        fixture.hold_air(false);
        let done = fixture.arena_end();
        assert!(matches!(done.end(), AttemptEnd::Acked { .. }));
        let mut returned = Returned::default();
        let _first = fixture
            .port
            .arena()
            .release(done, returned.take())
            .expect("released");
        assert_eq!(returned.tags(), [10]);
    });
}

#[test]
fn an_arena_past_its_cancel_on_the_air_is_a_named_fault() {
    with_fixture(|fixture| {
        let _second = fixture.cancel_on_the_air();
        let watchdog = fixture.port.arena().deadline().expect("the last watchdog");
        fixture.advance(watchdog.saturating_duration_since(fixture.now()));
        let now = fixture.now();
        let Err(fault) = fixture
            .port
            .service_tx(TxFacts::DUE, false, now, &mut fixture.outbox)
            .unwrap()
        else {
            panic!("the last watchdog quarantines");
        };
        assert_eq!(
            fault,
            Fault::Arena(crate::lower_mac::arena::ArenaFault::Watchdog)
        );
        assert_eq!(fixture.port.fault(), Some(fault));
    });
}

#[test]
fn a_retransmission_after_a_cancel_on_the_air_has_a_cancel_of_its_own() {
    with_fixture(|fixture| {
        let _second = fixture.cancel_on_the_air();
        // The PPDU ends unanswered: the plan sends it again, and this queue
        // lets go when its watchdog cancels it.
        fixture.observed.borrow_mut().statuses = std::vec![(5, 0)];
        fixture.observed.borrow_mut().taken_by_air = false;
        fixture.hold_air(false);
        assert!(fixture.turn().is_none(), "the plan sends it again");
        fixture.hold_air(true);
        assert!(fixture.turn().is_none(), "the queue holds it again");
        assert_eq!(fixture.observed.borrow().publications, 2);
        let watchdog = fixture.port.arena().deadline().expect("a new watchdog");
        fixture.advance(watchdog.saturating_duration_since(fixture.now()));
        assert!(fixture.turn().is_none(), "the abort settles first");
        fixture.advance(crate::ordinary_tx::TX_ABORT_SETTLE);
        let done = fixture.turn().expect("the cancel ends the arena");
        assert!(matches!(done.end(), AttemptEnd::Aborted));
        let mut returned = Returned::default();
        let _first = fixture
            .port
            .arena()
            .release(done, returned.take())
            .expect("released");
        assert_eq!(returned.tags(), [10]);
    });
}

#[test]
fn a_device_fault_is_named_and_quarantines_the_mpdus_and_the_ring_and_the_stop_is_unconfirmed() {
    with_installations(|mut fixture, installations| {
        fixture.start();
        fixture.configure_station();
        // The first MPDU's air never answers; the second queues behind it.
        fixture.observed.borrow_mut().held_queues = [true; 4];
        let dropped = Rc::new(Cell::new(0));
        for _ in 0..2 {
            let attempt = fixture
                .mpdu_with_body(Payload(dropped.clone()))
                .expect("attempt");
            fixture.submit_now(attempt);
        }
        fixture.turn();
        assert_eq!(fixture.observed.borrow().publications, 1);
        let frame = fixture.frame();
        fixture.observed.borrow_mut().received.push_back(frame);
        fixture.observed.borrow_mut().fail_poll = Some(Fault::RemovalMismatch);
        let waker = fixture.waker();
        let facts = fixture.port.poll_facts(&mut Context::from_waker(&waker));
        assert!(
            facts.tx.is_some(),
            "the next service answers with the fault"
        );
        assert_eq!(
            fixture.port.fault(),
            Some(Fault::RemovalMismatch),
            "the fault by name"
        );
        let now = fixture.now();
        assert!(matches!(
            fixture
                .port
                .service_tx(TxFacts::DUE, false, now, &mut fixture.outbox)
                .unwrap(),
            Err(Fault::RemovalMismatch)
        ));
        assert!(matches!(
            fixture.port.receive(8, now),
            Err(Fault::RemovalMismatch)
        ));
        // The quarantine keeps every owner: no MPDU ended, no body dropped,
        // the frame stays on the ring.
        assert!(fixture.outbox.terminals.is_empty());
        assert_eq!(dropped.get(), 0);
        assert_eq!(fixture.observed.borrow().received.len(), 1);
        // The stop answers at once, unconfirmed, with what the port holds.
        let Poll::Ready(Err(unconfirmed)) = fixture.poll_stop_now() else {
            panic!("a faulted port cannot confirm its stop");
        };
        assert_eq!(
            unconfirmed.cause,
            StopCause::Device(Fault::RemovalMismatch),
            "the stop names the fault it met"
        );
        assert_eq!(unconfirmed.observations.published, 1);
        assert_eq!(unconfirmed.observations.queued, 1);
        assert_eq!(fixture.now(), now, "no time passed for the answer");
        assert_eq!(
            fixture.observed.borrow().stops,
            0,
            "the device was not stopped"
        );
        // Nothing the quarantine holds is issued again.
        assert_eq!(
            fixture.port.reinstall(installations, 18).err(),
            Some(ReinstallRefused::NotRetired)
        );
        assert!(fixture.port.release().is_none());
        assert!(!fixture.observed.borrow().detached);
        assert_eq!(dropped.get(), 0);
    });
}

#[test]
fn a_stop_the_device_holds_wakes_at_the_devices_own_settle() {
    with_installations(|mut fixture, _| {
        fixture.start();
        fixture.observed.borrow_mut().hold_stop = true;
        assert!(fixture.poll_stop_now().is_pending());
        let _ = fixture.turn();
        // The device's pause asks to look again soon, as a receive slot
        // still claimed or a MAC still stopping does.
        let settle = fixture.now().saturating_add(Duration::from_micros(100));
        fixture.observed.borrow_mut().rx_continuation = Some(settle);
        assert!(fixture.poll_stop_now().is_pending());
        assert_eq!(
            fixture.port.deadline(),
            Some(settle),
            "the port sleeps until the device looks again, not to its bound"
        );
    });
}

/// Poll the stop the first call asked for, serving a turn between polls
/// and never moving time: the stop's answers in order, up to its first
/// confirmation or `polls` answers.
fn stop_answers<'id, Id: radio::SlotIdentity + 'id>(
    fixture: &mut Fixture<'id, Id>,
    polls: usize,
) -> Vec<Result<(), StopUnconfirmed<Fault>>> {
    let mut answers = Vec::new();
    for _ in 0..64 {
        if let Poll::Ready(answer) = fixture.poll_stop_now() {
            let confirmed = answer.is_ok();
            answers.push(answer);
            if confirmed || answers.len() == polls {
                return answers;
            }
        }
        let _ = fixture.turn();
    }
    panic!("the stop answers");
}

/// A line mask the line's handler has not confirmed within its bound is a
/// stop not confirmed yet, never a fault of the device: the stop's answer
/// is `Deadline`, the port stays in the stop the first call began, and the
/// device's later confirmation completes it at the next poll. No time
/// passes: the device's own answers drive it.
#[test]
fn a_late_line_mask_is_a_stop_not_confirmed_yet_that_its_later_confirmation_completes() {
    with_installations(|mut fixture, _| {
        fixture.start();
        fixture.observed.borrow_mut().fail_stop =
            Some(Fault::Receive(crate::rx::control::WifiRxCause::LineRefused));
        let began = fixture.now();
        let answers = stop_answers(&mut fixture, 2);
        let [Err(late), Ok(())] = answers.as_slice() else {
            panic!("a late mask, then the confirmation: {answers:?}");
        };
        assert_eq!(late.cause, StopCause::Deadline, "not a device fault");
        assert_eq!(fixture.now(), began, "no time passed");
        assert_eq!(fixture.observed.borrow().stops, 1, "the device stopped");
        assert_eq!(fixture.port.fault(), None);
        assert_eq!(
            fixture.poll_stop_now(),
            Poll::Ready(Ok(())),
            "it stays stopped"
        );
    });
}

/// Every other failure of the device's stop is the device's fault, named by
/// that very fault.
#[test]
fn another_failure_of_the_devices_stop_is_named_by_its_own_fault() {
    with_installations(|mut fixture, _| {
        fixture.start();
        let cause = Fault::Receive(crate::rx::control::WifiRxCause::ForeignExecutor);
        fixture.observed.borrow_mut().fail_stop = Some(cause);
        let answers = stop_answers(&mut fixture, 1);
        let [Err(failed)] = answers.as_slice() else {
            panic!("the device's failure: {answers:?}");
        };
        assert_eq!(failed.cause, StopCause::Device(cause));
    });
}

#[test]
fn a_stop_past_its_bound_is_unconfirmed_and_the_port_neither_reinstalls_nor_releases() {
    with_installations(|mut fixture, installations| {
        fixture.start();
        // An MPDU queues behind the closed gate.
        let gate = fixture.close_gate();
        let attempt = fixture.station_mpdu().expect("attempt");
        fixture.submit_now(attempt);
        // The device never confirms its stop.
        fixture.observed.borrow_mut().hold_stop = true;
        let began = fixture.now();
        let (stopped, ended) = fixture.stop_keeping(Duration::from_millis(50));
        assert!(ended.is_empty());
        let Err(unconfirmed) = stopped else {
            panic!("the stop expires");
        };
        assert_eq!(unconfirmed.cause, StopCause::Deadline);
        assert_eq!(unconfirmed.observations, StopObservations::default());
        assert!(fixture.now().saturating_duration_since(began) >= COMMAND_BOUND);
        assert_eq!(
            fixture.take_completed(),
            1,
            "the stop withdrew the queued MPDU to the caller"
        );
        // Nothing comes back from a port whose stop is unconfirmed.
        assert_eq!(
            fixture.port.reinstall(installations, 18).err(),
            Some(ReinstallRefused::NotRetired)
        );
        assert!(fixture.port.release().is_none());
        assert!(!fixture.observed.borrow().detached);
        drop(gate);
    });
}

#[test]
fn an_installation_missing_a_pool_buffer_is_quarantined_and_never_starts() {
    block_on(radio::with_local_slot(async move |claim| {
        let (parts, observed, timer) = installed(Vec::new());
        // Another owner holds one of the pool's buffers.
        let withheld =
            crate::mac::tx::test_support::pool_backing(parts.pool, 3).expect("withheld buffer");
        let mut installations = Installations::<_, InstallationPools>::claim(claim);
        let minted: Minted<_, InstallationPools> = installations.first().expect("first mint");
        let driver = Box::leak(Box::new(PortDriver::vacant()));
        let installation = Port::install(parts, minted, driver).ok().expect("install");
        let mut fixture = Fixture::new(installation, observed, timer);
        assert_eq!(fixture.port.fault(), Some(Fault::InstallationResources));
        assert_eq!(
            fixture.poll_start_on(channel(6)),
            Poll::Ready(Err(StartRefused::Device))
        );
        assert_eq!(fixture.observed.borrow().starts, 0, "no radio start");
        drop(withheld);
    }));
}

#[test]
fn a_port_that_transmitted_reinstalls_with_every_owner_home_and_transmits_again() {
    with_installations(|mut fixture, installations| {
        for _ in 0..2 {
            fixture.start();
            let attempt = fixture.station_mpdu().expect("attempt");
            fixture.submit_now(attempt);
            for _ in 0..4 {
                fixture.turn();
            }
            assert_eq!(fixture.take_completed(), 1);
            let gate = fixture.close_gate();
            fixture.gate = Some(fixture.port.open_gate(gate));
            fixture.turn();
            fixture.stop();
            if fixture.observed.borrow().stops == 2 {
                break;
            }
            fixture = reinstall(fixture, installations)
                .expect("every buffer, descriptor and core owner came home");
        }
        assert_eq!(fixture.observed.borrow().publications, 2);
        assert_eq!(fixture.observed.borrow().starts, 2);
        assert_eq!(fixture.observed.borrow().stops, 2);
    });
}

#[test]
fn the_last_started_channel_survives_a_stop_and_reinstall() {
    with_installations(|mut fixture, installations| {
        let tuned = Channel::from_ghz2_4_channel(channel(11));
        assert_eq!(fixture.poll_start_on(channel(11)), Poll::Ready(Ok(())));
        assert_eq!(DataPort::channel(&fixture.port), tuned);
        fixture.stop();
        let mut fixture = reinstall(fixture, installations).expect("reinstalled");
        assert_eq!(DataPort::channel(&fixture.port), tuned);
        assert_eq!(fixture.poll_start_on(channel(11)), Poll::Ready(Ok(())));
        assert_eq!(
            fixture.observed.borrow().start_channels,
            [channel(11), channel(11)]
        );
        fixture.stop();
    });
}

#[test]
fn a_refused_retune_refuses_the_start_with_its_cause_and_leaves_the_port_stopped_to_start_again() {
    // The radio actor refused the retune because a tracking tick had failed.
    let tick = Poison {
        origin: PoisonOrigin::TrackingTick,
        cause: PoisonCause::Tracking(PhyFault::EdgeTimedOut),
    };
    let retunes = [
        WifiRetuneError::Radio(Failure::Poisoned(tick)),
        WifiRetuneError::Radio(Failure::Poisoned(Poison::request(
            oer_chips_esp32s31_hal::radio::domain::RadioClient::Wifi,
            RequestKind::Retune,
            PoisonCause::Channel(PhyFault::UnexpectedBinding),
        ))),
        WifiRetuneError::Radio(Failure::Refused),
        WifiRetuneError::NotStopped(
            oer_chips_esp32s31_hal::system::owner::maintenance::Error::MacActive { state: 3 },
        ),
    ]
    .map(Fault::Retune);
    // The regulatory domain's refusal of the channel is the front end's too.
    for cause in retunes.into_iter().chain([Fault::ChannelNotPermitted]) {
        with_fixture(|fixture| {
            fixture.start();
            fixture.stop();
            fixture.observed.borrow_mut().fail_start = Some(cause);
            assert_eq!(
                fixture.poll_start_on(channel(1)),
                Poll::Ready(Err(StartRefused::Retune(cause))),
                "{cause:?}: the refusal keeps its cause"
            );
            assert_eq!(fixture.port.fault(), None, "a refused start is no fault");
            assert_eq!(fixture.observed.borrow().start_channels, [channel(6)]);
            let attempt = fixture.station_mpdu().expect("attempt");
            let now = fixture.now();
            assert_eq!(
                fixture
                    .port
                    .submit(attempt, now, &mut fixture.outbox)
                    .unwrap(),
                Err(SubmitRefusal::NotEnabled),
                "the port stayed stopped"
            );
            assert_eq!(fixture.take_completed(), 1);
            assert_eq!(fixture.poll_start_on(channel(1)), Poll::Ready(Ok(())));
            assert_eq!(
                fixture.observed.borrow().start_channels,
                [channel(6), channel(1)]
            );
            fixture.stop();
        });
    }
}

#[test]
fn a_start_the_device_began_and_refused_stops_its_receive_path_before_the_refusal() {
    with_fixture(|fixture| {
        fixture.start();
        fixture.stop();
        let stops = fixture.observed.borrow().stops;
        // A device that refuses its start before it began anything stops
        // nothing.
        let cause = Fault::Receive(crate::rx::control::WifiRxCause::LineRefused);
        fixture.observed.borrow_mut().fail_start = Some(cause);
        assert_eq!(
            fixture.poll_start_on(channel(6)),
            Poll::Ready(Err(StartRefused::Device))
        );
        assert_eq!(fixture.observed.borrow().stops, stops, "nothing began");
        // One whose receive path began stops it first.
        {
            let mut observed = fixture.observed.borrow_mut();
            observed.begin_start = true;
            observed.fail_start = Some(cause);
        }
        assert_eq!(
            fixture.poll_start_on(channel(6)),
            Poll::Ready(Err(StartRefused::Device))
        );
        assert_eq!(fixture.observed.borrow().stops, stops + 1, "rolled back");
        assert_eq!(fixture.port.fault(), None);
        // A rollback the device holds answers once it stopped.
        {
            let mut observed = fixture.observed.borrow_mut();
            observed.fail_start = Some(cause);
            observed.hold_stop = true;
        }
        assert!(fixture.poll_start_on(channel(6)).is_pending());
        fixture.observed.borrow_mut().hold_stop = false;
        assert_eq!(
            fixture.poll_start_on(channel(6)),
            Poll::Ready(Err(StartRefused::Device))
        );
        assert_eq!(fixture.observed.borrow().stops, stops + 2);
        fixture.observed.borrow_mut().begin_start = false;
        assert_eq!(fixture.poll_start_on(channel(6)), Poll::Ready(Ok(())));
        fixture.stop();
    });
}

#[test]
fn a_started_device_whose_start_expires_stops_before_the_deadline_refusal() {
    with_fixture(|fixture| {
        fixture.start();
        fixture.stop();
        let stops = fixture.observed.borrow().stops;
        {
            let mut observed = fixture.observed.borrow_mut();
            observed.begin_start = true;
            observed.hold_start = true;
        }
        assert!(fixture.poll_start_on(channel(6)).is_pending());
        fixture.advance(COMMAND_BOUND);
        assert_eq!(
            fixture.poll_start_on(channel(6)),
            Poll::Ready(Err(StartRefused::Deadline))
        );
        assert_eq!(fixture.observed.borrow().stops, stops + 1);
        // A rollback the device never confirms leaves the port stopping: the
        // answer keeps the start's cause beside the stop's, and the stop
        // goes on with it.
        fixture.observed.borrow_mut().hold_stop = true;
        assert!(fixture.poll_start_on(channel(6)).is_pending());
        fixture.advance(COMMAND_BOUND);
        // The rollback has its own bound from the start's.
        assert!(fixture.poll_start_on(channel(6)).is_pending());
        fixture.advance(COMMAND_BOUND);
        assert!(matches!(
            fixture.poll_start_on(channel(6)),
            Poll::Ready(Err(StartRefused::Unstopped {
                cause: StartFailure::Deadline,
                stop: StopUnconfirmed {
                    cause: StopCause::Deadline,
                    ..
                },
            }))
        ));
        assert_eq!(
            fixture.poll_start_on(channel(6)),
            Poll::Ready(Err(StartRefused::Running)),
            "the port is not stopped"
        );
        assert_eq!(fixture.port.fault(), None);
        {
            let mut observed = fixture.observed.borrow_mut();
            observed.hold_stop = false;
            observed.hold_start = false;
            observed.begin_start = false;
        }
        fixture.stop();
        assert_eq!(fixture.poll_start_on(channel(6)), Poll::Ready(Ok(())));
        fixture.stop();
    });
}

#[test]
fn a_rollback_the_device_refuses_is_unstopped_with_the_start_cause_and_the_stop_goes_on() {
    with_fixture(|fixture| {
        fixture.start();
        fixture.stop();
        let cause = Fault::Receive(crate::rx::control::WifiRxCause::LineRefused);
        {
            let mut observed = fixture.observed.borrow_mut();
            observed.begin_start = true;
            observed.fail_start = Some(cause);
            observed.hold_stop = true;
        }
        assert!(fixture.poll_start_on(channel(6)).is_pending());
        fixture.advance(COMMAND_BOUND);
        assert!(matches!(
            fixture.poll_start_on(channel(6)),
            Poll::Ready(Err(StartRefused::Unstopped {
                cause: StartFailure::Device,
                stop: StopUnconfirmed {
                    cause: StopCause::Deadline,
                    ..
                },
            }))
        ));
        // No start runs before the stop confirms, and nothing retires.
        assert_eq!(
            fixture.poll_start_on(channel(6)),
            Poll::Ready(Err(StartRefused::Running))
        );
        assert!(fixture.port.release().is_none());
        assert_eq!(fixture.port.fault(), None);
        {
            let mut observed = fixture.observed.borrow_mut();
            observed.hold_stop = false;
            observed.begin_start = false;
        }
        fixture.stop();
        assert_eq!(fixture.poll_start_on(channel(6)), Poll::Ready(Ok(())));
        fixture.stop();
    });
}

#[test]
fn a_start_whose_block_ack_programming_fails_after_the_core_enabled_ends_unstopped_then_stops() {
    with_fixture(|fixture| {
        fixture.start();
        fixture.stop();
        // A receive agreement the stopped port applies at its next start.
        let slot = fixture.block_acks.pop().expect("slot");
        let agreement = SupportedBlockAck::<Caps>::new(
            IndividualAddress::new(BSSID).expect("BSSID"),
            oer_protocols_ieee80211_contracts_port::Tid::new(0).expect("TID"),
            oer_protocols_ieee80211_wire::sequence::SequenceNumber::new(10).expect("sequence"),
            8,
        )
        .expect("agreement");
        let agreement = fixture.port.add_block_ack(slot, agreement);
        fixture.observed.borrow_mut().reject_rx_block_acks = true;
        let stops = fixture.observed.borrow().stops;
        // The tune enabled the core before the banks refused the agreement:
        // the refusal says the stop has not run yet.
        assert!(matches!(
            fixture.poll_start_on(channel(6)),
            Poll::Ready(Err(StartRefused::Unstopped {
                cause: StartFailure::Device,
                stop: StopUnconfirmed {
                    cause: StopCause::CoreEnabled,
                    ..
                },
            }))
        ));
        assert_eq!(fixture.port.fault(), None, "a refused start is no fault");
        assert_eq!(
            fixture.poll_start_on(channel(6)),
            Poll::Ready(Err(StartRefused::Running)),
            "no start before the stop confirms"
        );
        assert!(fixture.port.release().is_none(), "the radio is not stopped");
        // The ordinary stop disables the core in the next service and then
        // stops the device.
        fixture.stop();
        assert_eq!(fixture.observed.borrow().stops, stops + 1);
        fixture.observed.borrow_mut().reject_rx_block_acks = false;
        assert_eq!(fixture.poll_start_on(channel(6)), Poll::Ready(Ok(())));
        fixture.stop();
        let slot = fixture.port.remove_block_ack(agreement);
        fixture.block_acks.push(slot);
    });
}

#[test]
fn a_station_command_offered_to_a_faulted_port_comes_back_and_the_port_recovers() {
    with_fixture(|fixture| {
        fixture.start();
        let fault = Fault::Unpublished(crate::port::Unpublished::Key);
        fixture.port.fault_for_test(fault);
        let now = fixture.now();
        let command = PortCommand::Configure {
            permission: fixture.configure.take().expect("permission"),
            receive: StationReceive::Off,
        };
        let Err(CommandFailed::Returned {
            command: PortCommand::Configure { permission, .. },
            fault: found,
        }) = fixture
            .port
            .command(command, now, &mut fixture.outbox)
            .unwrap()
        else {
            panic!("the command comes back beside the fault");
        };
        assert_eq!(found, fault);
        fixture.configure = Some(permission);
        assert!(fixture.outbox.terminals.is_empty(), "it did not run");
        // The port keeps no command: the fault yields to the stop.
        assert!(fixture.port.recover_for_stop());
        assert_eq!(fixture.port.fault(), None);
        fixture.stop();
    });
}

#[test]
fn a_held_retune_starts_once_the_radio_answers_within_its_bound_and_is_refused_past_it() {
    with_fixture(|fixture| {
        fixture.start();
        fixture.stop();
        // The radio actor holds the registers for the retune.
        fixture.observed.borrow_mut().hold_start = true;
        let began = fixture.now();
        assert!(fixture.poll_start_on(channel(1)).is_pending());
        assert!(
            fixture
                .port
                .deadline()
                .is_some_and(|at| at <= began.saturating_add(COMMAND_BOUND)),
            "the held start bounds the port's deadline"
        );
        fixture.advance(COMMAND_BOUND.saturating_sub(Duration::from_millis(1)));
        assert!(fixture.poll_start_on(channel(1)).is_pending());
        // A clock sample meanwhile reads no register and faults nothing.
        assert_eq!(
            fixture.port.sample_clock(),
            Err(radio::ClockError::Unavailable)
        );
        assert_eq!(fixture.port.fault(), None);
        fixture.observed.borrow_mut().hold_start = false;
        assert_eq!(fixture.poll_start_on(channel(1)), Poll::Ready(Ok(())));
        assert_eq!(
            fixture.observed.borrow().start_channels,
            [channel(6), channel(1)]
        );
        assert!(fixture.port.sample_clock().is_ok());
        fixture.stop();
        // A retune the radio holds past the bound refuses the start.
        fixture.observed.borrow_mut().hold_start = true;
        assert!(fixture.poll_start_on(channel(6)).is_pending());
        fixture.advance(COMMAND_BOUND);
        assert_eq!(
            fixture.poll_start_on(channel(6)),
            Poll::Ready(Err(StartRefused::Deadline))
        );
        assert_eq!(fixture.port.fault(), None);
        fixture.observed.borrow_mut().hold_start = false;
        assert_eq!(fixture.poll_start_on(channel(6)), Poll::Ready(Ok(())));
        fixture.stop();
    });
}

/// Install the station's pairwise key in its one slot and take it from its
/// terminal.
fn installed_pairwise<'id, Id: radio::SlotIdentity + 'id>(
    fixture: &mut Fixture<'id, Id>,
) -> PairwiseKey<'id, Id, Station> {
    let (association, _, slot) = fixture.association();
    let (install, _) = fixture.commands.install_pairwise(
        &association,
        slot,
        IndividualAddress::new(BSSID).expect("peer"),
        group_key(7),
    );
    assert!(matches!(fixture.run(install), Ok(CommandProgress::Done)));
    match fixture.outbox.terminals.pop() {
        Some(LowerMacTerminal::PairwiseKeyInstalled(InstalledPairwise::Station(key))) => key,
        _ => panic!("the installation terminal returns the key"),
    }
}

#[test]
fn a_removal_the_registers_refuse_comes_back_and_leaves_the_key_installed() {
    with_fixture(|fixture| {
        fixture.start();
        let (association, _, slot) = fixture.association();
        let (install, _) = fixture.commands.install_pairwise(
            &association,
            slot,
            IndividualAddress::new(BSSID).expect("peer"),
            group_key(7),
        );
        assert!(matches!(fixture.run(install), Ok(CommandProgress::Done)));
        let Some(LowerMacTerminal::PairwiseKeyInstalled(InstalledPairwise::Station(key))) =
            fixture.outbox.terminals.pop()
        else {
            panic!("the installation terminal returns the key");
        };
        let writes = fixture.observed.borrow().key_writes;
        let place = fixture.port.key_place_for_test(&key);
        assert!(place.is_some(), "the installed key has its place");
        fixture.observed.borrow_mut().hardware_unavailable = true;
        let now = fixture.now();
        let Err(CommandFailed::Returned {
            command: PortCommand::RemovePairwise { key, .. },
            fault: Fault::HardwareUnavailable,
        }) = fixture
            .port
            .command(
                PortCommand::RemovePairwise { association, key },
                now,
                &mut fixture.outbox,
            )
            .unwrap()
        else {
            panic!("the removal comes back beside the fault");
        };
        assert!(fixture.outbox.terminals.is_empty(), "nothing was removed");
        assert_eq!(fixture.observed.borrow().key_writes, writes);
        assert_eq!(
            fixture.port.key_place_for_test(&key),
            place,
            "the returned key is still installed in its own place"
        );
    });
}

/// A removal whose key's last use is out: its command is done and the port
/// keeps the command's rights; the last use returns through a sink that
/// takes no receipt, before or after the radio's stop; the next service
/// ends the removal once with its rights, the key cleared once, and the
/// stopped port retires.
fn late_removal_receipt(after_stop: bool) {
    with_installations(|mut fixture, _| {
        fixture.start();
        let (association, _, slot) = fixture.association();
        let (install, _) = fixture.commands.install_pairwise(
            &association,
            slot,
            IndividualAddress::new(BSSID).expect("peer"),
            group_key(7),
        );
        assert!(matches!(fixture.run(install), Ok(CommandProgress::Done)));
        let Some(LowerMacTerminal::PairwiseKeyInstalled(InstalledPairwise::Station(key))) =
            fixture.outbox.terminals.pop()
        else {
            panic!("the installation terminal returns the key");
        };
        let held = fixture.port.hold_key_use_for_test().expect("a key use");
        let _ = fixture.turn();
        let cleared = fixture.observed.borrow().key_clears;
        let now = fixture.now();
        assert!(matches!(
            fixture
                .port
                .command(
                    PortCommand::RemovePairwise { association, key },
                    now,
                    &mut fixture.outbox,
                )
                .unwrap(),
            Ok(CommandProgress::Done)
        ));
        assert!(
            fixture.outbox.terminals.is_empty(),
            "its key is still in use"
        );
        assert!(fixture.port.removal_waits_for_test());
        if after_stop {
            fixture.stop();
        }
        fixture.port.return_key_use_for_test(held);
        assert!(fixture.outbox.terminals.is_empty(), "refused, it waits");
        assert!(fixture.port.removal_waits_for_test());
        if after_stop {
            assert!(
                fixture.port.release().is_none(),
                "a waiting removal holds the port"
            );
        }
        // The next service ends it once.
        let _ = fixture.turn();
        let removed = |terminals: &mut Vec<_>| {
            let before = terminals.len();
            terminals.retain(|terminal| !matches!(terminal, LowerMacTerminal::KeyRemoved(_)));
            before - terminals.len()
        };
        assert_eq!(
            removed(&mut fixture.outbox.terminals),
            1,
            "one terminal with its rights"
        );
        assert!(!fixture.port.removal_waits_for_test());
        let _ = fixture.turn();
        assert_eq!(removed(&mut fixture.outbox.terminals), 0, "not again");
        if !after_stop {
            // The running port's device observes its registers at a turn.
            assert_eq!(
                fixture.observed.borrow().key_clears,
                cleared + 1,
                "cleared once"
            );
            fixture.stop();
        }
        let Fixture {
            mut port, buffers, ..
        } = fixture;
        drop(buffers);
        assert!(port.release().is_some(), "the stopped port retires");
    });
}

#[test]
fn a_late_removal_receipt_ends_its_removal_once_and_the_port_retires() {
    late_removal_receipt(false);
}

#[test]
fn a_late_removal_receipt_after_the_stop_ends_its_removal_once_and_the_port_retires() {
    late_removal_receipt(true);
}

/// The station's removal terminals among `terminals`: pairwise or group.
fn removed_station<'id, T>(terminals: &[LowerMacTerminal<'id, T>], pairwise: bool) -> usize
where
    T: oer_protocols_ieee80211_contracts_port::PortTypes<'id>,
{
    terminals
        .iter()
        .filter(|terminal| match terminal {
            LowerMacTerminal::KeyRemoved(Removed::Station(RemovedKey::Pairwise { .. })) => pairwise,
            LowerMacTerminal::KeyRemoved(Removed::Station(RemovedKey::Group { .. })) => !pairwise,
            _ => false,
        })
        .count()
}

/// A removal its command leaves waiting keeps its rights in its own key's
/// table place: another key's removal meanwhile ends at once with its own
/// rights, and the waiting one ends later, once, with its own.
#[test]
fn a_removal_beside_a_waiting_one_ends_with_its_own_rights_and_the_waiting_one_with_its_own() {
    with_installations(|mut fixture, _| {
        fixture.start();
        let (association, pairwise, group, permission) = fixture.installed_keys();
        // The pairwise key's last use is out: its removal waits.
        let held = fixture.port.hold_key_use_for_test().expect("a key use");
        let _ = fixture.turn();
        let now = fixture.now();
        assert!(matches!(
            fixture
                .port
                .command(
                    PortCommand::RemovePairwise {
                        association,
                        key: pairwise,
                    },
                    now,
                    &mut fixture.outbox,
                )
                .unwrap(),
            Ok(CommandProgress::Done)
        ));
        assert!(fixture.outbox.terminals.is_empty(), "its key is in use");
        assert!(fixture.port.removal_waits_for_test());
        // The group key has no use: its removal ends at once, with the
        // group's rights, and leaves the waiting one where it is.
        assert!(matches!(
            fixture.run(PortCommand::RemoveGroup {
                permission,
                key: group,
            }),
            Ok(CommandProgress::Done)
        ));
        assert_eq!(
            removed_station(&fixture.outbox.terminals, false),
            1,
            "the group's own"
        );
        assert_eq!(
            removed_station(&fixture.outbox.terminals, true),
            0,
            "still waits"
        );
        assert!(fixture.port.removal_waits_for_test());
        fixture.outbox.terminals.clear();
        // The last use returns: the next service ends the waiting removal
        // once, with the pairwise rights.
        fixture.port.return_key_use_for_test(held);
        let _ = fixture.turn();
        let _ = fixture.turn();
        assert_eq!(removed_station(&fixture.outbox.terminals, true), 1, "once");
        assert_eq!(
            removed_station(&fixture.outbox.terminals, false),
            0,
            "not again"
        );
        assert!(!fixture.port.removal_waits_for_test());
        fixture.stop();
        let Fixture {
            mut port, buffers, ..
        } = fixture;
        drop(buffers);
        assert!(port.release().is_some(), "the stopped port retires");
    });
}

/// Registers held elsewhere at a publication: every owner of the attempt
/// goes back, the refusal recovers for a stop, and the stop ends the
/// attempt once, withdrawn with its owners, through the port's real stop.
#[test]
fn a_publication_the_registers_refuse_ends_once_at_the_stop_with_its_owners() {
    with_fixture(|fixture| {
        fixture.start();
        let tickets = fixture.port.admissions_for_test().0;
        fixture.observed.borrow_mut().hardware_unavailable = true;
        let attempt = fixture.station_mpdu().expect("attempt");
        let now = fixture.now();
        let _submitted = fixture
            .port
            .submit(attempt, now, &mut fixture.outbox)
            .unwrap();
        let _served = fixture
            .port
            .service_tx(TxFacts::DUE, false, now, &mut fixture.outbox)
            .unwrap();
        assert_eq!(
            fixture.port.fault(),
            Some(Fault::Unpublished(crate::port::Unpublished::Registers))
        );
        assert_eq!(fixture.take_completed(), 0, "no terminal yet");
        assert_eq!(fixture.port.admissions_for_test().0, tickets, "ticket home");
        // The registers come back; nothing publishes the attempt again.
        fixture.observed.borrow_mut().hardware_unavailable = false;
        let published = fixture.observed.borrow().publications;
        let stops = fixture.observed.borrow().stops;
        assert!(fixture.port.recover_for_stop(), "every owner is whole");
        fixture.stop();
        assert!(
            fixture.observed.borrow().stops > stops,
            "the device's own stop ran"
        );
        assert_eq!(fixture.observed.borrow().publications, published);
        assert!(matches!(
            last_outcome(fixture),
            Some(TxOutcome::Withdrawn(WithdrawCause::Disabled))
        ));
        assert_eq!(fixture.take_completed(), 1, "exactly one terminal");
        assert_eq!(fixture.port.admissions_for_test().0, tickets);
        assert_eq!(fixture.port.fault(), None);
    });
}

#[test]
fn a_key_left_installed_is_erased_by_the_reinstall_and_its_slot_takes_the_next_key() {
    with_installations(|mut fixture, installations| {
        fixture.start();
        // The lifecycle stops without removing its key.
        let _left = installed_pairwise(&mut fixture);
        fixture.stop();
        let mut fixture = reinstall(fixture, installations).expect("reinstalled");
        fixture.start();
        // The next installation's key takes the same slot, and frames go
        // out under it.
        let key = installed_pairwise(&mut fixture);
        let attempt = fixture.protected_mpdu(&key).expect("a frame");
        let published = fixture.observed.borrow().publications;
        fixture.submit_now(attempt);
        let _ = fixture.turn();
        assert_eq!(
            fixture.observed.borrow().publications,
            published + 1,
            "the frame under the new key is published"
        );
    });
}

/// A single MPDU's plan that falls back from 24 to 6 Mb/s: its arena's end
/// costs each publication at the rate it went at, PPDU, SIFS and the Ack
/// the model declares, summed; never the last rate times the count.
#[test]
fn a_single_arena_s_cost_sums_each_publication_at_its_own_rate() {
    with_fixture(|fixture| {
        fixture.start();
        fixture.observed.borrow_mut().statuses = std::vec![ACK_TIMEOUT, SUCCESS];
        let [first, _second] = fixture.arenas();
        fixture.fill(&first, &[10], plain);
        let now = fixture.now();
        let mut arenas = fixture.port.arena();
        let fast = arenas
            .rate(PhyRate::Legacy(LegacyRate::Ofdm24M))
            .expect("24 Mb/s");
        let slow = arenas
            .rate(PhyRate::Legacy(LegacyRate::Ofdm6M))
            .expect("6 Mb/s");
        let plan = plan(fast, 1).then(RetryStage {
            rate: slow,
            transmissions: core::num::NonZeroU8::MIN,
            protection: Protection::None,
        });
        let plan = plan.unwrap_or_else(|_| panic!("two stages"));
        arenas
            .publish(publication(first, ArenaFormat::Single(plan)), now)
            .unwrap_or_else(|_| panic!("published"));
        let done = fixture.arena_end();
        assert!(matches!(done.end(), AttemptEnd::Acked { .. }));
        // By hand, IEEE Std 802.11-2020 17.4.3: the 30-octet MPDU and its
        // FCS, 16 SERVICE and 6 tail bits, 4 us symbols of 96 or 24 bits,
        // 20 us preamble, 6 us signal extension; each publication's SIFS of
        // 10 us and Ack of 14 octets at 6 Mb/s.
        let psdu = 30 + 4;
        let ppdu =
            |bits_per_symbol: u32| 20 + 4 * (16 + 8 * psdu + 6_u32).div_ceil(bits_per_symbol) + 6;
        let ack = 10 + 20 + 4 * (16 + 8 * 14 + 6_u32).div_ceil(24) + 6;
        assert_eq!(
            done.cost(),
            oer_protocols_ieee80211_contracts_port::TxEstimate::Estimated {
                micros: ppdu(96) + ack + ppdu(24) + ack
            }
        );
        let _first = fixture.port.arena().release(done, drop).expect("released");
    });
}

impl<Id: radio::SlotIdentity> Fixture<'_, Id> {
    /// The arena's end, letting a hardware timeout's abort settle.
    fn arena_end_through_abort(&mut self) -> Completed<'_, Id> {
        for _ in 0..16 {
            if let Some(done) = self.turn() {
                return done;
            }
            self.advance(crate::ordinary_tx::TX_ABORT_SETTLE);
        }
        panic!("the arena ends");
    }
}

/// A publication that never reached the air (a collision ended it before
/// its PPDU) adds nothing to its attempt's cost, and what the attempt's
/// earlier publication cost stays: one publication's PPDU, SIFS and Ack.
#[test]
fn a_publication_that_never_reached_the_air_adds_no_cost_and_keeps_the_earlier() {
    with_fixture(|fixture| {
        fixture.start();
        fixture.observed.borrow_mut().statuses = std::vec![ACK_TIMEOUT, COLLISION];
        let [first, _second] = fixture.arenas();
        fixture.fill(&first, &[10], plain);
        fixture.publish_single(first, 2);
        let done = fixture.arena_end();
        // The attempt's end keeps the transmission its first publication
        // made; the collided one made none.
        assert!(
            matches!(done.end(), AttemptEnd::Collision(Some(air)) if air.transmissions.get() == 1),
            "the collided publication never reached the air"
        );
        // IEEE Std 802.11-2020 17.4.3, as for the summed cost above: the
        // 30-octet MPDU at 24 Mb/s, its SIFS and an Ack at 6 Mb/s.
        let psdu = 30 + 4;
        let ppdu = 20 + 4 * (16 + 8 * psdu + 6_u32).div_ceil(96) + 6;
        let ack = 10 + 20 + 4 * (16 + 8 * 14 + 6_u32).div_ceil(24) + 6;
        assert_eq!(
            done.cost(),
            oer_protocols_ieee80211_contracts_port::TxEstimate::Estimated { micros: ppdu + ack }
        );
        let _first = fixture.port.arena().release(done, drop).expect("released");
    });
}

/// An aborted publication may have reached the air, so its attempt's cost
/// is unknown, never zero: first alone, and after a publication of known
/// cost, which the unknown one makes unknown too.
#[test]
fn an_aborted_publication_makes_its_attempt_s_cost_unknown() {
    let histories: [&[(u8, u8)]; 2] = [&[TIMEOUT_EDGE], &[ACK_TIMEOUT, TIMEOUT_EDGE]];
    for statuses in histories {
        with_fixture(|fixture| {
            fixture.start();
            fixture.observed.borrow_mut().statuses = statuses.to_vec();
            let [first, _second] = fixture.arenas();
            fixture.fill(&first, &[10], plain);
            let transmissions = u8::try_from(statuses.len()).expect("a short plan");
            fixture.publish_single(first, transmissions);
            let done = fixture.arena_end_through_abort();
            assert!(matches!(done.end(), AttemptEnd::Aborted), "{statuses:?}");
            assert_eq!(
                done.cost(),
                oer_protocols_ieee80211_contracts_port::TxEstimate::Unknown,
                "{statuses:?}"
            );
            let _first = fixture.port.arena().release(done, drop).expect("released");
        });
    }
}

/// Serve one transmit turn, whatever it answers.
fn serve_once<'id, Id: radio::SlotIdentity + 'id>(fixture: &mut Fixture<'id, Id>) {
    let waker = fixture.waker();
    let now = fixture.now();
    let facts = fixture.port.poll_facts(&mut Context::from_waker(&waker));
    let _served = fixture
        .port
        .service_tx(
            facts.tx.unwrap_or(TxFacts::DUE),
            facts.tbtt,
            now,
            &mut fixture.outbox,
        )
        .unwrap();
}

/// The single MPDU that ended last, and its outcome.
fn last_outcome<'id, Id: radio::SlotIdentity + 'id>(
    fixture: &Fixture<'id, Id>,
) -> Option<oer_protocols_ieee80211_contracts_port::SingleOutcome<Ack, core::convert::Infallible>> {
    match fixture.outbox.terminals.last() {
        Some(LowerMacTerminal::TxCompleted(TxCompleted::Single(completed))) => {
            Some(*completed.outcome())
        }
        _ => None,
    }
}

/// The attempts a faulted port's stop reports: queued, then published.
fn stop_counts<'id, Id: radio::SlotIdentity + 'id>(fixture: &mut Fixture<'id, Id>) -> (u8, u8) {
    let Poll::Ready(Err(unconfirmed)) = fixture.poll_stop_now() else {
        panic!("a faulted port's stop is unconfirmed");
    };
    (
        unconfirmed.observations.queued,
        unconfirmed.observations.published,
    )
}

#[test]
fn a_submission_its_queue_refuses_ends_withdrawn_with_its_owners_beside_the_fault() {
    with_fixture(|fixture| {
        fixture.start();
        let attempt = fixture.station_mpdu().expect("attempt");
        let before = fixture.port.admissions_for_test();
        fixture
            .port
            .refuse_for_test(crate::port::TestRefusal::QueuePush);
        let now = fixture.now();
        let _submitted = fixture
            .port
            .submit(attempt, now, &mut fixture.outbox)
            .unwrap();
        assert!(matches!(
            fixture.port.fault(),
            Some(Fault::QueueFull { .. })
        ));
        assert!(matches!(
            last_outcome(fixture),
            Some(TxOutcome::Withdrawn(WithdrawCause::Disabled))
        ));
        assert_eq!(fixture.take_completed(), 1, "the attempt came back");
        assert_eq!(fixture.port.admissions_for_test(), before, "no ticket left");
        assert_eq!(stop_counts(fixture), (0, 0), "nothing stays behind");
    });
}

#[test]
fn a_refused_publication_its_queue_cannot_take_back_ends_withdrawn_beside_the_fault() {
    with_fixture(|fixture| {
        fixture.start();
        let key = installed_pairwise(fixture);
        // The first MPDU holds the queue; the second waits behind it.
        fixture.hold_air(true);
        let first = fixture.station_mpdu().expect("first");
        fixture.submit_now(first);
        let second = fixture.protected_mpdu(&key).expect("second");
        fixture.submit_now(second);
        // The second's key is gone at its publication, which goes back to
        // the queue: the queue refuses it.
        fixture.port.uninstall_keys_for_test();
        fixture
            .port
            .refuse_for_test(crate::port::TestRefusal::QueuePush);
        fixture.hold_air(false);
        serve_once(fixture);
        serve_once(fixture);
        assert!(matches!(
            fixture.port.fault(),
            Some(Fault::QueueFull { .. })
        ));
        assert!(matches!(
            last_outcome(fixture),
            Some(TxOutcome::Withdrawn(WithdrawCause::Disabled))
        ));
        assert_eq!(fixture.take_completed(), 2, "both MPDUs came back");
        assert_eq!(stop_counts(fixture), (0, 0), "nothing stays behind");
    });
}

#[test]
fn a_retry_its_queue_refuses_stays_published_beside_the_fault() {
    with_fixture(|fixture| {
        fixture.start();
        fixture.observed.borrow_mut().statuses = std::vec![ACK_TIMEOUT];
        let attempt = fixture
            .mpdu_of(None, core::num::NonZeroU8::new(2).expect("two"))
            .expect("attempt");
        fixture.submit_now(attempt);
        fixture
            .port
            .refuse_for_test(crate::port::TestRefusal::QueuePush);
        serve_once(fixture);
        assert!(matches!(
            fixture.port.fault(),
            Some(Fault::QueueFull { .. })
        ));
        assert_eq!(fixture.take_completed(), 0, "no terminal");
        assert_eq!(
            stop_counts(fixture),
            (0, 1),
            "the attempt stays in its published place"
        );
    });
}

#[test]
fn a_completion_whose_buffer_refuses_its_memory_keeps_the_attempt_published() {
    with_fixture(|fixture| {
        fixture.start();
        let attempt = fixture.station_mpdu().expect("attempt");
        let before = fixture.port.admissions_for_test();
        fixture.submit_now(attempt);
        fixture
            .port
            .refuse_for_test(crate::port::TestRefusal::Return);
        serve_once(fixture);
        assert!(matches!(
            fixture.port.fault(),
            Some(Fault::ReturnRefused { .. })
        ));
        assert_eq!(fixture.take_completed(), 0, "no terminal");
        assert_eq!(
            fixture.port.admissions_for_test().0,
            before.0,
            "the completion's ticket went home"
        );
        assert_eq!(stop_counts(fixture), (0, 1), "the attempt stays published");
    });
}

/// A refused return leaves the lease whole on its descriptor and the
/// attempt beside it: the stop recovers and confirms, its cleanup gives the
/// lease back to its attempt, which ends withdrawn with its memory, and the
/// port, its descriptors idle, reinstalls.
#[test]
fn a_refused_return_s_lease_goes_home_at_the_cleanup_and_the_port_reinstalls() {
    with_installations(|mut fixture, installations| {
        fixture.start();
        let attempt = fixture.station_mpdu().expect("attempt");
        fixture.submit_now(attempt);
        fixture
            .port
            .refuse_for_test(crate::port::TestRefusal::Return);
        serve_once(&mut fixture);
        assert!(matches!(
            fixture.port.fault(),
            Some(Fault::ReturnRefused { .. })
        ));
        assert!(fixture.port.recover_for_stop(), "every owner stays whole");
        fixture.stop();
        assert_eq!(
            fixture.take_completed(),
            0,
            "the attempt waits beside its lease"
        );
        assert!(matches!(
            fixture.port.clean_stopped(&mut fixture.outbox),
            Ok(Ok(()))
        ));
        let [LowerMacTerminal::TxCompleted(TxCompleted::Single(completed))] =
            fixture.outbox.terminals.as_slice()
        else {
            panic!("the attempt's one terminal");
        };
        assert!(matches!(
            completed.outcome(),
            TxOutcome::Withdrawn(WithdrawCause::Disabled)
        ));
        // A retirement needs every bank place home and every descriptor
        // idle: the terminal's drop returns the memory it carries.
        assert!(
            reinstall(fixture, installations).is_ok(),
            "the memory came home with its attempt, every descriptor idle"
        );
    });
}

#[test]
fn a_submission_the_core_refuses_goes_back_to_its_queue_with_its_ticket() {
    with_fixture(|fixture| {
        fixture.start();
        let attempt = fixture.station_mpdu().expect("attempt");
        let before = fixture.port.admissions_for_test();
        fixture
            .port
            .refuse_for_test(crate::port::TestRefusal::Submit);
        let published = fixture.observed.borrow().publications;
        let now = fixture.now();
        let _submitted = fixture
            .port
            .submit(attempt, now, &mut fixture.outbox)
            .unwrap();
        assert_eq!(
            fixture.port.fault(),
            Some(Fault::Core(crate::lower_mac::LowerMacFault::KeyUseMismatch))
        );
        assert_eq!(fixture.take_completed(), 0, "no terminal");
        assert_eq!(
            fixture.port.admissions_for_test(),
            before,
            "the ticket is back"
        );
        assert_eq!(fixture.observed.borrow().publications, published);
        assert_eq!(
            stop_counts(fixture),
            (1, 0),
            "the attempt waits at the head of its queue"
        );
    });
}

/// One service finds the first queue's MPDU delivered and the next
/// queue's completion unreadable: the delivered one still comes back, and
/// the service answers the core's fault.
#[test]
fn a_completion_ahead_of_a_faulting_queue_in_one_service_still_comes_back() {
    with_fixture(|fixture| {
        fixture.start();
        fixture.observed.borrow_mut().statuses = std::vec![SUCCESS, (9, 0)];
        fixture.hold_air(true);
        let one = core::num::NonZeroU8::MIN;
        let first = fixture
            .mpdu_in(None, one, WmmAccessCategory::Voice)
            .expect("first");
        fixture.submit_now(first);
        let second = fixture
            .mpdu_in(None, one, WmmAccessCategory::Background)
            .expect("second");
        fixture.submit_now(second);
        fixture.hold_air(false);
        // Both completions latch before the one service reads them.
        let waker = fixture.waker();
        let _ = fixture.port.poll_facts(&mut Context::from_waker(&waker));
        let _ = fixture.port.poll_facts(&mut Context::from_waker(&waker));
        let now = fixture.now();
        let served = fixture
            .port
            .service_tx(TxFacts::DUE, false, now, &mut fixture.outbox)
            .unwrap();
        assert!(matches!(served, Err(Fault::Core(_))), "{served:?}");
        assert!(matches!(
            last_outcome(fixture),
            Some(TxOutcome::Ended(SingleEnd::Delivered { .. }))
        ));
        assert_eq!(fixture.take_completed(), 1, "the delivered MPDU came back");
    });
}
