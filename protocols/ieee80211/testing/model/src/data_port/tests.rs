//! The model's data port: the shared conformance cases and the model's
//! own focused cases.

use super::*;
use alloc::sync::Arc;
use core::num::NonZeroU8;
use core::sync::atomic::{AtomicUsize, Ordering};
use oer_protocols_ieee80211_contracts_port::Tid;
use oer_protocols_ieee80211_contracts_port::{
    AckHeader, Attempt, ChannelWidth, ClassFrame, HeaderSpec, LowerMacTerminal, MpduLength,
    MpduProtection, Protection, RetryPlan, RetryStage, SizedFrame, StationReceive, SupportedPower,
    TerminalAccess, TerminalParts, TxCompleted, TxOperation, TxPower,
    arena::{ArenaFormat, ArenaPublication},
};
use oer_protocols_ieee80211_testing_conformance::data_port::{
    self as cases, Case, DataPortHarness,
};
use oer_protocols_ieee80211_wire::{frame::FrameControl, phy::LegacyRate};
use oer_protocols_radio_port::{LocalId, SlotClaim, with_local_slot};

const STATION: [u8; 6] = [2, 0, 0, 0, 0, 1];
const ACCESS_POINT: [u8; 6] = [2, 0, 0, 0, 0, 2];
const PEER: [u8; 6] = [2, 0, 0, 0, 0, 9];
const CATEGORY: WmmAccessCategory = WmmAccessCategory::BestEffort;

type B<'s> = ModelBackend<LocalId<'s>>;
type Types<'s> = ModelTypes<B<'s>>;
type Port<'s> = ModelPort<'s, LocalId<'s>>;

/// A waker that counts its wakes.
struct Counter(AtomicUsize);
impl alloc::task::Wake for Counter {
    fn wake(self: Arc<Self>) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }
    fn wake_by_ref(self: &Arc<Self>) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }
}

/// What the port put in its sink, in its order.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Put {
    Terminal,
    Received,
}

/// The terminals and frames the port put in its sink.
struct Sink<'s> {
    terminals: Vec<LowerMacTerminal<'s, Types<'s>>>,
    received: Vec<(ModelRxBuffer<'s, LocalId<'s>>, ModelRxMeta)>,
    tbtt: usize,
    order: Vec<Put>,
}
impl<'s> PortSink<'s, Types<'s>> for Sink<'s> {
    fn terminal(&mut self, terminal: LowerMacTerminal<'s, Types<'s>>) {
        self.terminals.push(terminal);
        self.order.push(Put::Terminal);
    }
    fn tbtt(&mut self, _: oer_protocols_ieee80211_contracts_port::TbttEvent) {
        self.tbtt += 1;
    }
    fn received(&mut self, frame: ModelRxBuffer<'s, LocalId<'s>>, meta: ModelRxMeta) {
        self.received.push((frame, meta));
        self.order.push(Put::Received);
    }
}

/// The station's association start, group slots and group permission.
type GroupTokens<'s> = (
    oer_protocols_ieee80211_contracts_port::association::AssociationStart<
        's,
        Station,
        MODEL_KEY_SLOTS,
        MODEL_KEY_SLOTS,
        ModelPool<LocalId<'s>, 5>,
    >,
    [GroupKeySlot<'s, Station, MODEL_KEY_SLOTS, MODEL_KEY_SLOTS, ModelPool<LocalId<'s>, 5>>;
        MODEL_KEY_SLOTS],
    Slot<'s, ModelGroupPermission<Station>, ModelPool<LocalId<'s>, 8>>,
    [PairwiseKeySlot<'s, Station, MODEL_KEY_SLOTS, MODEL_KEY_SLOTS, ModelPool<LocalId<'s>, 5>>;
        MODEL_KEY_SLOTS],
);

struct Harness<'s> {
    clock: Rc<oer_portable_time_virtual::VirtualClock>,
    air: Rc<ModelAir>,
    installations: Installations<LocalId<'s>, ModelPools>,
    port: Port<'s>,
    commands: ModelCommands<'s, LocalId<'s>>,
    sink: Sink<'s>,
    tickets: Vec<Slot<'s, ModelTickets, ModelPool<LocalId<'s>, 1>>>,
    small: Vec<ModelBuffer<'s, B<'s>, ModelSmall>>,
    terminal_access: TerminalAccess<'s, ModelTickets, ModelPool<LocalId<'s>, 1>>,
    configure: Option<
        ConfigurePermission<
            's,
            Station,
            oer_protocols_radio_port::Child<ModelPool<LocalId<'s>, 9>, 1>,
        >,
    >,
    gate: Option<GateOpen<'s, ModelPool<LocalId<'s>, 0>>>,
    group: Option<GroupTokens<'s>>,
    arenas: Vec<Arena>,
    counter: Arc<Counter>,
    /// The access point's own permissions.
    access_point: Option<ModelAccessPointPermissions<'s, B<'s>>>,
    /// The station MPDU the last station terminal returned `Sent`, kept for a
    /// retransmission.
    station: Option<(
        Slot<'s, ModelTickets, ModelPool<LocalId<'s>, 1>>,
        ModelSent<'s, B<'s>, ModelSmall>,
    )>,
}

fn channel() -> Channel {
    Channel::ghz2_4(6, ChannelWidth::Mhz20).unwrap()
}

impl<'s> Harness<'s> {
    fn new(claim: SlotClaim<LocalId<'s>>) -> Self {
        let clock = Rc::new(oer_portable_time_virtual::VirtualClock::new());
        let mut installations = Installations::claim(claim);
        let (install, air) = ModelPort::install(
            &mut installations,
            channel(),
            [
                IndividualAddress::new(STATION).unwrap(),
                IndividualAddress::new(ACCESS_POINT).unwrap(),
            ],
            clock.clone(),
        )
        .unwrap();
        let ModelInstall {
            port,
            commands,
            resources,
        } = install;
        let ModelResources {
            shared:
                ModelShared {
                    tickets,
                    small,
                    terminal_access,
                    gate,
                    ..
                },
            station:
                ModelStationPermissions {
                    commands: station_commands,
                    association: station_association,
                    group: station_group,
                    group_permission: station_group_permission,
                    pairwise: station_pairwise,
                    ..
                },
            access_point,
        } = resources;
        Self {
            clock,
            air,
            installations,
            port,
            commands,
            sink: Sink {
                terminals: Vec::new(),
                received: Vec::new(),
                tbtt: 0,
                order: Vec::new(),
            },
            tickets,
            small,
            terminal_access,
            configure: Some(station_commands.configure),
            gate: Some(gate),
            group: Some((
                station_association,
                station_group,
                station_group_permission,
                station_pairwise,
            )),
            arenas: Vec::new(),
            counter: Arc::new(Counter(AtomicUsize::new(0))),
            access_point: Some(access_point),
            station: None,
        }
    }

    fn now_wake(&self) -> (Instant, Waker) {
        (self.clock.now(), Waker::from(self.counter.clone()))
    }

