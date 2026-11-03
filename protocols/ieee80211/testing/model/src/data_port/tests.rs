//! The model's data port: the shared conformance cases and the model's
//! own focused cases.

use super::*;
use alloc::sync::Arc;
use core::num::NonZeroU8;
use core::sync::atomic::{AtomicUsize, Ordering};
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

/// The terminals and frames the port put in its sink.
struct Sink<'s> {
    terminals: Vec<LowerMacTerminal<'s, Types<'s>>>,
    received: Vec<(ModelRxBuffer<'s, LocalId<'s>>, ModelRxMeta)>,
    tbtt: usize,
}
impl<'s> PortSink<'s, Types<'s>> for Sink<'s> {
    fn terminal(&mut self, terminal: LowerMacTerminal<'s, Types<'s>>) {
        self.terminals.push(terminal);
    }
    fn tbtt(&mut self, _: oer_protocols_ieee80211_contracts_port::TbttEvent) {
        self.tbtt += 1;
    }
    fn received(&mut self, frame: ModelRxBuffer<'s, LocalId<'s>>, meta: ModelRxMeta) {
        self.received.push((frame, meta));
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
            tickets,
            small,
            terminal_access,
            station_commands,
            station_association,
            station_group,
            station_group_permission,
            station_pairwise,
            gate,
            ..
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
        mut buffer: ModelBuffer<'s, B<'s>, ModelSmall>,
        key: Option<&ModelPairwiseKey<'s, B<'s>, Station>>,
    ) -> ModelAdmitted<LocalId<'s>, ModelCaps, ()> {
        buffer.bytes_mut().unwrap().fill(0);
        let rate = self
            .port
            .rate(PhyRate::Legacy(LegacyRate::Ofdm6M))
            .unwrap()
            .phase::<'s>();
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
                transmissions: NonZeroU8::new(2).unwrap(),
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
                    Some(key) => {
                        TxProtection::Ccmp(oer_protocols_ieee80211_contracts_port::NumberedKey {
                            key,
                            packet_number:
                                oer_protocols_ieee80211_wire::ccmp::CcmpPacketNumber::new(1)
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
            protection: Protection::None,
            encryption: MpduProtection::Open,
            power: SupportedPower::new(TxPower::Calibrated).unwrap(),
        };
        arena.publish(publication, now).is_ok()
    }
    fn release_arena(&mut self) {
        self.air.hold_completion(CATEGORY, false);
    }
    fn settle_arena(&mut self, done: ModelArenaCompleted) {
        let mut arena = self.port.arena();
        let right = arena.release(done, |_| {}).unwrap();
        self.arenas.push(right);
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
        let refused = harness.port.submit(attempt, now).err().unwrap();
        assert_eq!(refused.reason, SubmitRefusal::NotEnabled);
        assert_eq!(refused.attempt.operation.ticket().index(), ticket);
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
        assert_eq!(harness.air.commands(), 0);
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
        harness.port.submit(attempt, now).ok().unwrap();
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
        harness.port.submit(attempt, admitted).ok().unwrap();
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
        harness.port.submit(attempt, now).ok().unwrap();
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
        harness.port.submit(attempt, now).ok().unwrap();
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
fn a_quarantined_air_is_named_with_what_the_port_held() {
    with_harness(|harness| {
        harness.start();
        harness.air.hold_completion(CATEGORY, true);
        let attempt = harness.mpdu().unwrap();
        let now = harness.clock.now();
        harness.port.submit(attempt, now).ok().unwrap();
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
        harness.port.submit(attempt, now).ok().unwrap();
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
        harness.port.submit(attempt, now).ok().unwrap();
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
