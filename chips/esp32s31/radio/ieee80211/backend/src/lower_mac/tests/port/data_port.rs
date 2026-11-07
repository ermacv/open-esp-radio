//! The S31 data port ([`Esp32s31Port`]) over the model device: the shared
//! data-port cases and the port's own turns, barriers, deadline, receive
//! room, lend credit across a reinstall and named faults.
use super::*;
use crate::mac::tx::TxBacking;
use crate::port::{PortDriver, PortInstallation};
use oer_protocols_ieee80211_contracts_port::arena::{
    Arena, ArenaFormat, ArenaPublication, TxArena,
};
use oer_protocols_ieee80211_testing_conformance::data_port::{
    self as cases, Case, DataPortHarness,
};
use oer_protocols_radio_port::{Installations, Minted};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

mod keys_receive;
mod lifecycle;
mod transmit;

type Port<'id, Id> = Esp32s31Port<'id, Id, TestDevice, 512, Draws>;
type Ticket<'id, Id> = radio::Slot<'id, Tickets, radio::Child<radio::Root<Id>, 2>>;
type TestBuffer<'id, Id> = Buffer<'id, Id, TestDevice, 512, Draws>;
type Configure<'id, Id> = ConfigurePermission<'id, Station, radio::Child<radio::Root<Id>, 8>, Tag>;

/// One MPDU's completions up to its delivery, and whether its last
/// publication carries the Retry bit: an ACK timeout sets it, a collision or a
/// CTS timeout does not, and a bit once set stays.
const RETRY_HISTORIES: [(&[(u8, u8)], bool); 12] = [
    (&[SUCCESS], false),
    (&[ACK_TIMEOUT, SUCCESS], true),
    (&[COLLISION, SUCCESS], false),
    (&[COLLISION_DETACH, SUCCESS], false),
    (&[CTS_TIMEOUT, SUCCESS], false),
    (&[COLLISION, CTS_TIMEOUT, SUCCESS], false),
    (&[ACK_TIMEOUT, COLLISION, SUCCESS], true),
    (&[ACK_TIMEOUT, COLLISION_DETACH, SUCCESS], true),
    (&[ACK_TIMEOUT, CTS_TIMEOUT, SUCCESS], true),
    (&[COLLISION, ACK_TIMEOUT, SUCCESS], true),
    (&[COLLISION_DETACH, ACK_TIMEOUT, SUCCESS], true),
    (&[CTS_TIMEOUT, ACK_TIMEOUT, SUCCESS], true),
];

/// The station's association and its group and pairwise key slots.
type AssociationParts<'id, Id> = (
    <PortKeys<Id, Station> as KeyTypes<'id>>::Association,
    <PortKeys<Id, Station> as KeyTypes<'id>>::GroupSlot,
    <PortKeys<Id, Station> as KeyTypes<'id>>::PairwiseSlot,
);

/// A waker that counts its wakes.
struct Count(AtomicUsize);
impl std::task::Wake for Count {
    fn wake(self: Arc<Self>) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }
    fn wake_by_ref(self: &Arc<Self>) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }
}

/// The turn's outbox: every terminal, TBTT and cut unit, in order.
struct Outbox<'id, Id: radio::SlotIdentity + 'id> {
    terminals: Vec<LowerMacTerminal<'id, Family<Id>>>,
    tbtt: usize,
    received: Vec<(
        ReceiveBuffer<'id, Id, TestReceive>,
        crate::lower_mac::Esp32s31RxMeta,
    )>,
}
impl<'id, Id: radio::SlotIdentity + 'id> PortSink<'id, Family<Id>> for Outbox<'id, Id> {
    fn terminal(&mut self, terminal: LowerMacTerminal<'id, Family<Id>>) {
        self.terminals.push(terminal);
    }
    fn tbtt(&mut self, _: TbttEvent) {
        self.tbtt += 1;
    }
    fn received(
        &mut self,
        frame: ReceiveBuffer<'id, Id, TestReceive>,
        meta: crate::lower_mac::Esp32s31RxMeta,
    ) {
        self.received.push((frame, meta));
    }
}