    /// The station's unprotected data frame of `ticket` and `buffer`,
    /// admitted by the port.
    fn admitted(
        &mut self,
        ticket: Slot<'s, ModelTickets, ModelPool<LocalId<'s>, 1>>,
        buffer: ModelBuffer<'s, B<'s>, ModelSmall>,
    ) -> ModelAdmitted<LocalId<'s>, ModelCaps, ()> {
        self.admitted_under(ticket, buffer, None)
    }

    fn admitted_under(
        &mut self,
        ticket: Slot<'s, ModelTickets, ModelPool<LocalId<'s>, 1>>,
        buffer: ModelBuffer<'s, B<'s>, ModelSmall>,
        key: Option<&ModelPairwiseKey<'s, B<'s>, Station>>,
    ) -> ModelAdmitted<LocalId<'s>, ModelCaps, ()> {
        self.admitted_numbered(ticket, buffer, key.map(|key| (key, 1)))
    }

    /// The station's protected data frame under `key`, numbered `number`.
    fn admitted_under_number(
        &mut self,
        ticket: Slot<'s, ModelTickets, ModelPool<LocalId<'s>, 1>>,
        buffer: ModelBuffer<'s, B<'s>, ModelSmall>,
        key: &ModelPairwiseKey<'s, B<'s>, Station>,
        number: u64,
    ) -> ModelAdmitted<LocalId<'s>, ModelCaps, ()> {
        self.admitted_numbered(ticket, buffer, Some((key, number)))
    }

    fn admitted_numbered(
        &mut self,
        ticket: Slot<'s, ModelTickets, ModelPool<LocalId<'s>, 1>>,
        buffer: ModelBuffer<'s, B<'s>, ModelSmall>,
        key: Option<(&ModelPairwiseKey<'s, B<'s>, Station>, u64)>,
    ) -> ModelAdmitted<LocalId<'s>, ModelCaps, ()> {
        self.admitted_planned(ticket, buffer, key, NonZeroU8::new(2).unwrap())
    }

    /// The station's data frame under one stage of `transmissions`.
    fn admitted_planned(
        &mut self,
        ticket: Slot<'s, ModelTickets, ModelPool<LocalId<'s>, 1>>,
        mut buffer: ModelBuffer<'s, B<'s>, ModelSmall>,
        key: Option<(&ModelPairwiseKey<'s, B<'s>, Station>, u64)>,
        transmissions: NonZeroU8,
    ) -> ModelAdmitted<LocalId<'s>, ModelCaps, ()> {
        buffer.bytes_mut().unwrap().fill(0);
        let rate = self.port.rate(PhyRate::Legacy(LegacyRate::Ofdm6M)).unwrap();
        let header = HeaderSpec::new(
            FrameControl::new(if key.is_some() { 0x4108 } else { 0x0108 }).unwrap(),
            0,
            IndividualAddress::new(PEER).unwrap(),
            STATION,
            PEER,
            0,
            None,
            None,
            None,
        )
        .and_then(AckHeader::new)
        .unwrap();
        let frame = TxFrame::Individual {
            frame: SizedFrame::new(header, MpduLength::new(40).unwrap()).unwrap(),
            plan: RetryPlan::new(RetryStage {
                rate,
                transmissions,
                protection: Protection::None,
            }),
        };
        let frame = ClassFrame::new(frame).ok().unwrap();
        let source = prepare_model(
            TxSource::Fresh {
                buffer,
                frame,
                body: None,
                protection: match key {
                    Some((key, number)) => {
                        TxProtection::Ccmp(oer_protocols_ieee80211_contracts_port::NumberedKey {
                            key,
                            packet_number:
                                oer_protocols_ieee80211_wire::ccmp::CcmpPacketNumber::new(number)
                                    .unwrap(),
                        })
                    }
                    None => TxProtection::Open,
                },
            }
            .check()
            .ok()
            .unwrap(),
        )
        .ok()
        .unwrap();
        let attempt = Attempt {
            context: (),
            ticket,
            source,
            access_category: CATEGORY,
            protection: Protection::None,
            power: SupportedPower::new(TxPower::Calibrated).unwrap(),
            coex_operation: TxOperation::Data,
        }
        .admit();
        self.port.admit(attempt)
    }

    fn mpdu(&mut self) -> Option<ModelAdmitted<LocalId<'s>, ModelCaps, ()>> {
        let ticket = self.tickets.pop()?;
        let buffer = self.small.pop()?;
        Some(self.admitted(ticket, buffer))
    }

    fn service(&mut self) -> Option<ModelArenaCompleted> {
        let (now, waker) = self.now_wake();
        let facts = self.port.poll_facts(&mut Context::from_waker(&waker));
        self.port
            .service_tx(
                facts.tx.unwrap_or(TxFacts::DUE),
                facts.tbtt,
                now,
                &mut self.sink,
            )
            .unwrap()
    }

    fn start(&mut self) {
        cases::start(self).unwrap();
    }

    fn command(
        &mut self,
        command: PortCommand<'s, Types<'s>, ModelCaps>,
    ) -> CommandProgress<'s, Types<'s>, ModelCaps> {
        let now = self.clock.now();
        self.port.command(command, now, &mut self.sink).unwrap()
    }

    fn recycle_received(&mut self) -> usize {
        let frames = core::mem::take(&mut self.sink.received);
        let count = frames.len();
        for (frame, _) in frames {
            self.port.recycle(frame);
        }
        count
    }
}

impl<'s> DataPortHarness<'s> for Harness<'s> {
    type Port = Port<'s>;
    type Sink = Sink<'s>;

    fn parts(&mut self) -> (&mut Port<'s>, &mut Sink<'s>) {
        (&mut self.port, &mut self.sink)
    }
    fn now(&self) -> Instant {
        self.clock.now()
    }
    fn advance(&mut self, by: oer_portable_time::Duration) {
        self.clock.advance(by).unwrap();
        self.air.clock_advanced();
    }
    fn waker(&self) -> Waker {
        Waker::from(self.counter.clone())
    }
    fn wakes(&self) -> usize {
        self.counter.0.load(Ordering::Relaxed)
    }
    fn channel(&self) -> SupportedChannel<ModelCaps> {
        SupportedChannel::new(
            oer_protocols_radio_regulatory::TEST_DOMAIN
                .permit(oer_protocols_ieee80211_contracts_port::RegulatedChannel::from(channel()))
                .unwrap(),
        )
        .unwrap()
    }
    fn settle(&mut self) {}
    fn deliver(&mut self, units: usize) {
        for unit in 0..units {
            self.air.receive(
                &[
                    0x08, 0x01, 0, 0, 2, 0, 0, 0, 0, 1, 2, 0, 0, 0, 0, 9, 2, 0, 0, 0, 0, 9,
                    unit as u8, 0,
                ],
                ModelRxMeta::new(channel(), -40, -90),
            );
        }
    }
    fn station_mpdu(&mut self) -> Option<ModelAdmitted<LocalId<'s>, ModelCaps, ()>> {
        self.mpdu()
    }
    fn identity(attempt: &ModelAdmitted<LocalId<'s>, ModelCaps, ()>) -> u8 {
        attempt.operation.ticket().index()
    }
    fn hold_air(&mut self, hold: bool) {
        self.air.hold_completion(CATEGORY, hold);
    }
    fn configure(&mut self) -> Option<PortCommand<'s, Types<'s>, ModelCaps>> {
        Some(PortCommand::Configure {
            permission: self.configure.take()?,
            receive: StationReceive::Off,
        })
    }
    fn take_configured(&mut self) -> bool {
        let terminals = core::mem::take(&mut self.sink.terminals);
        let mut found = false;
        for terminal in terminals {
            match terminal {
                LowerMacTerminal::Configured(
                    oer_protocols_ieee80211_contracts_port::Configured::Station(permission),
                ) => {
                    self.configure = Some(permission);
                    found = true;
                }
                other => self.sink.terminals.push(other),
            }
        }
        found
    }
    fn take_completed(&mut self) -> usize {
        let terminals = core::mem::take(&mut self.sink.terminals);
        let mut completed = 0;
        for terminal in terminals {
            match terminal {
                LowerMacTerminal::TxCompleted(TxCompleted::Single(terminal)) => {
                    let TerminalParts {
                        ticket, payload, ..
                    } = terminal.into_parts(&mut self.terminal_access);
                    self.tickets.push(ticket);
                    let (buffer, _) = payload.into_parts();
                    let mut buffer = match buffer {
                        ModelAnyBuffer::Small(buffer) => buffer,
                        ModelAnyBuffer::SmallSent(sent) => {
                            <Types<'s> as TxBufferPool<'s, ModelSmall>>::renew(sent).0
                        }
                        _ => panic!("the harness submits small buffers"),
                    };
                    // A new MSDU starts its own admission.
                    buffer.renew_msdu().unwrap();
                    self.small.push(buffer);
                    completed += 1;
                }
                other => self.sink.terminals.push(other),
            }
        }
        completed
    }
    fn publish_held_arena(&mut self) -> bool {
        self.air.hold_completion(CATEGORY, true);
        self.publish_arena()
    }
    fn release_arena(&mut self) {
        self.air.hold_completion(CATEGORY, false);
    }
    fn settle_arena(&mut self, done: ModelArenaCompleted) {
        let mut arena = self.port.arena();
        let right = arena.release(done, |_| {}).unwrap();
        self.arenas.push(right);
    }
    fn answer_station(&mut self, ends: &[cases::StationEnd]) {
        use cases::StationEnd;
        for end in ends {
            self.air.answer(match end {
                StationEnd::Ack => AirAnswer::Ack,
                StationEnd::AckTimeout => AirAnswer::NoResponse,
                StationEnd::CtsTimeout => AirAnswer::CtsTimeout,
                StationEnd::Collision => AirAnswer::Collision,
                StationEnd::KeyRefused => AirAnswer::KeyRefused,
                StationEnd::HardwareTimeout => AirAnswer::HardwareTimeout,
            });
        }
    }
    fn submit_station(
        &mut self,
        again: bool,
        transmissions: NonZeroU8,
    ) -> Result<(), cases::Violation> {
        let kept = self.station.take();
        let attempt = match (again, kept) {
            (true, Some((ticket, sent))) => {
                let rate = self
                    .port
                    .rate(PhyRate::Legacy(LegacyRate::Ofdm6M))
                    .ok_or(cases::Violation::Fixture)?;
                let source = TxSource::Retransmission {
                    mpdu: sent,
                    plan: oer_protocols_ieee80211_contracts_port::RetransmissionPlan::Individual(
                        RetryPlan::new(RetryStage {
                            rate,
                            transmissions,
                            protection: Protection::None,
                        }),
                    ),
                }
                .check()
                .map_err(|_| cases::Violation::Fixture)?;
                let source = prepare_model::<Station, ModelCaps, B<'_>, ModelSmall>(source)
                    .map_err(|_| cases::Violation::Fixture)?;
                self.port.admit(
                    Attempt {
                        context: (),
                        ticket,
                        source,
                        access_category: CATEGORY,
                        protection: Protection::None,
                        power: SupportedPower::new(TxPower::Calibrated).unwrap(),
                        coex_operation: TxOperation::Data,
                    }
                    .admit(),
                )
            }
            (true, None) => return Err(cases::Violation::Fixture),
            (false, kept) => {
                if let Some((ticket, sent)) = kept {
                    // The old MPDU ends here; its memory starts a new one.
                    let mut buffer = <Types<'s> as TxBufferPool<'s, ModelSmall>>::renew(sent).0;
                    buffer.renew_msdu().unwrap();
                    self.small.push(buffer);
                    self.tickets.push(ticket);
                }
                let ticket = self.tickets.pop().ok_or(cases::Violation::Fixture)?;
                let buffer = self.small.pop().ok_or(cases::Violation::Fixture)?;
                self.admitted_planned(ticket, buffer, None, transmissions)
            }
        };
        let now = self.clock.now();
        self.port
            .submit(attempt, now, &mut self.sink)
            .map_err(|_| cases::Violation::Refusal)
    }
    fn take_station(&mut self) -> Option<cases::StationSent> {
        use cases::SentEnd;
        let at = self.sink.terminals.iter().position(|terminal| {
            matches!(
                terminal,
                LowerMacTerminal::TxCompleted(TxCompleted::Single(_))
            )
        })?;
        let LowerMacTerminal::TxCompleted(TxCompleted::Single(terminal)) =
            self.sink.terminals.remove(at)
        else {
            panic!("the station terminal");
        };
        let TerminalParts {
            ticket,
            payload,
            outcome,
            ..
        } = terminal.into_parts(&mut self.terminal_access);
        let end = match outcome {
            TxOutcome::Ended(SingleEnd::Delivered { .. }) => SentEnd::Delivered,
            TxOutcome::Ended(_) => SentEnd::Undelivered,
            TxOutcome::Indeterminate(_) => SentEnd::Indeterminate,
            TxOutcome::Expired { .. } => SentEnd::Expired,
            _ => SentEnd::Other,
        };
        let retry = match payload.into_parts() {
            (ModelAnyBuffer::SmallSent(sent), _) => {
                let memory: &[u8] = sent.buffer.memory.as_ref();
                let retry = oer_protocols_ieee80211_wire::frame::FrameControlFields::from_bytes(
                    *memory.first_chunk::<2>().unwrap(),
                )
                .retry();
                self.station = Some((ticket, sent));
                retry
            }
            (ModelAnyBuffer::Small(buffer), _) => {
                self.small.push(buffer);
                self.tickets.push(ticket);
                false
            }
            _ => panic!("the harness submits small buffers"),
        };
        Some(cases::StationSent { end, retry })
    }
}

impl Harness<'_> {
    /// Publish a single 28-octet MPDU of the first free arena at 6 Mb/s.
    fn publish_arena(&mut self) -> bool {
        self.publish_arena_as(Protection::None)
    }

    /// [`Self::publish_arena`] under `protection`.
    fn publish_arena_as(&mut self, protection: Protection) -> bool {
        let now = self.clock.now();
        let mut arena = self.port.arena();
        if self.arenas.is_empty() {
            let Some(rights) = arena.arenas() else {
                return false;
            };
            self.arenas.extend(rights);
        }
        let Some(right) = self.arenas.pop() else {
            return false;
        };
        let memory: ModelNetworkMemory = Box::new([0; ModelLarge::CAPACITY]);
        if arena
            .intake(&right, memory, |region| {
                let header = [
                    0x08, 0x01, 0, 0, 2, 0, 0, 0, 0, 9, 2, 0, 0, 0, 0, 1, 2, 0, 0, 0, 0, 9,
                ];
                region.get_mut(..header.len())?.copy_from_slice(&header);
                region.get_mut(header.len()..28)?.fill(0);
                Some(28)
            })
            .is_err()
        {
            return false;
        }
        let Some(rate) = arena.rate(PhyRate::Legacy(LegacyRate::Ofdm6M)) else {
            return false;
        };
        let publication = ArenaPublication {
            arena: right,
            format: ArenaFormat::Single(RetryPlan::new(RetryStage {
                rate,
                transmissions: NonZeroU8::MIN,
                protection: Protection::None,
            })),
            access_category: CATEGORY,
            protection,
            encryption: MpduProtection::Open,
            power: SupportedPower::new(TxPower::Calibrated).unwrap(),
        };
        arena.publish(publication, now).is_ok()
    }

    /// The air's next event, with the clock's owner's notice.
    fn advance_air(&mut self) -> Instant {
        let at = self.air.next_air_event().unwrap();
        self.clock.advance_to(at);
        self.air.clock_advanced();
        at
    }
}