/// One installation's port and the tokens the actor and lifecycle hold.
struct Fixture<'id, Id: radio::SlotIdentity + 'id> {
    port: Port<'id, Id>,
    outbox: Outbox<'id, Id>,
    commands: Esp32s31Commands<'id, Id, TestDevice, 512, Draws>,
    observed: Rc<RefCell<Observed>>,
    timer: TestTimer,
    count: Arc<Count>,
    tickets: Vec<Ticket<'id, Id>>,
    buffers: Vec<TestBuffer<'id, Id>>,
    configure: Option<Configure<'id, Id>>,
    configured: bool,
    spare: Option<Arena>,
    released: Released,
    association: Option<
        crate::lower_mac::association::VacantStationAssociation<
            'id,
            radio::Child<radio::Root<Id>, 0>,
        >,
    >,
    group_permission:
        Option<radio::Slot<'id, GroupPermission<Station>, radio::Child<radio::Root<Id>, 5>>>,
    gate: Option<GateOpen<'id, radio::Child<radio::Root<Id>, 1>>>,
    /// The terminals' access, which opens a single MPDU's terminal.
    access: Option<TerminalAccess<'id, Tickets, radio::Child<radio::Root<Id>, 2>>>,
}

fn supported(
    channel: oer_protocols_ieee80211_wire::channel::WifiChannel,
) -> SupportedChannel<Caps> {
    SupportedChannel::new(
        oer_protocols_radio_regulatory::RegulatoryDomain::of(
            oer_protocols_radio_regulatory::Country::new(*b"DE").expect("test country"),
        )
        .expect("domain fixture")
        .permit(
            oer_protocols_ieee80211_contracts_port::RegulatedChannel::from(
                Channel::from_wifi_channel(channel),
            ),
        )
        .expect("permitted test channel"),
    )
    .expect("supported test channel")
}

impl<'id, Id: radio::SlotIdentity + 'id> Fixture<'id, Id> {
    fn new(
        installation: crate::port::PortInstallation<'id, Id, TestDevice, 512, Draws>,
        observed: Rc<RefCell<Observed>>,
        timer: TestTimer,
    ) -> Self {
        let crate::port::PortInstallation {
            port,
            commands,
            resources,
        } = installation;
        let tickets = resources.tickets.into_iter().collect();
        let buffers = resources.buffers.into_iter().collect();
        let configure = Some(
            resources
                .station_commands
                .configure
                .with_context(Tag(7, None)),
        );
        Self {
            port,
            outbox: Outbox {
                terminals: Vec::new(),
                tbtt: 0,
                received: Vec::new(),
            },
            commands,
            observed,
            timer,
            count: Arc::new(Count(AtomicUsize::new(0))),
            tickets,
            buffers,
            configure,
            configured: false,
            spare: None,
            released: Released::default(),
            association: Some(resources.station_association),
            group_permission: Some(resources.station_group_permission),
            gate: Some(resources.gate),
            access: Some(resources.terminal_access),
        }
    }

    fn now(&self) -> Instant {
        self.timer.now()
    }

    fn start(&mut self) {
        assert_eq!(cases::start(self), Ok(()));
    }

    /// Serve one turn's facts.
    fn turn(
        &mut self,
    ) -> Option<oer_protocols_ieee80211_contracts_port::PortCompleted<'id, Port<'id, Id>>> {
        let waker = Waker::from(self.count.clone());
        let now = self.now();
        let facts = self.port.poll_facts(&mut Context::from_waker(&waker));
        self.port
            .service_tx(
                facts.tx.unwrap_or(TxFacts::DUE),
                facts.tbtt,
                now,
                &mut self.outbox,
            )
            .expect("service")
    }

    /// Run the station's receive configuration to its terminal.
    fn configure_station(&mut self) {
        let command = DataPortHarness::configure(self).expect("permission");
        let now = self.now();
        assert!(matches!(
            self.port.command(command, now, &mut self.outbox),
            Ok(CommandProgress::Done)
        ));
        for _ in 0..8 {
            if DataPortHarness::take_configured(self) {
                self.configured = true;
                return;
            }
            self.turn();
        }
        panic!("the configuration ends");
    }

    fn frame(&self) -> TestReceive {
        TestReceive {
            bytes: crate::lower_mac::tests::data_frame(STATION),
            view: 0..26,
            released: self.released.clone(),
        }
    }
}

impl<'id, Id: radio::SlotIdentity + 'id> Fixture<'id, Id> {
    /// A station MPDU under `key`, or open, admitted by the port.
    fn mpdu(
        &mut self,
        key: Option<&PairwiseKey<'id, Id, Station>>,
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
            plan: RetryPlan::new(RetryStage {
                rate,
                transmissions: core::num::NonZeroU8::MIN,
                protection: Protection::None,
            }),
        })
        .ok()?;
        let source = TxSource::Fresh {
            buffer,
            frame,
            body: None,
            protection: match key {
                Some(key) => TxProtection::Ccmp(NumberedKey {
                    key,
                    packet_number: oer_protocols_ieee80211_wire::ccmp::CcmpPacketNumber::new(7)
                        .expect("PN"),
                }),
                None => TxProtection::Open,
            },
        }
        .check()
        .ok()?;
        let source =
            <Family<Id> as TxBufferPool<'id, BufferClass<512>>>::prepare::<Station, Caps>(source)
                .ok()?;
        let attempt = Attempt {
            context: Tag(91, None),
            ticket,
            source,
            access_category: oer_protocols_ieee80211_wire::qos::WmmAccessCategory::BestEffort,
            protection: Protection::None,
            power: SupportedPower::new(TxPower::Calibrated).expect("power"),
            coex_operation: TxOperation::Data,
        }
        .admit();
        Some(self.port.admit(attempt))
    }

    fn protected_mpdu(
        &mut self,
        key: &PairwiseKey<'id, Id, Station>,
    ) -> Option<<Port<'id, Id> as DataPort<'id>>::Admitted> {
        self.mpdu(Some(key))
    }
}

/// A QoS Data MPDU of 30 octets to the BSS.
fn write_frame(region: &mut [u8]) -> Option<u16> {
    let frame = region.get_mut(..30)?;
    frame.fill(0);
    frame[..26].copy_from_slice(&crate::lower_mac::tests::data_frame(BSSID));
    Some(30)
}

impl<'id, Id: radio::SlotIdentity + 'id> DataPortHarness<'id> for Fixture<'id, Id> {
    type Port = Port<'id, Id>;
    type Sink = Outbox<'id, Id>;

    fn parts(&mut self) -> (&mut Self::Port, &mut Self::Sink) {
        (&mut self.port, &mut self.outbox)
    }
    fn now(&self) -> Instant {
        self.timer.now()
    }
    fn advance(&mut self, by: oer_portable_time::Duration) {
        self.timer.0.advance(by).expect("advance");
    }
    fn waker(&self) -> Waker {
        Waker::from(self.count.clone())
    }
    fn wakes(&self) -> usize {
        self.count.0.load(Ordering::Relaxed)
    }
    fn channel(&self) -> SupportedChannel<Caps> {
        supported(channel(6))
    }
    fn settle(&mut self) {}
    fn deliver(&mut self, units: usize) {
        if !self.configured {
            self.configure_station();
        }
        for _ in 0..units {
            let frame = self.frame();
            self.observed.borrow_mut().received.push_back(frame);
        }
    }
    fn station_mpdu(&mut self) -> Option<<Self::Port as DataPort<'id>>::Admitted> {
        self.mpdu(None)
    }
    fn identity(attempt: &<Self::Port as DataPort<'id>>::Admitted) -> u8 {
        attempt.operation.ticket().index()
    }
    fn hold_air(&mut self, hold: bool) {
        self.observed.borrow_mut().hold_completion = hold;
    }
    fn configure(&mut self) -> Option<PortCommand<'id, Family<Id>, Caps>> {
        Some(PortCommand::Configure {
            permission: self.configure.take()?,
            receive: StationReceive::Bss {
                bssid: IndividualAddress::new(BSSID).expect("BSSID"),
                filter: SupportedFilter::new(ReceiveFilter::BSS_MEMBER).expect("filter"),
            },
        })
    }
    fn take_configured(&mut self) -> bool {
        let at = self
            .outbox
            .terminals
            .iter()
            .position(|terminal| matches!(terminal, LowerMacTerminal::Configured(_)));
        at.map(|at| self.outbox.terminals.remove(at)).is_some()
    }
    fn take_completed(&mut self) -> usize {
        let before = self.outbox.terminals.len();
        self.outbox.terminals.retain(|terminal| {
            !matches!(
                terminal,
                LowerMacTerminal::TxCompleted(TxCompleted::Single(_))
            )
        });
        before - self.outbox.terminals.len()
    }
    fn publish_held_arena(&mut self) -> bool {
        self.observed.borrow_mut().hold_completion = true;
        let now = self.now();
        let mut arena = self.port.arena();
        let Some([first, second]) = arena.arenas() else {
            return false;
        };
        let memory = crate::mac::tx::test_support::model_backing().expect("model memory");
        if arena.intake(&first, memory, write_frame).is_err() {
            return false;
        }
        let Some(rate) = arena.rate(PhyRate::Legacy(LegacyRate::Ofdm24M)) else {
            return false;
        };
        let publication = ArenaPublication {
            arena: first,
            format: ArenaFormat::Single(RetryPlan::new(RetryStage {
                rate,
                transmissions: core::num::NonZeroU8::MIN,
                protection: Protection::None,
            })),
            access_category: oer_protocols_ieee80211_wire::qos::WmmAccessCategory::BestEffort,
            protection: Protection::None,
            encryption: MpduProtection::Open,
            power: SupportedPower::new(TxPower::Calibrated).expect("power"),
        };
        let published = arena.publish(publication, now).is_ok();
        self.spare = Some(second);
        published
    }
    fn release_arena(&mut self) {
        self.observed.borrow_mut().hold_completion = false;
    }
    fn settle_arena(
        &mut self,
        done: oer_protocols_ieee80211_contracts_port::PortCompleted<'id, Self::Port>,
    ) {
        let mut arena = self.port.arena();
        let _right = arena.release(done, drop::<TxBacking<512>>).expect("settle");
    }
}