fn with_harness(test: impl for<'s> FnOnce(&mut Harness<'s>)) {
    embassy_futures::block_on(with_local_slot(async |claim| {
        let mut harness = Harness::new(claim);
        test(&mut harness);
        harness.recycle_received();
    }));
}

#[test]
fn the_model_passes_every_data_port_case() {
    for case in Case::ALL {
        with_harness(|harness| {
            assert_eq!(cases::check(harness, case), Ok(()), "{case:?}");
        });
    }
}

#[test]
fn refusals_before_the_start_return_every_owner_unchanged() {
    with_harness(|harness| {
        let attempt = harness.mpdu().unwrap();
        let ticket = attempt.operation.ticket().index();
        let now = harness.clock.now();
        let refused = harness
            .port
            .submit(attempt, now, &mut harness.sink)
            .err()
            .unwrap();
        assert_eq!(refused, SubmitRefusal::NotEnabled);
        // The attempt ended withdrawn at once: its terminal returns its
        // ticket and buffer, never published.
        match harness.sink.terminals.pop() {
            Some(LowerMacTerminal::TxCompleted(TxCompleted::Single(terminal))) => {
                let TerminalParts {
                    ticket: back,
                    outcome,
                    ..
                } = terminal.into_parts(&mut harness.terminal_access);
                assert_eq!(back.index(), ticket);
                assert!(matches!(
                    outcome,
                    oer_protocols_ieee80211_contracts_port::TxOutcome::Withdrawn(_)
                ));
                harness.tickets.push(back);
            }
            _ => panic!("the withdrawn terminal"),
        }
        assert!(harness.air.sent().is_empty());
        let color = oer_protocols_ieee80211_contracts_port::BssColor::new(5).unwrap();
        match harness.command(PortCommand::SetBssColor(color)) {
            CommandProgress::Refused(
                PortCommand::SetBssColor(back),
                CommandRefusal::NotEnabled,
            ) => {
                assert_eq!(back, color);
            }
            _ => panic!("a stopped port refuses commands"),
        }
        let gate = harness.gate.take().unwrap();
        match harness.command(PortCommand::CloseGate(gate)) {
            CommandProgress::Refused(PortCommand::CloseGate(_), CommandRefusal::NotEnabled) => {}
            _ => panic!("the gate comes back"),
        }
        assert!(
            harness.sink.terminals.is_empty(),
            "a refusal makes no terminal"
        );
    });
}

#[test]
fn no_data_port_call_wakes_its_caller() {
    with_harness(|harness| {
        harness.start();
        harness.deliver(2);
        let (now, waker) = harness.now_wake();
        let mut cx = Context::from_waker(&waker);
        let before = harness.wakes();
        let facts = harness.port.poll_facts(&mut cx);
        assert!(facts.rx, "the delivered units are receive work");
        let _ = harness.port.deadline();
        let attempt = harness.mpdu().unwrap();
        harness
            .port
            .submit(attempt, now, &mut harness.sink)
            .ok()
            .unwrap();
        harness
            .port
            .service_tx(TxFacts::DUE, false, now, &mut harness.sink)
            .unwrap();
        let RxPull::Unit { frame, .. } = harness.port.receive(4, now).unwrap() else {
            panic!("a delivered unit");
        };
        harness.port.recycle(frame);
        let RxPull::Unit { frame, .. } = harness.port.receive(4, now).unwrap() else {
            panic!("the second unit");
        };
        let lent = harness.port.lend(frame).ok().unwrap();
        assert_eq!(harness.port.lent(), 1);
        assert!(harness.port.lend_room());
        let _ = harness.port.poll_lend_room(&mut cx);
        let _ = harness.port.poll_lent_returned(&mut cx);
        let color = oer_protocols_ieee80211_contracts_port::BssColor::new(1).unwrap();
        assert!(matches!(
            harness.command(PortCommand::SetBssColor(color)),
            CommandProgress::Done
        ));
        let _ = harness.port.sample_clock();
        let _ = harness.port.poll_facts(&mut cx);
        assert_eq!(harness.wakes(), before, "no call woke its caller");
        // The network's return is the world's edge, which wakes the waiter.
        drop(lent);
        assert_eq!(harness.port.lent(), 0);
        harness.take_completed();
    });
}

#[test]
fn a_receive_pull_takes_one_unit_within_the_callers_room() {
    with_harness(|harness| {
        harness.start();
        harness.deliver(2);
        let now = harness.clock.now();
        let RxPull::Unit { frame: first, .. } = harness.port.receive(1, now).unwrap() else {
            panic!("one unit within the room");
        };
        assert!(
            matches!(harness.port.receive(0, now).unwrap(), RxPull::Empty),
            "no room takes no unit"
        );
        assert_eq!(
            harness.air.pending_rx(),
            1,
            "the other unit waits in the ring"
        );
        harness.port.recycle(first);
        let RxPull::Unit { frame, .. } = harness.port.receive(1, now).unwrap() else {
            panic!("the waiting unit");
        };
        harness.port.recycle(frame);
        assert!(matches!(
            harness.port.receive(1, now).unwrap(),
            RxPull::Empty
        ));
    });
}

/// A quiet timed air with these receive limits and 32-subframe arenas.
fn timed(ring: usize, held: usize, lent: usize) -> AirConfig {
    AirConfig {
        seed: 1,
        timing: AirTiming::HT_2_4_GHZ,
        arena_frames: 32,
        rx_limits: ModelRxLimits { ring, held, lent },
        busy: None,
        peer: None,
    }
}

/// An access point whose first frame contends from the air's first instant.
fn saturated_peer() -> AirPeer {
    AirPeer::best_effort(
        Box::new(|_| alloc::vec![0; 100]),
        ModelRxMeta::new(channel(), -40, -90),
        PhyRate::Legacy(LegacyRate::Ofdm54M),
        Offered::Saturated,
        PeerWindow::Agreed,
        Tid::new(0).unwrap(),
    )
}

/// The time on air of a 28-octet MPDU and its FCS at 6 Mb/s with its ACK.
const TIMED_EXCHANGE_MICROS: u64 = {
    // 16 + 256 + 6 bits at 24 a symbol: 12 symbols, 48 us, then 26 us of
    // preamble and signal extension; SIFS; 14 octets at 24 Mb/s, 34 us.
    48 + 26 + 10 + 34
};