/// Run `test` on a fresh installed fixture of a local identity.
fn with_fixture<R>(test: impl for<'id> FnOnce(&mut Fixture<'id, radio::LocalId<'id>>) -> R) -> R {
    block_on(radio::with_local_slot(async move |claim| {
        let (parts, observed, timer) = installed(Vec::new());
        let mut installations = Installations::<_, InstallationPools>::claim(claim);
        let minted: Minted<_, InstallationPools> = installations.first().expect("first mint");
        let driver = Box::leak(Box::new(PortDriver::vacant()));
        let installation = Port::install(parts, minted, driver).ok().expect("install");
        let mut fixture = Fixture::new(installation, observed, timer);
        test(&mut fixture)
    }))
}

#[test]
fn every_data_port_case_passes_on_the_s31_port() {
    for case in Case::ALL {
        let result = with_fixture(|fixture| cases::check(fixture, case));
        assert_eq!(result, Ok(()), "{case:?}");
    }
}

/// Run `test` on a fresh fixture, with the installation authority beside it.
fn with_installations<R>(
    test: impl for<'id> FnOnce(
        Fixture<'id, radio::LocalId<'id>>,
        &mut Installations<radio::LocalId<'id>, InstallationPools>,
    ) -> R,
) -> R {
    block_on(radio::with_local_slot(async move |claim| {
        let (parts, observed, timer) = installed(Vec::new());
        let mut installations = Installations::<_, InstallationPools>::claim(claim);
        let minted: Minted<_, InstallationPools> = installations.first().expect("first mint");
        let driver = Box::leak(Box::new(PortDriver::vacant()));
        let installation = Port::install(parts, minted, driver).ok().expect("install");
        let fixture = Fixture::new(installation, observed, timer);
        test(fixture, &mut installations)
    }))
}