#[test]
fn a_publication_on_a_timed_air_ends_with_its_exchange_and_not_before() {
    with_harness(|harness| {
        harness.air.set_air_time(timed(8, 8, 8)).unwrap();
        harness.start();
        let start = harness.clock.now();
        assert!(harness.publish_arena());
        let waker = harness.waker();
        let quiet = harness.port.poll_facts(&mut Context::from_waker(&waker));
        assert!(quiet.tx.is_none(), "nothing to serve before the air's end");
        assert!(harness.service().is_none());
        let onto_the_channel = harness.advance_air();
        assert_eq!(onto_the_channel, start);
        assert!(harness.service().is_none());
        let end = harness.advance_air();
        assert_eq!(
            end.saturating_duration_since(start).as_micros(),
            TIMED_EXCHANGE_MICROS
        );
        let facts = harness.port.poll_facts(&mut Context::from_waker(&waker));
        assert_ne!(facts.tx.unwrap().bits(), 0, "the air's end is a completion");
        let done = harness.service().unwrap();
        assert!(matches!(
            done.end(),
            oer_protocols_ieee80211_contracts_port::arena::AttemptEnd::Acked { .. }
        ));
        harness.settle_arena(done);
        assert_eq!(harness.air.air_report().unwrap().station.exchanges, 1);
        assert_eq!(harness.air.air_report().unwrap().station.subframes, 1);
    });
}

#[test]
fn a_publications_protection_and_encryption_take_their_time_on_the_air() {
    with_harness(|harness| {
        harness.air.set_air_time(timed(8, 8, 8)).unwrap();
        harness.start();
        let start = harness.clock.now();
        assert!(harness.publish_arena_as(Protection::RtsCts {
            rate: LegacyRate::Ofdm24M,
        }));
        harness.advance_air();
        let end = harness.advance_air();
        assert_eq!(
            end.saturating_duration_since(start).as_micros(),
            TIMED_EXCHANGE_MICROS + 34 + 10 + 34 + 10
        );
    });
}

#[test]
fn an_arena_on_the_air_reports_no_completion_while_other_work_is_due() {
    with_harness(|harness| {
        harness.air.set_air_time(timed(8, 8, 8)).unwrap();
        harness.start();
        assert!(harness.publish_arena());
        let attempt = harness.mpdu().unwrap();
        let (now, waker) = harness.now_wake();
        assert!(harness.port.submit(attempt, now, &mut harness.sink).is_ok());
        let facts = harness.port.poll_facts(&mut Context::from_waker(&waker));
        assert_eq!(facts.tx.map(|tx| tx.bits()), Some(0), "work, no completion");
    });
}

#[test]
fn a_collision_on_a_timed_air_ends_the_publication_unanswered() {
    with_harness(|harness| {
        let config = AirConfig {
            peer: Some(saturated_peer()),
            ..timed(8, 8, 8)
        };
        harness.air.set_air_time(config).unwrap();
        harness.start();
        assert!(harness.publish_arena());
        harness.advance_air();
        harness.advance_air();
        let done = harness.service().unwrap();
        assert!(matches!(
            done.end(),
            oer_protocols_ieee80211_contracts_port::arena::AttemptEnd::NoResponse(_)
        ));
        assert_eq!(harness.air.air_report().unwrap().station.collisions, 1);
        harness.settle_arena(done);
    });
}

#[test]
fn an_answer_that_ends_the_attempt_before_the_air_takes_no_time() {
    with_harness(|harness| {
        harness.air.set_air_time(timed(8, 8, 8)).unwrap();
        harness.start();
        harness.air.answer(AirAnswer::AirDenied);
        assert!(harness.publish_arena());
        assert_eq!(harness.air.next_air_event(), None);
        let done = harness.service().unwrap();
        harness.settle_arena(done);
    });
}

#[test]
fn a_single_mpdu_on_a_timed_air_waits_for_its_exchange() {
    with_harness(|harness| {
        harness.air.set_air_time(timed(8, 8, 8)).unwrap();
        harness.start();
        let attempt = harness.mpdu().unwrap();
        let (now, _) = harness.now_wake();
        assert!(harness.port.submit(attempt, now, &mut harness.sink).is_ok());
        assert!(harness.service().is_none());
        assert!(harness.sink.terminals.is_empty());
        let start = harness.advance_air();
        assert!(harness.air.next_air_event().is_some());
        harness.service();
        assert!(harness.sink.terminals.is_empty(), "still on the air");
        let end = harness.advance_air();
        assert!(end > start);
        harness.service();
        assert_eq!(harness.take_completed(), 1);
    });
}

#[test]
fn a_single_mpdu_that_collides_takes_the_next_transmission_of_its_plan() {
    with_harness(|harness| {
        let config = AirConfig {
            peer: Some(saturated_peer()),
            ..timed(8, 8, 8)
        };
        harness.air.set_air_time(config).unwrap();
        harness.start();
        let attempt = harness.mpdu().unwrap();
        let (now, _) = harness.now_wake();
        assert!(harness.port.submit(attempt, now, &mut harness.sink).is_ok());
        harness.service();
        harness.advance_air();
        harness.advance_air();
        harness.service();
        let report = harness.air.air_report().unwrap();
        assert_eq!(report.station.collisions, 1);
        assert_eq!(harness.take_completed(), 0, "its plan has one more");
        for _ in 0..1000 {
            harness.advance_air();
            harness.service();
            if !harness.sink.terminals.is_empty() {
                break;
            }
        }
        assert_eq!(harness.take_completed(), 1);
        assert_eq!(harness.air.air_report().unwrap().station.exchanges, 2);
    });
}

#[test]
fn a_stop_ends_an_exchange_the_air_still_owes() {
    with_harness(|harness| {
        harness.air.set_air_time(timed(8, 8, 8)).unwrap();
        harness.start();
        assert!(harness.publish_arena());
        harness.advance_air();
        let (now, waker) = harness.now_wake();
        let mut cx = Context::from_waker(&waker);
        assert!(harness.port.poll_stop(&mut cx, now).is_pending());
        let done = harness.service().unwrap();
        assert!(matches!(
            done.end(),
            oer_protocols_ieee80211_contracts_port::arena::AttemptEnd::Aborted
        ));
        harness.settle_arena(done);
        assert!(matches!(
            harness.port.poll_stop(&mut cx, now),
            Poll::Ready(Ok(()))
        ));
    });
}

#[test]
fn a_timed_air_sets_the_arenas_subframes() {
    with_harness(|harness| {
        let mut arena = harness.port.arena();
        assert_eq!(usize::from(arena.capacity()), MODEL_ARENA_FRAMES);
        drop(arena);
        harness.air.set_air_time(timed(8, 8, 8)).unwrap();
        arena = harness.port.arena();
        assert_eq!(arena.capacity(), 32);
        assert_eq!(arena.ampdu_capabilities().max_subframes, 32);
    });
}

#[test]
fn the_receive_limits_bind_each_at_its_own_place() {
    with_harness(|harness| {
        harness.air.set_air_time(timed(2, 3, 1)).unwrap();
        harness.start();
        // The caller's units stop at `held`, whatever the ring holds.
        harness.deliver(5);
        let now = harness.clock.now();
        let mut units = Vec::new();
        while let RxPull::Unit { frame, .. } = harness.port.receive(u8::MAX, now).unwrap() {
            units.push(frame);
        }
        assert_eq!(units.len(), 3);
        // The network's units stop at `lent`, whatever the caller holds.
        let first = units.pop().unwrap();
        let lent = harness.port.lend(first).ok().unwrap();
        assert!(!harness.port.lend_room());
        let refused = harness.port.lend(units.pop().unwrap()).err().unwrap();
        assert_eq!(refused.reason, LendRefusal::Capacity);
        harness.port.recycle(refused.buffer);
        harness.port.recycle(units.pop().unwrap());
        drop(lent);
        assert!(harness.port.lend_room());
        harness.recycle_received();
    });
}

#[test]
fn the_ring_limit_refuses_a_landing_frame_only() {
    with_harness(|harness| {
        let peer = AirPeer::best_effort(
            Box::new(|sequence| {
                let mut frame = alloc::vec![0_u8; 28];
                frame[..2].copy_from_slice(&[0x08, 0x01]);
                frame[22..24].copy_from_slice(&sequence.sequence_control().to_le_bytes());
                frame
            }),
            ModelRxMeta::new(channel(), -40, -90),
            PhyRate::Ht(
                oer_protocols_ieee80211_wire::phy::HtRate::new(
                    oer_protocols_ieee80211_wire::phy::HtMcs::new(7).unwrap(),
                    oer_protocols_ieee80211_wire::phy::PpduBandwidth::Mhz20,
                    false,
                )
                .unwrap(),
            ),
            Offered::Saturated,
            PeerWindow::Assumed(4),
            Tid::new(0).unwrap(),
        );
        harness
            .air
            .set_air_time(AirConfig {
                peer: Some(peer),
                ..timed(2, 8, 8)
            })
            .unwrap();
        harness.start();
        for _ in 0..100 {
            if harness.air.air_report().unwrap().ring_drops >= 2 {
                break;
            }
            harness.advance_air();
        }
        let report = harness.air.air_report().unwrap();
        assert_eq!(report.landed, 2);
        assert_eq!(harness.air.pending_rx(), 2);
        harness.recycle_received();
    });
}

#[test]
fn the_model_holds_its_receive_bound_and_keeps_the_rest_in_the_ring() {
    with_harness(|harness| {
        harness.start();
        harness.deliver(MODEL_RX_EVENTS + 1);
        let now = harness.clock.now();
        let mut held = Vec::new();
        while let RxPull::Unit { frame, .. } = harness.port.receive(u8::MAX, now).unwrap() {
            held.push(frame);
        }
        assert_eq!(held.len(), MODEL_RX_EVENTS);
        assert_eq!(harness.air.pending_rx(), 1);
        for frame in held {
            harness.port.recycle(frame);
        }
        let RxPull::Unit { frame, .. } = harness.port.receive(1, now).unwrap() else {
            panic!("room again");
        };
        harness.port.recycle(frame);
    });
}

#[test]
fn the_deadline_is_kept_and_follows_admission_expiry_and_completion() {
    with_harness(|harness| {
        harness.start();
        assert_eq!(harness.port.deadline(), None, "an idle port has none");
        harness.air.hold(CATEGORY, true);
        let attempt = harness.mpdu().unwrap();
        let admitted = harness.clock.now();
        harness
            .port
            .submit(attempt, admitted, &mut harness.sink)
            .ok()
            .unwrap();
        let lifetime = harness
            .air
            .edca(VifRole::Station)
            .access_category(CATEGORY)
            .msdu_lifetime;
        let expiry = admitted.saturating_add(lifetime);
        assert_eq!(harness.port.deadline(), Some(expiry));
        assert_eq!(harness.port.deadline(), Some(expiry), "reading keeps it");
        harness.service();
        assert_eq!(
            harness.port.deadline(),
            Some(expiry),
            "the busy queue keeps it"
        );
        harness.advance(lifetime);
        harness.service();
        assert_eq!(harness.port.deadline(), None, "expiry took it off");
        assert_eq!(harness.take_completed(), 1);
        assert_eq!(harness.air.expired_msdus(), 1);
        // A published MPDU bounds the deadline until its completion.
        harness.air.hold(CATEGORY, false);
        harness.air.hold_completion(CATEGORY, true);
        let attempt = harness.mpdu().unwrap();
        let now = harness.clock.now();
        harness
            .port
            .submit(attempt, now, &mut harness.sink)
            .ok()
            .unwrap();
        assert_eq!(
            harness.port.deadline(),
            Some(now),
            "a free queue publishes at once"
        );
        harness.service();
        assert_eq!(
            harness.port.deadline(),
            Some(now.saturating_add(MODEL_PUBLICATION_BOUND))
        );
        harness.air.hold_completion(CATEGORY, false);
        harness.service();
        assert_eq!(harness.take_completed(), 1);
        assert_eq!(harness.port.deadline(), None, "the completion took it off");
    });
}

#[test]
fn a_configuration_is_held_behind_a_published_station_mpdu_then_runs() {
    with_harness(|harness| {
        harness.start();
        harness.air.hold_completion(CATEGORY, true);
        let attempt = harness.mpdu().unwrap();
        let now = harness.clock.now();
        harness
            .port
            .submit(attempt, now, &mut harness.sink)
            .ok()
            .unwrap();
        harness.service();
        let command = harness.configure().unwrap();
        let CommandProgress::Held(command) = harness.command(command) else {
            panic!("a station MPDU on the air holds the configuration");
        };
        assert!(!harness.take_configured());
        harness.air.hold_completion(CATEGORY, false);
        harness.service();
        assert_eq!(harness.take_completed(), 1);
        assert!(matches!(harness.command(command), CommandProgress::Done));
        assert!(harness.take_configured());
    });
}

fn group_install<'s>(harness: &mut Harness<'s>) -> PortCommand<'s, Types<'s>, ModelCaps> {
    let (station_association, station_group, station_group_permission, _) =
        harness.group.take().unwrap();
    let association = ModelKeys::<B<'s>, Station>::begin_association(
        station_association,
        oer_protocols_ieee80211_wire::security::Spp::NotNegotiated,
    );
    let [slot, _] = station_group;
    harness.commands.install_group(
        &association,
        station_group_permission,
        slot,
        GroupKeyId::new(1).unwrap(),
        Ccmp128Key::new(&[3; 16]),
    )
}

#[test]
fn a_group_installation_is_held_while_the_air_holds_group_commands_then_runs() {
    with_harness(|harness| {
        harness.start();
        let install = group_install(harness);
        harness.air.hold_group_commands(true);
        let CommandProgress::Held(install) = harness.command(install) else {
            panic!("held group commands hold the installation");
        };
        harness.air.hold_group_commands(false);
        assert!(matches!(harness.command(install), CommandProgress::Done));
        assert!(matches!(
            harness.sink.terminals.as_slice(),
            [LowerMacTerminal::GroupKeyInstalled(_)]
        ));
    });
}