impl<'id, Id: radio::SlotIdentity + 'id> Fixture<'id, Id> {
    /// Run `command` until it is no longer held, serving a turn between.
    fn run(
        &mut self,
        command: PortCommand<'id, Family<Id>, Caps>,
    ) -> Result<CommandProgress<'id, Family<Id>, Caps>, Fault> {
        let mut command = command;
        for _ in 0..16 {
            let now = self.now();
            match self.port.command(command, now, &mut self.outbox)? {
                CommandProgress::Held(held) => command = held,
                other => return Ok(other),
            }
            if let Some(done) = self.turn() {
                self.settle_arena(done);
            }
        }
        Ok(CommandProgress::Held(command))
    }

    /// The station's association and its group and pairwise key slots.
    fn association(&mut self) -> AssociationParts<'id, Id> {
        let (start, bank) = self.association.take().expect("association").into_parts();
        let (pairwise, [group]) = bank.into_parts();
        let [pairwise] = pairwise;
        let association = start.begin(oer_protocols_ieee80211_wire::security::Spp::NotNegotiated);
        (association, group, pairwise)
    }

    /// Stop the port, serving its turns until it confirms.
    fn stop(&mut self) {
        for _ in 0..16 {
            let waker = self.waker();
            let now = self.now();
            match self.port.poll_stop(&mut Context::from_waker(&waker), now) {
                Poll::Ready(Ok(())) => return,
                Poll::Ready(Err(unconfirmed)) => panic!("stop unconfirmed: {unconfirmed:?}"),
                Poll::Pending => {
                    let _ = self.turn();
                }
            }
        }
        panic!("the stop ends");
    }
}

fn group_key(material: u8) -> oer_protocols_ieee80211_contracts_security::Ccmp128Key {
    oer_protocols_ieee80211_contracts_security::Ccmp128Key::new(&[material; 16])
}

fn key_id(id: u8) -> oer_protocols_ieee80211_wire::ccmp::GroupKeyId {
    oer_protocols_ieee80211_wire::ccmp::GroupKeyId::new(id).expect("KeyID")
}

#[test]
fn no_data_port_call_wakes_its_caller() {
    with_fixture(|fixture| {
        fixture.start();
        fixture.deliver(2);
        let attempt = fixture.station_mpdu().expect("attempt");
        let now = fixture.now();
        fixture
            .port
            .submit(attempt, now, &mut fixture.outbox)
            .ok()
            .expect("submitted");
        for _ in 0..4 {
            fixture.turn();
        }
        let now = fixture.now();
        let mut units = Vec::new();
        while let Ok(RxPull::Unit { frame, .. }) = fixture.port.receive(8, now) {
            units.push(frame);
        }
        assert_eq!(units.len(), 2);
        let lent = fixture
            .port
            .lend(units.pop().expect("unit"))
            .ok()
            .expect("lent");
        fixture.port.recycle(units.pop().expect("unit"));
        let color = BssColor::new(5).expect("color");
        assert!(matches!(
            fixture
                .port
                .command(PortCommand::SetBssColor(color), now, &mut fixture.outbox),
            Ok(CommandProgress::Done)
        ));
        let edca = SupportedEdca::new(EdcaParameterSet::STANDARD_DEFAULT).expect("EDCA");
        assert!(matches!(
            fixture
                .port
                .command(PortCommand::SetEdca(edca), now, &mut fixture.outbox),
            Ok(CommandProgress::Done)
        ));
        let waker = fixture.waker();
        let mut cx = Context::from_waker(&waker);
        assert!(fixture.port.poll_lend_room(&mut cx).is_ready());
        assert!(fixture.port.poll_lent_returned(&mut cx).is_pending());
        let _ = fixture.port.deadline();
        let _ = fixture.port.sample_clock();
        assert_eq!(
            fixture.wakes(),
            0,
            "no call before the return woke the caller"
        );
        drop(lent);
        // The network's return is the one wake, an edge the caller waits for.
        assert_eq!(fixture.wakes(), 1);
        fixture.stop();
        assert_eq!(fixture.wakes(), 1);
    });
}

#[test]
fn a_receive_pull_stops_at_the_held_units_and_keeps_the_rest_on_the_ring() {
    with_fixture(|fixture| {
        fixture.start();
        fixture.deliver(RX_HELD + 4);
        let now = fixture.now();
        let mut held = Vec::new();
        while let RxPull::Unit { frame, .. } = fixture.port.receive(32, now).expect("receive") {
            held.push(frame);
        }
        // The port gives at most RX_HELD units out at once.
        assert_eq!(held.len(), RX_HELD);
        assert_eq!(fixture.observed.borrow().received.len(), 4);
        fixture.port.recycle(held.pop().expect("unit"));
        assert!(matches!(
            fixture.port.receive(32, now),
            Ok(RxPull::Unit { .. })
        ));
        assert_eq!(fixture.observed.borrow().received.len(), 3);
        // A caller without room takes nothing and leaves the ring as it is.
        assert!(!matches!(
            fixture.port.receive(0, now),
            Ok(RxPull::Unit { .. })
        ));
        assert_eq!(fixture.observed.borrow().received.len(), 3);
        for frame in held {
            fixture.port.recycle(frame);
        }
    });
}