#[test]
fn old_group_key_ingress_at_every_stage_reaches_the_sink_before_the_replacements_terminal() {
    with_harness(|harness| {
        harness.start();
        let install = group_install(harness);
        assert!(matches!(harness.command(install), CommandProgress::Done));
        let Some(LowerMacTerminal::GroupKeyInstalled(
            oer_protocols_ieee80211_contracts_port::InstalledGroup::Station { key, permission },
        )) = harness.sink.terminals.pop()
        else {
            panic!("the installation's terminal");
        };
        // A group data frame from the access point, captured under the
        // installed key at each stage of the receive path.
        let frame = [
            0x08, 0x02, 0, 0, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 2, 0, 0, 0, 0, 2, 2, 0, 0, 0, 0,
            2, 0, 0,
        ];
        for stage in [
            GroupRxStage::Dma,
            GroupRxStage::Interrupt,
            GroupRxStage::Software,
        ] {
            assert!(harness.air.receive_group(
                &frame,
                ModelRxMeta::new(channel(), -40, -90),
                VifRole::Station,
                GroupKeyId::new(1).unwrap(),
                &[3; 16],
                stage,
            ));
        }
        harness.sink.order.clear();
        let replace = PortCommand::ReplaceGroup {
            permission,
            installed: key,
            key_id: GroupKeyId::new(1).unwrap(),
            key: Ccmp128Key::new(&[4; 16]),
        };
        assert!(matches!(harness.command(replace), CommandProgress::Done));
        assert_eq!(
            harness.sink.order,
            [Put::Received, Put::Received, Put::Received, Put::Terminal],
            "the old key's ingress precedes the replacement's terminal"
        );
        assert!(matches!(
            harness.sink.terminals.as_slice(),
            [LowerMacTerminal::GroupKeyReplaced { .. }]
        ));
        assert_eq!(harness.recycle_received(), 3);
    });
}

#[test]
fn a_held_command_past_its_bound_is_a_named_fault_that_keeps_its_owners() {
    with_harness(|harness| {
        harness.start();
        let install = group_install(harness);
        harness.air.hold_group_commands(true);
        let since = harness.clock.now();
        let CommandProgress::Held(install) = harness.command(install) else {
            panic!("held");
        };
        harness.advance(MODEL_HELD_COMMAND_BOUND);
        let now = harness.clock.now();
        let fault = harness
            .port
            .command(install, now, &mut harness.sink)
            .err()
            .unwrap();
        assert_eq!(
            fault,
            ModelPortFault::CommandStalled {
                kind: CommandKind::InstallGroup,
                since,
            }
        );
        assert_eq!(harness.port.retained.len(), 1, "the port keeps the command");
        // Every later fallible call reports the same fault.
        let now = harness.clock.now();
        assert_eq!(
            harness
                .port
                .service_tx(TxFacts::DUE, false, now, &mut harness.sink)
                .err(),
            Some(fault)
        );
    });
}

#[test]
fn a_held_command_refused_by_a_stop_measures_its_bound_from_the_restart() {
    with_harness(|harness| {
        harness.start();
        let install = group_install(harness);
        harness.air.hold_group_commands(true);
        let CommandProgress::Held(install) = harness.command(install) else {
            panic!("held");
        };
        let waker = harness.waker();
        let now = harness.clock.now();
        assert!(matches!(
            harness
                .port
                .poll_stop(&mut Context::from_waker(&waker), now),
            Poll::Ready(Ok(()))
        ));
        let CommandProgress::Refused(install, CommandRefusal::NotEnabled) =
            harness.command(install)
        else {
            panic!("a stopped port refuses the held command");
        };
        // The radio stays stopped longer than a held command's bound.
        harness.advance(MODEL_HELD_COMMAND_BOUND);
        harness.start();
        assert_eq!(harness.port.deadline(), None, "no held command's bound");
        let CommandProgress::Held(install) = harness.command(install) else {
            panic!("the air still holds group commands");
        };
        harness.air.hold_group_commands(false);
        assert!(matches!(harness.command(install), CommandProgress::Done));
        assert!(matches!(
            harness.sink.terminals.as_slice(),
            [LowerMacTerminal::GroupKeyInstalled(_)]
        ));
    });
}

#[test]
fn a_quarantined_air_is_named_with_what_the_port_held() {
    with_harness(|harness| {
        harness.start();
        harness.air.hold_completion(CATEGORY, true);
        let attempt = harness.mpdu().unwrap();
        let now = harness.clock.now();
        harness
            .port
            .submit(attempt, now, &mut harness.sink)
            .ok()
            .unwrap();
        harness.service();
        harness.deliver(1);
        let RxPull::Unit { frame, .. } = harness.port.receive(1, now).unwrap() else {
            panic!("a unit");
        };
        harness.air.poison();
        let now = harness.clock.now();
        let fault = harness
            .port
            .service_tx(TxFacts::DUE, false, now, &mut harness.sink)
            .err()
            .unwrap();
        assert_eq!(
            fault,
            ModelPortFault::Quarantined {
                call: ModelPortCall::ServiceTx,
                observation: ModelPortObservation {
                    published: 1,
                    queued: 0,
                    arena_on_air: false,
                    rx_out: 1,
                    lent: 0,
                    removals: 0,
                },
            }
        );
        assert_eq!(harness.port.fault(), Some(fault));
        harness.port.recycle(frame);
    });
}

#[test]
fn a_stop_withdraws_queued_work_then_reinstalls_with_the_network_credit() {
    embassy_futures::block_on(with_local_slot(async |claim| {
        let mut harness = Harness::new(claim);
        harness.start();
        harness.deliver(1);
        let now = harness.clock.now();
        let RxPull::Unit { frame, .. } = harness.port.receive(1, now).unwrap() else {
            panic!("a unit");
        };
        let lent = harness.port.lend(frame).ok().unwrap();
        harness.air.hold(CATEGORY, true);
        let attempt = harness.mpdu().unwrap();
        harness
            .port
            .submit(attempt, now, &mut harness.sink)
            .ok()
            .unwrap();
        let waker = harness.waker();
        let mut cx = Context::from_waker(&waker);
        // The withdrawal's terminal goes out through the next service.
        assert!(harness.port.poll_stop(&mut cx, now).is_pending());
        assert_eq!(harness.port.deadline(), Some(now));
        harness.service();
        assert_eq!(harness.take_completed(), 1);
        assert!(matches!(
            harness.port.poll_stop(&mut cx, now),
            Poll::Ready(Ok(()))
        ));
        assert_eq!(harness.air.disable_calls(), 1);
        let Harness {
            port,
            mut installations,
            ..
        } = harness;
        let Ok(ModelInstall { port, .. }) = port.reinstall(&mut installations) else {
            panic!("a retired port reinstalls");
        };
        assert_eq!(port.generation(), 1);
        assert_eq!(port.lent(), 1, "the network's unit stays lent");
        drop(lent);
        assert_eq!(
            port.lent(),
            0,
            "its return lowers the new installation's count"
        );
    }));
}

#[test]
fn a_port_with_work_out_does_not_reinstall() {
    embassy_futures::block_on(with_local_slot(async |claim| {
        let mut harness = Harness::new(claim);
        harness.start();
        let Harness {
            port,
            mut installations,
            ..
        } = harness;
        let Err(refused) = port.reinstall(&mut installations) else {
            panic!("a running port does not reinstall");
        };
        let (port, refused) = *refused;
        assert_eq!(refused, ModelReinstallRefused::NotRetired);
        assert_eq!(port.generation(), 0);
    }));
}