#[test]
fn a_served_receive_continuation_leaves_the_ports_deadline() {
    with_fixture(|fixture| {
        fixture.start();
        fixture.deliver(8);
        fixture.observed.borrow_mut().continuous_rx = true;
        let now = fixture.now();
        let mut held = Vec::new();
        let at = loop {
            match fixture.port.receive(32, now).expect("receive") {
                RxPull::Unit { frame, .. } => held.push(frame),
                RxPull::Later(at) => break at,
                RxPull::Empty => panic!("the ring continues later"),
            }
        };
        assert!(at > now);
        assert_eq!(fixture.port.deadline(), Some(at), "the alarm keeps it");
        // The caller comes back at the continuation and takes one unit.
        fixture.advance(at.saturating_duration_since(now));
        let now = fixture.now();
        assert!(matches!(
            fixture.port.receive(32, now),
            Ok(RxPull::Unit { .. })
        ));
        // The served continuation no longer bounds the port: a passed
        // deadline is work for the port's next service.
        assert!(
            fixture
                .port
                .deadline()
                .is_none_or(|deadline| deadline > now),
            "{:?} at {now:?}",
            fixture.port.deadline()
        );
        fixture.observed.borrow_mut().continuous_rx = false;
        for frame in held {
            fixture.port.recycle(frame);
        }
    });
}

#[test]
fn a_network_return_is_one_receive_fact() {
    with_fixture(|fixture| {
        fixture.start();
        fixture.deliver(1);
        let now = fixture.now();
        let RxPull::Unit { frame, .. } = fixture.port.receive(8, now).expect("receive") else {
            panic!("a unit");
        };
        let lent = fixture.port.lend(frame).ok().expect("lent");
        let waker = fixture.waker();
        let mut cx = Context::from_waker(&waker);
        assert!(!fixture.port.poll_facts(&mut cx).rx, "the ring is empty");
        drop(lent);
        assert!(fixture.port.poll_facts(&mut cx).rx, "the return rearms");
        // A caller that does not pull now hears the return once.
        assert!(!fixture.port.poll_facts(&mut cx).rx);
        assert_eq!(fixture.observed.borrow().returned, 1);
    });
}

#[test]
fn a_refused_submission_and_command_return_their_owners_unchanged() {
    with_fixture(|fixture| {
        let attempt = fixture.station_mpdu().expect("attempt");
        let now = fixture.now();
        let refused = fixture.port.submit(attempt, now, &mut fixture.outbox);
        assert_eq!(refused, Err(SubmitRefusal::NotEnabled));
        // The attempt ended withdrawn at once: its terminal returns its
        // owners.
        assert!(matches!(
            fixture.outbox.terminals.as_slice(),
            [LowerMacTerminal::TxCompleted(TxCompleted::Single(_))]
        ));
        assert_eq!(fixture.take_completed(), 1);
        let gate = fixture.gate.take().expect("gate");
        let refused = fixture
            .port
            .command(PortCommand::CloseGate(gate), now, &mut fixture.outbox);
        let Ok(CommandProgress::Refused(PortCommand::CloseGate(gate), CommandRefusal::NotEnabled)) =
            refused
        else {
            panic!("a stopped port refuses the gate with it");
        };
        assert!(fixture.outbox.terminals.is_empty());
        // A new attempt runs once the port does.
        fixture.start();
        let attempt = fixture.station_mpdu().expect("attempt");
        fixture
            .port
            .submit(attempt, now, &mut fixture.outbox)
            .ok()
            .expect("submitted");
        assert!(matches!(
            fixture.run(PortCommand::CloseGate(gate)),
            Ok(CommandProgress::Done)
        ));
        for _ in 0..8 {
            fixture.turn();
        }
        assert!(
            fixture
                .outbox
                .terminals
                .iter()
                .any(|terminal| matches!(terminal, LowerMacTerminal::GateClosed(_)))
        );
        assert_eq!(fixture.take_completed(), 1);
    });
}

#[test]
fn a_queued_mpdu_bounds_the_deadline_until_it_expires_behind_a_closed_gate() {
    with_fixture(|fixture| {
        fixture.start();
        let gate = fixture.gate.take().expect("gate");
        assert!(matches!(
            fixture.run(PortCommand::CloseGate(gate)),
            Ok(CommandProgress::Done)
        ));
        assert!(
            fixture
                .outbox
                .terminals
                .iter()
                .any(|terminal| matches!(terminal, LowerMacTerminal::GateClosed(_)))
        );
        let idle = fixture.port.deadline();
        let attempt = fixture.station_mpdu().expect("attempt");
        let admitted = fixture.now();
        fixture
            .port
            .submit(attempt, admitted, &mut fixture.outbox)
            .ok()
            .expect("queued");
        let lifetime = EdcaParameterSet::STANDARD_DEFAULT
            .access_category(oer_protocols_ieee80211_wire::qos::WmmAccessCategory::BestEffort)
            .msdu_lifetime;
        let expiry = admitted.saturating_add(lifetime);
        // The submission updated the cached deadline without a turn.
        assert_eq!(
            fixture.port.deadline(),
            Some(expiry.min(idle.unwrap_or(expiry)))
        );
        fixture.advance(lifetime);
        assert_eq!(fixture.now(), expiry);
        fixture.turn();
        assert_eq!(
            fixture.take_completed(),
            1,
            "the MPDU expired at its deadline"
        );
        assert_eq!(fixture.port.expired_msdus(), 1);
        // Its bound left the cached deadline with it.
        assert_eq!(fixture.port.deadline(), idle);
    });
}