#[test]
fn a_pairwise_removal_is_held_while_a_published_mpdu_uses_its_key_then_runs() {
    with_harness(|harness| {
        harness.start();
        let (start, _, _, [slot, _]) = harness.group.take().unwrap();
        let association = ModelKeys::<B<'_>, Station>::begin_association(
            start,
            oer_protocols_ieee80211_wire::security::Spp::NotNegotiated,
        );
        let (install, identity) = harness.commands.install_pairwise(
            &association,
            slot,
            IndividualAddress::new(PEER).unwrap(),
            Ccmp128Key::new(&[7; 16]),
        );
        assert!(matches!(harness.command(install), CommandProgress::Done));
        let Some(LowerMacTerminal::PairwiseKeyInstalled(
            oer_protocols_ieee80211_contracts_port::InstalledPairwise::Station(key),
        )) = harness.sink.terminals.pop()
        else {
            panic!("the installation's terminal");
        };
        assert!(
            ModelKeys::<B<'_>, Station>::pairwise_identity(&key) == identity,
            "the issued identity names the installed key"
        );
        harness.air.hold_completion(CATEGORY, true);
        let ticket = harness.tickets.pop().unwrap();
        let buffer = harness.small.pop().unwrap();
        let attempt = harness.admitted_under(ticket, buffer, Some(&key));
        let now = harness.clock.now();
        harness
            .port
            .submit(attempt, now, &mut harness.sink)
            .ok()
            .unwrap();
        harness.service();
        let CommandProgress::Held(remove) =
            harness.command(PortCommand::RemovePairwise { association, key })
        else {
            panic!("a published MPDU under the key holds its removal");
        };
        assert!(harness.sink.terminals.is_empty(), "no removal terminal yet");
        harness.air.hold_completion(CATEGORY, false);
        harness.service();
        assert_eq!(harness.take_completed(), 1);
        assert!(matches!(harness.command(remove), CommandProgress::Done));
        assert!(matches!(
            harness.sink.terminals.as_slice(),
            [LowerMacTerminal::KeyRemoved(_)]
        ));
    });
}

#[test]
fn a_retransmission_keeps_its_packet_number_and_sets_retry_while_a_renewal_numbers_anew() {
    with_harness(|harness| {
        harness.start();
        let (start, _, _, [slot, _]) = harness.group.take().unwrap();
        let association = ModelKeys::<B<'_>, Station>::begin_association(
            start,
            oer_protocols_ieee80211_wire::security::Spp::NotNegotiated,
        );
        let (install, _) = harness.commands.install_pairwise(
            &association,
            slot,
            IndividualAddress::new(PEER).unwrap(),
            Ccmp128Key::new(&[7; 16]),
        );
        assert!(matches!(harness.command(install), CommandProgress::Done));
        let Some(LowerMacTerminal::PairwiseKeyInstalled(
            oer_protocols_ieee80211_contracts_port::InstalledPairwise::Station(key),
        )) = harness.sink.terminals.pop()
        else {
            panic!("the installation's terminal");
        };
        // Both transmissions of the plan go unanswered.
        harness.air.answer(AirAnswer::NoResponse);
        harness.air.answer(AirAnswer::NoResponse);
        let ticket = harness.tickets.pop().unwrap();
        let buffer = harness.small.pop().unwrap();
        let attempt = harness.admitted_under(ticket, buffer, Some(&key));
        let now = harness.clock.now();
        harness
            .port
            .submit(attempt, now, &mut harness.sink)
            .ok()
            .unwrap();
        harness.service();
        let Some(LowerMacTerminal::TxCompleted(TxCompleted::Single(terminal))) =
            harness.sink.terminals.pop()
        else {
            panic!("the unanswered MPDU's terminal");
        };
        let TerminalParts {
            ticket, payload, ..
        } = terminal.into_parts(&mut harness.terminal_access);
        let (ModelAnyBuffer::SmallSent(sent), None) = payload.into_parts() else {
            panic!("a published MPDU comes back sent");
        };
        let rate = harness
            .port
            .rate(PhyRate::Legacy(LegacyRate::Ofdm6M))
            .unwrap();
        let source = prepare_model::<Station, ModelCaps, B<'_>, ModelSmall>(
            TxSource::Retransmission {
                mpdu: sent,
                plan: oer_protocols_ieee80211_contracts_port::RetransmissionPlan::Individual(
                    RetryPlan::new(RetryStage {
                        rate,
                        transmissions: NonZeroU8::MIN,
                        protection: Protection::None,
                    }),
                ),
            }
            .check()
            .ok()
            .unwrap(),
        )
        .ok()
        .unwrap();
        let attempt = harness.port.admit(
            Attempt {
                context: (),
                ticket,
                source,
                access_category: CATEGORY,
                protection: Protection::None,
                power: SupportedPower::new(TxPower::Calibrated).unwrap(),
                coex_operation: TxOperation::Data,
            }
            .admit(),
        );
        let now = harness.clock.now();
        harness
            .port
            .submit(attempt, now, &mut harness.sink)
            .ok()
            .unwrap();
        harness.service();
        let sent = harness.air.sent();
        let [first, .., retried] = sent.as_slice() else {
            panic!("the MPDU went out twice");
        };
        assert_eq!(first.packet_number, Some(1));
        assert_eq!(
            retried.packet_number, first.packet_number,
            "a retransmission keeps the packet number"
        );
        let retry = |frame: &[u8]| {
            oer_protocols_ieee80211_wire::frame::FrameControlFields::from_bytes(
                *frame.first_chunk::<2>().unwrap(),
            )
            .retry()
        };
        assert!(retry(&retried.frame), "a retransmission sets Retry");
        // Renewal returns fresh memory, which the next MPDU numbers anew.
        assert_eq!(harness.take_completed(), 1);
        let ticket = harness.tickets.pop().unwrap();
        let buffer = harness.small.pop().unwrap();
        let attempt = harness.admitted_under_number(ticket, buffer, &key, 2);
        let now = harness.clock.now();
        harness
            .port
            .submit(attempt, now, &mut harness.sink)
            .ok()
            .unwrap();
        harness.service();
        let renewed = harness.air.sent().pop().unwrap();
        assert_eq!(renewed.packet_number, Some(2));
        assert!(!retry(&renewed.frame), "a renewed MPDU is a new one");
    });
}

#[test]
fn a_response_the_checked_frame_does_not_solicit_quarantines_before_publication() {
    with_harness(|harness| {
        harness.start();
        // An individual data frame solicits an ACK, not a BlockAck.
        harness.air.answer(AirAnswer::BlockAck(1));
        let attempt = harness.mpdu().unwrap();
        let now = harness.clock.now();
        harness
            .port
            .submit(attempt, now, &mut harness.sink)
            .ok()
            .unwrap();
        let now = harness.clock.now();
        let fault = harness
            .port
            .service_tx(TxFacts::DUE, false, now, &mut harness.sink)
            .err()
            .unwrap();
        assert!(matches!(
            fault,
            ModelPortFault::Quarantined {
                call: ModelPortCall::ServiceTx,
                ..
            }
        ));
        assert!(harness.air.sent().is_empty(), "nothing was published");
        assert!(harness.sink.terminals.is_empty(), "the port keeps the MPDU");
    });
}

mod access_point;