#[test]
fn a_pairwise_removal_is_held_while_its_key_is_on_air_then_runs() {
    with_fixture(|fixture| {
        fixture.start();
        let (association, _, slot) = fixture.association();
        let (install, identity) = fixture.commands.install_pairwise(
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
        assert!(<PortKeys<_, Station> as KeyTypes<'_>>::pairwise_identity(&key) == identity);
        fixture.hold_air(true);
        let attempt = fixture.protected_mpdu(&key).expect("attempt");
        let now = fixture.now();
        fixture
            .port
            .submit(attempt, now, &mut fixture.outbox)
            .ok()
            .expect("published");
        let removal = PortCommand::RemovePairwise { association, key };
        let removal = match fixture.port.command(removal, now, &mut fixture.outbox) {
            Ok(CommandProgress::Held(removal)) => removal,
            _ => panic!("the removal waits for the published attempt"),
        };
        fixture.hold_air(false);
        assert!(matches!(fixture.run(removal), Ok(CommandProgress::Done)));
        assert_eq!(fixture.take_completed(), 1);
        assert!(matches!(
            fixture.outbox.terminals.pop(),
            Some(LowerMacTerminal::KeyRemoved(Removed::Station(
                RemovedKey::Pairwise { .. }
            )))
        ));
    });
}

#[test]
fn a_group_installation_delivers_its_cut_units_before_its_terminal() {
    with_fixture(|fixture| {
        fixture.start();
        fixture.configure_station();
        let (association, slot, _) = fixture.association();
        let permission = fixture.group_permission.take().expect("permission");
        for _ in 0..2 {
            let frame = fixture.frame();
            fixture.observed.borrow_mut().captured.push_back(frame);
        }
        let install =
            fixture
                .commands
                .install_group(&association, permission, slot, key_id(1), group_key(1));
        assert!(matches!(fixture.run(install), Ok(CommandProgress::Done)));
        // Both captured units came out ahead of the installation's terminal.
        assert_eq!(fixture.outbox.received.len(), 2);
        assert!(matches!(
            fixture.outbox.terminals.last(),
            Some(LowerMacTerminal::GroupKeyInstalled(
                InstalledGroup::Station { .. }
            ))
        ));
        assert_eq!(fixture.observed.borrow().resumed_cuts, 1);
        for (frame, _) in core::mem::take(&mut fixture.outbox.received) {
            fixture.port.recycle(frame);
        }
    });
}

#[test]
fn a_stalled_group_command_becomes_a_named_fault_that_keeps_the_command() {
    with_fixture(|fixture| {
        fixture.start();
        let (association, slot, _) = fixture.association();
        let permission = fixture.group_permission.take().expect("permission");
        fixture.observed.borrow_mut().hold_cut = true;
        let install =
            fixture
                .commands
                .install_group(&association, permission, slot, key_id(1), group_key(1));
        let now = fixture.now();
        let install = match fixture.port.command(install, now, &mut fixture.outbox) {
            Ok(CommandProgress::Held(install)) => install,
            _ => panic!("the cut holds the installation"),
        };
        // The held command bounds the deadline.
        let bound = now.saturating_add(COMMAND_BOUND);
        assert!(fixture.port.deadline().is_some_and(|at| at <= bound));
        fixture.advance(COMMAND_BOUND);
        let now = fixture.now();
        let fault = Fault::CommandExpired(
            oer_protocols_ieee80211_contracts_port::CommandKind::InstallGroup,
        );
        assert!(matches!(
            fixture.port.command(install, now, &mut fixture.outbox),
            Err(found) if found == fault
        ));
        assert_eq!(fixture.port.fault(), Some(fault));
        // Every later call answers with the same fault; nothing poisoned.
        assert!(matches!(
            fixture.port.service_tx(TxFacts::DUE, false, now, &mut fixture.outbox),
            Err(found) if found == fault
        ));
        assert!(matches!(fixture.port.receive(4, now), Err(found) if found == fault));
        let attempt = fixture.station_mpdu().expect("attempt");
        assert!(
            fixture
                .port
                .submit(attempt, now, &mut fixture.outbox)
                .is_err()
        );
        // The refused attempt's withdrawn terminal returns its owners.
        assert_eq!(fixture.take_completed(), 1);
        assert!(fixture.outbox.terminals.is_empty());
    });
}

#[test]
fn a_reinstall_keeps_the_lend_credit_and_a_late_return_rejoins_the_new_ring() {
    with_installations(|mut fixture, installations| {
        fixture.start();
        fixture.deliver(1);
        let now = fixture.now();
        let Ok(RxPull::Unit { frame, .. }) = fixture.port.receive(4, now) else {
            panic!("received unit");
        };
        let lent = fixture.port.lend(frame).ok().expect("lent");
        assert_eq!(fixture.port.lent(), 1);
        fixture.stop();
        let Fixture {
            mut port,
            observed,
            timer,
            buffers,
            ..
        } = fixture;
        // The fixture's transmit buffers go home before the retire checks.
        drop(buffers);
        let (commands, resources) = port
            .reinstall(installations, 18)
            .expect("reinstalled without waiting for the network");
        let installation = PortInstallation {
            port,
            commands,
            resources,
        };
        let mut second = Fixture::new(installation, observed, timer);
        assert_eq!(
            second.port.lent(),
            1,
            "the lend credit outlived the installation"
        );
        second.start();
        let rearmed = second.observed.borrow().returned;
        drop(lent);
        let now = second.now();
        let _ = second.port.receive(4, now).expect("receive");
        assert_eq!(second.port.lent(), 0);
        assert_eq!(
            second.observed.borrow().returned,
            rearmed + 1,
            "the unit rejoined the ring"
        );
        assert_eq!(second.observed.borrow().stops, 1);
    });
}

#[test]
fn a_running_port_refuses_its_reinstall_and_keeps_running() {
    with_installations(|mut fixture, installations| {
        fixture.start();
        assert_eq!(
            fixture.port.reinstall(installations, 18).err(),
            Some(crate::port::ReinstallRefused::NotRetired)
        );
        fixture.stop();
    });
}

#[test]
fn only_a_stopped_port_releases_its_device() {
    with_installations(|mut fixture, _| {
        fixture.start();
        assert!(fixture.port.release().is_none());
        fixture.stop();
        let Fixture {
            mut port, buffers, ..
        } = fixture;
        // The fixture's transmit buffers go home before the retire checks.
        drop(buffers);
        assert!(port.release().is_some());
    });
}
