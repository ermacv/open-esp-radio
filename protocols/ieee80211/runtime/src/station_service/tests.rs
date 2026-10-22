//! The service on the model, the test playing the access point: the
//! station's frames go at the rates its selection picks, and the control
//! record switches once the station associated, the handshake's EAPOL on
//! the association's record.

use core::cell::{Cell, RefCell};
use core::future::{Future, poll_fn};
use core::pin::Pin;
use core::task::{Poll, Waker};
use oer_ieee80211_sta::machine::{
    CommandRefusal, ConnectCommand, DisconnectCommand, ScanCommand, StaCommand, StaCommandCredit,
    StaCommandCredits, StaCommandOutcome, StaCommandPermission,
};

use embassy_futures::join::join;
use oer_espressif_ieee80211_policy::{
    rate_control::EspressifRateControl, rate_selection::EspressifRateSelection,
};
use oer_ieee80211_arbiter::ArbiterPolicy;
use oer_ieee80211_port::{Channel, ChannelWidth, LowerMacSetup, SignalMetadata};
use oer_ieee80211_rsn::{Pmk, PtkContext, frames::RsnTxFrame};
use oer_ieee80211_sta::machine::{GroupCapacity, StaConfig, StaSecurity, StationResources};
use oer_ieee80211_upper_mac::rate_control::RateControl;
use oer_ieee80211_wire::address::IndividualAddress;
use oer_radio_port::{Ended, LocalId, PortBody, SlotIdentity, run_port, with_local_slot};
use oer_time::Duration;
use oer_time_virtual::VirtualClock;
use std::boxed::Box;
use std::rc::Rc;
use std::vec::Vec;
use {
    oer_ieee80211_port::PhyMode,
    oer_ieee80211_wire::{
        phy::{LegacyRate, PhyRate},
        security::rsn::Akm,
        station::AssociationCapabilities,
    },
};

use super::*;

mod contracts;
use oer_ieee80211_model::{
    AirFrame, MODEL_RX_EVENTS, MODEL_TX_QUEUE_DEPTH, ModelAck, ModelAir, ModelAnyBuffer,
    ModelBackend, ModelCaps24, ModelLarge, ModelPhase, ModelResources, ModelRxMeta, ModelSetup,
    ModelSmall, ModelTypes,
};

const LOCAL: [u8; 6] = [0x02, 0, 0, 0x12, 0x34, 0x56];
const ACCESS_POINT_ADDRESS: [u8; 6] = [0x02, 0, 0, 0, 0, 0x99];
const BSSID: [u8; 6] = [0x30, 0x05, 0x5c, 0x11, 0x22, 0x33];
const SSID: &[u8] = b"open-net";
const SNONCE: [u8; 32] = [3; 32];
const ANONCE: [u8; 32] = [4; 32];
const PMK: [u8; 32] = [7; 32];
/// CCMP-128 with PSK.
const RSN: [u8; 22] = [
    0x30, 20, 1, 0, 0, 0x0f, 0xac, 4, 1, 0, 0, 0x0f, 0xac, 4, 1, 0, 0, 0x0f, 0xac, 2, 0, 0,
];

struct ServiceContexts;

impl oer_ieee80211_model::ModelContext for ServiceContexts {
    type Body = core::convert::Infallible;
    type OwnedContext<Id: SlotIdentity> =
        TxOwnedContext<ModelCaps24, ModelLarge, oer_ieee80211_model::ModelPool<Id, 4>>;
    type Context<'p, Id: SlotIdentity + 'p> =
        TxContext<'p, ModelCaps24, ModelLarge, oer_ieee80211_model::ModelPool<Id, 4>>;
    type OwnedStationConfigureContext<Id: SlotIdentity> = oer_radio_port::ContextChoice<
        (),
        oer_ieee80211_sta::epoch::EpochContext<oer_ieee80211_model::ModelPool<Id, 4>, ()>,
    >;
    type StationConfigureContext<'p, Id: SlotIdentity + 'p> =
        oer_ieee80211_sta::machine::StaConfigurationContext<
            'p,
            oer_ieee80211_model::ModelPool<Id, 4>,
        >;
    type OwnedAccessPointConfigureContext<Id: SlotIdentity> = ();
    type AccessPointConfigureContext<'p, Id: SlotIdentity + 'p> = ();
}

struct ServicePort<Id: SlotIdentity>(PhantomData<Id>);

impl<Id: SlotIdentity> StationPort for ServicePort<Id> {
    type Backend = ModelBackend<Id, ModelCaps24, ServiceContexts>;
    type Caps = ModelCaps24;
    type SlotId = Id;
    type Types<'p>
        = ModelTypes<Self::Backend>
    where
        Self: 'p,
        Id: 'p;
    type TerminalStorage<T> = TestTerminalStorage<T>;
    type Small = ModelSmall;
    type Large = ModelLarge;

    fn lend_phase<'a, 'p, 'r>(
        phase: &'r mut ModelPhase<'a, 'p, ModelCaps24, Self::Backend>,
    ) -> &'r mut impl LowerMacPhase<'a, 'p, Caps = ModelCaps24, Types = ModelTypes<Self::Backend>>
    where
        Self: 'p,
        Id: 'p,
        'p: 'a,
    {
        phase
    }
    fn terminal<'p>(
        terminal: LowerMacTerminal<'p, ModelTypes<Self::Backend>>,
    ) -> LowerMacTerminal<'p, ModelTypes<Self::Backend>>
    where
        Self: 'p,
        Id: 'p,
    {
        terminal
    }
    fn port_terminal<'p>(
        terminal: LowerMacTerminal<'p, ModelTypes<Self::Backend>>,
    ) -> LowerMacTerminal<'p, ModelTypes<Self::Backend>>
    where
        Self: 'p,
        Id: 'p,
    {
        terminal
    }
    fn tune<'a, 'p>(setup: &mut ModelSetup<'a, ModelCaps24>, channel: SupportedChannel<ModelCaps24>)
    where
        Self: 'p,
        Id: 'p,
        'p: 'a,
    {
        setup.set_channel(channel);
    }
    fn event<'p>(
        event: LowerMacEvent<'p, ModelTypes<Self::Backend>>,
    ) -> LowerMacEvent<'p, ModelTypes<Self::Backend>>
    where
        Self: 'p,
        Id: 'p,
    {
        event
    }
    fn tx_buffer<'p>(buffer: ModelAnyBuffer<'p, Self::Backend>) -> StationTxBuffer<'p, Self>
    where
        Self: 'p,
        Id: 'p,
    {
        match buffer {
            ModelAnyBuffer::Small(buffer) => StationTxBuffer::Small(ReturnedMpdu::Fresh(buffer)),
            ModelAnyBuffer::SmallSent(buffer) => StationTxBuffer::Small(ReturnedMpdu::Sent(buffer)),
            ModelAnyBuffer::Large(buffer) => StationTxBuffer::Large(ReturnedMpdu::Fresh(buffer)),
            ModelAnyBuffer::LargeSent(buffer) => StationTxBuffer::Large(ReturnedMpdu::Sent(buffer)),
        }
    }
}

// Fault injection violates the storage contract, never manufactures terminal
// owners. Each test runs on its own thread; normal storage uses its full bound.
std::thread_local! { static CUSTODY_LIMIT: Cell<Option<usize>> = const { Cell::new(None) }; }
struct TestTerminalStorage<T> {
    values: Box<heapless::Deque<T, { oer_ieee80211_model::MODEL_TERMINALS }>>,
    limit: usize,
}
impl<T> TerminalStorage<T> for TestTerminalStorage<T> {
    const CAPACITY: usize = oer_ieee80211_model::MODEL_TERMINALS;
    fn empty() -> Self {
        Self {
            values: Box::new(heapless::Deque::new()),
            limit: CUSTODY_LIMIT.get().unwrap_or(Self::CAPACITY),
        }
    }
    fn try_push(&mut self, terminal: T) -> Result<(), T> {
        if self.values.len() == self.limit {
            Err(terminal)
        } else {
            self.values.push_back(terminal)
        }
    }
    fn pop(&mut self) -> Option<T> {
        self.values.pop_front()
    }
}

static CAPABILITIES: AssociationCapabilities = AssociationCapabilities {
    ht20: [0; 28],
    ht40: [0; 28],
    he20_ht: [0; 28],
    he20: [0; 24],
    he20_extended: [0; 14],
    wmm: [0; 9],
};

fn nonce() -> [u8; 32] {
    SNONCE
}

fn channel6() -> Channel {
    Channel::ghz2_4(6, ChannelWidth::Mhz20).unwrap()
}

type TestConfig = (StaConfig<ModelCaps24>, StaSecurity);

fn config(security: StaSecurity) -> TestConfig {
    let channels = heapless::Vec::from_slice(&[SupportedChannel::new(
        oer_radio_regulatory::TEST_DOMAIN
            .permit(channel6())
            .unwrap(),
    )
    .unwrap()])
    .unwrap();
    (
        StaConfig {
            address: oer_ieee80211_wire::address::OwnAddress::new(
                oer_ieee80211_wire::address::IndividualAddress::new(LOCAL).unwrap(),
            ),
            ssid: heapless::Vec::from_slice(SSID).unwrap(),
            channels,
            dwell: Duration::from_millis(10),
            skipped_absences: 3,
            access_point_margin_db: 6,
            supported_rates: [0x82, 0x84, 0x8b, 0x96, 0x0c, 0x12, 0x18, 0x24],
            association: &CAPABILITIES,
            listen_interval: oer_ieee80211_sta::request::StationListenInterval::new(10).unwrap(),
            phy: PhyMode::Legacy,
            nonce,
            he_packet_padding: |_| oer_ieee80211_port::backend_protection::HePacketPadding::None,
        },
        security,
    )
}

/// A beacon of the network on channel 6, protected or not.
fn beacon(protected: bool) -> Vec<u8> {
    let mut frame = std::vec![0_u8; 55];
    frame[0..2].copy_from_slice(&0x0080_u16.to_le_bytes());
    frame[4..10].copy_from_slice(&[0xff; 6]);
    frame[10..16].copy_from_slice(&BSSID);
    frame[16..22].copy_from_slice(&BSSID);
    frame[32..34].copy_from_slice(&100_u16.to_le_bytes());
    let capability: u16 = if protected { 0x0011 } else { 0x0001 };
    frame[34..36].copy_from_slice(&capability.to_le_bytes());
    frame[37] = SSID.len() as u8;
    frame[38..46].copy_from_slice(SSID);
    frame[46] = 1;
    frame[47] = 4;
    frame[48..52].copy_from_slice(&[0x82, 0x84, 0x8b, 0x96]);
    frame[52] = 3;
    frame[53] = 1;
    frame[54] = 6;
    if protected {
        frame.extend_from_slice(&RSN);
    }
    frame
}

fn authentication_response(request: &[u8]) -> Option<[u8; 30]> {
    let (request, _) = request.split_first_chunk::<30>()?;
    if u16::from_le_bytes([request[0], request[1]]) & !0x0800 != 0x00b0
        || request[4..10] != BSSID
        || request[10..16] != LOCAL
        || request[16..22] != BSSID
        || request[24..30] != [0, 0, 1, 0, 0, 0]
    {
        return None;
    }
    let mut frame = [0_u8; 30];
    frame[0..2].copy_from_slice(&0x00b0_u16.to_le_bytes());
    frame[4..10].copy_from_slice(&LOCAL);
    frame[10..16].copy_from_slice(&BSSID);
    frame[16..22].copy_from_slice(&BSSID);
    frame[26..28].copy_from_slice(&2_u16.to_le_bytes());
    Some(frame)
}

fn association_response(protected: bool) -> [u8; 30] {
    let mut frame = [0_u8; 30];
    frame[0..2].copy_from_slice(&0x0010_u16.to_le_bytes());
    frame[4..10].copy_from_slice(&LOCAL);
    frame[10..16].copy_from_slice(&BSSID);
    frame[16..22].copy_from_slice(&BSSID);
    let capability: u16 = if protected { 0x0431 } else { 0x0421 };
    frame[24..26].copy_from_slice(&capability.to_le_bytes());
    frame[28..30].copy_from_slice(&0xc02a_u16.to_le_bytes());
    frame
}

/// An EAPOL frame from the access point, in a data frame from the DS.
fn eapol_frame(eapol: &[u8]) -> Vec<u8> {
    let mut frame = std::vec![0_u8; 24];
    frame[0..2].copy_from_slice(&0x0208_u16.to_le_bytes());
    frame[4..10].copy_from_slice(&LOCAL);
    frame[10..16].copy_from_slice(&BSSID);
    frame[16..22].copy_from_slice(&BSSID);
    frame.extend_from_slice(&[0xaa, 0xaa, 0x03, 0, 0, 0, 0x88, 0x8e]);
    frame.extend_from_slice(eapol);
    frame
}

/// Group Message 1 of key `key_id` with replay counter `replay`.
fn group_message1(ptk: &oer_ieee80211_rsn::Ptk, replay: u64, key_id: u8) -> Vec<u8> {
    let mut plain = Vec::new();
    plain.extend_from_slice(&[0xdd, 22, 0x00, 0x0f, 0xac, 0x01, key_id, 0x00]);
    plain.extend_from_slice(&[0x77; 16]);
    plain.extend_from_slice(&[0xdd, 0, 0, 0, 0, 0, 0, 0]);
    let wrapped = oer_ieee80211_cipher::aes::software_aes128_key_wrap(ptk.kek(), &plain).unwrap();
    let message =
        RsnTxFrame::<512>::group_message1(Akm::Psk, LOCAL, replay, [0; 8], wrapped.as_bytes())
            .unwrap()
            .authenticate(ptk);
    eapol_frame(message.as_bytes())
}

fn protected_group_message1(ptk: &oer_ieee80211_rsn::Ptk, replay: u64, key_id: u8) -> Vec<u8> {
    use oer_ieee80211_wire::ccmp::{CcmpHeader, CcmpKeyId, CcmpPacketNumber};
    let mut frame = group_message1(ptk, replay, key_id);
    frame[1] |= 0x40;
    let header = CcmpHeader::new(
        CcmpPacketNumber::new(replay).unwrap(),
        CcmpKeyId::new(0).unwrap(),
    );
    frame.splice(24..24, header.encode());
    frame
}

/// What the port reported of a frame the access point sent: its signal
/// over the noise floor, 55 dB.
fn meta() -> ModelRxMeta {
    ModelRxMeta::new(channel6(), -40, -95)
}

/// The Frame Control subtype bits of a frame.
fn kind(frame: &AirFrame) -> u16 {
    u16::from_le_bytes([frame.plaintext[0], frame.plaintext[1]]) & 0x00fc
}

const PROBE_REQUEST: u16 = 0x0040;
const AUTHENTICATION: u16 = 0x00b0;
const ASSOCIATION_REQUEST: u16 = 0x0000;
const DEAUTHENTICATION: u16 = 0x00c0;
const DATA: u16 = 0x0008;

/// Let the port's other futures run once.
async fn yield_now() {
    let mut yielded = false;
    poll_fn(|cx| {
        if yielded {
            Poll::Ready(())
        } else {
            yielded = true;
            cx.waker().wake_by_ref();
            Poll::Pending
        }
    })
    .await;
}

/// Poll with a test-owned watchdog clock. Each pending poll advances it by
/// one millisecond; the model's clock remains under the scenario's control.
fn block_on<F: Future>(future: F) -> F::Output {
    use oer_time::Timer as _;

    let watchdog = VirtualClock::<1>::new();
    let mut expired = core::pin::pin!(watchdog.wait_for(Duration::from_secs(30)));
    let mut future = core::pin::pin!(future);
    let mut context = core::task::Context::from_waker(Waker::noop());
    loop {
        if let Poll::Ready(value) = future.as_mut().poll(&mut context) {
            return value;
        }
        assert!(
            expired.as_mut().poll(&mut context).is_pending(),
            "the test never ended"
        );
        watchdog
            .advance(Duration::from_millis(1))
            .expect("watchdog time fits");
    }
}

/// The world the test plays: the air, the clock, the application's ends.
#[derive(Clone, Copy)]
enum ExpectedStreamEnd {
    Drained,
    SubmissionFailed,
    FalseDrained,
    Quarantined,
}

struct World<'b> {
    air: Rc<ModelAir>,
    clock: std::rc::Rc<VirtualClock>,
    outputs: RefCell<Vec<ObservedOutput>>,
    completions: RefCell<Vec<FrameRates>>,
    refuse_rate: Cell<bool>,
    fail_submission: Cell<bool>,
    counters: Cell<Option<ServiceCounters>>,
    disables_at_body_end: Cell<Option<u32>>,
    expect_queued_command_on_poison: Cell<bool>,
    expected_stream_end: Cell<Option<ExpectedStreamEnd>>,
    drop_stopped_service: Cell<bool>,
    restart_stopped_service: Cell<Option<bool>>,
    /// The network stack's end of the transmit queue.
    stack: RefCell<MsduSender<'b, TxMsdu, 2>>,
    /// The network stack's end of the receive queue.
    delivered: RefCell<MsduReceiver<'b, EthernetFrame, 2>>,
}

/// Observe the operation class supplied by the service, independently of
/// the transmitted bytes and the order in which the model ends queues.
struct RecordingRates<'a> {
    inner: EspressifRateSelection<ModelCaps24>,
    completions: &'a RefCell<Vec<FrameRates>>,
    refuse_rate: &'a Cell<bool>,
    fail_submission: &'a Cell<bool>,
    air: &'a ModelAir,
}

impl RateSelection<ModelRxMeta, ModelAck> for RecordingRates<'_> {
    fn packet_number_step(&self) -> oer_ieee80211_wire::ccmp::CcmpPacketNumberStep {
        oer_ieee80211_wire::ccmp::CcmpPacketNumberStep::ONE
    }

    fn plan<R>(
        &self,
        frame: FrameRates,
        carried: impl Fn(PhyRate) -> Option<R>,
    ) -> Option<oer_ieee80211_port::RetryPlan<R>> {
        if self.refuse_rate.replace(false) {
            return None;
        }
        // Inject the fault between preparation and transfer, while earlier
        // attempts still await their terminals. The submission retains owners.
        if self.fail_submission.replace(false) {
            self.air.fail_after_terminals();
        }
        <EspressifRateSelection<ModelCaps24> as RateSelection<ModelRxMeta, ModelAck>>::plan(
            &self.inner,
            frame,
            carried,
        )
    }

    fn associated(
        &mut self,
        peer: &oer_ieee80211_upper_mac::rate_control::RatePeer,
        response: &ModelRxMeta,
    ) {
        <EspressifRateSelection<ModelCaps24> as RateSelection<ModelRxMeta, ModelAck>>::associated(
            &mut self.inner,
            peer,
            response,
        );
    }

    fn disconnected(&mut self) {
        <EspressifRateSelection<ModelCaps24> as RateSelection<ModelRxMeta, ModelAck>>::disconnected(
            &mut self.inner,
        );
    }

    fn completed<D>(
        &mut self,
        now: Instant,
        frame: FrameRates,
        completion: &oer_ieee80211_port::SingleOutcome<ModelAck, D>,
    ) {
        self.completions.borrow_mut().push(frame);
        <EspressifRateSelection<ModelCaps24> as RateSelection<ModelRxMeta, ModelAck>>::completed(
            &mut self.inner,
            now,
            frame,
            completion,
        );
    }
}

type CommandPath<Id> = oer_radio_port::Child<oer_ieee80211_model::ModelPool<Id, 9>, 1>;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ObservedRequest {
    correlation: u64,
    command: StaCommand,
}
impl ObservedRequest {
    fn of<Id: SlotIdentity>(request: &StaRequest<'_, CommandPath<Id>>) -> Self {
        Self {
            correlation: request.correlation(),
            command: request.command(),
        }
    }
    const fn correlation(&self) -> u64 {
        self.correlation
    }
    const fn command(&self) -> StaCommand {
        self.command
    }
}
#[derive(Debug)]
struct ObservedCommandRefused {
    request: ObservedRequest,
    reason: CommandRefusal,
}
#[derive(Debug)]
struct ObservedCommandTerminal {
    request: ObservedRequest,
    outcome: StaCommandOutcome,
}
enum ObservedOutput {
    RxFrameDropped(oer_ieee80211_sta::machine::StaRxError),
    MsduRefused {
        packet: TxMsdu,
        reason: DataRefusal,
    },
    RxMsduRefused(FrameTooLong),
    TxRefused {
        operation: TxOperation,
        reason: TxRefusal,
    },
    Notice(StaNotice),
    CommandRefused(ObservedCommandRefused),
    CommandCompleted(ObservedCommandTerminal),
    AccessPoint(AccessPointCommand<ModelCaps24>),
}

struct CommandCredits<'id, Id: SlotIdentity + 'id> {
    connect: Option<StaCommandCredit<'id, CommandPath<Id>, ConnectCommand>>,
    scan: Option<StaCommandCredit<'id, CommandPath<Id>, ScanCommand>>,
    disconnect: Option<StaCommandCredit<'id, CommandPath<Id>, DisconnectCommand>>,
}
impl<'id, Id: SlotIdentity + 'id> CommandCredits<'id, Id> {
    fn new(credits: StaCommandCredits<'id, CommandPath<Id>>) -> Self {
        Self {
            connect: Some(credits.connect),
            scan: Some(credits.scan),
            disconnect: Some(credits.disconnect),
        }
    }
    fn take(&mut self, kind: StaCommand) -> StaCommandPermission<'id, CommandPath<Id>> {
        match kind {
            StaCommand::Connect => self
                .connect
                .take()
                .expect("application owns connect credit")
                .into(),
            StaCommand::BackgroundScan => self
                .scan
                .take()
                .expect("application owns scan credit")
                .into(),
            StaCommand::Disconnect => self
                .disconnect
                .take()
                .expect("application owns disconnect credit")
                .into(),
        }
    }
    fn restore(&mut self, permission: StaCommandPermission<'id, CommandPath<Id>>) {
        match permission {
            StaCommandPermission::Connect(credit) => {
                assert!(self.connect.replace(credit).is_none())
            }
            StaCommandPermission::BackgroundScan(credit) => {
                assert!(self.scan.replace(credit).is_none())
            }
            StaCommandPermission::Disconnect(credit) => {
                assert!(self.disconnect.replace(credit).is_none())
            }
        }
    }
}

/// Command transport and credits remain inside the installation which minted them.
/// The outer world's observations own no command permission.
struct Application<'a, 'b, 'id, Id: SlotIdentity + 'id> {
    world: &'a World<'b>,
    commands: Cell<Option<MsduSender<'a, ServiceCommand<'id, ModelCaps24, CommandPath<Id>>, 4>>>,
    credits: RefCell<CommandCredits<'id, Id>>,
}
impl<'b, Id: SlotIdentity> core::ops::Deref for Application<'_, 'b, '_, Id> {
    type Target = World<'b>;
    fn deref(&self) -> &Self::Target {
        self.world
    }
}

impl<'id, Id: SlotIdentity + 'id> Application<'_, '_, 'id, Id> {
    async fn station_command(&self, correlation: u64, command: StaCommand) {
        let permission = self.credits.borrow_mut().take(command);
        self.command(ServiceCommand::Station(StaRequest::new(
            correlation,
            permission,
        )))
        .await;
    }
    fn record_output(&self, output: ServiceOutput<'id, ModelCaps24, CommandPath<Id>>) {
        let observed = match output {
            ServiceOutput::RxFrameDropped(error) => ObservedOutput::RxFrameDropped(error),
            ServiceOutput::MsduRefused { packet, reason } => {
                ObservedOutput::MsduRefused { packet, reason }
            }
            ServiceOutput::RxMsduRefused(error) => ObservedOutput::RxMsduRefused(error),
            ServiceOutput::TxRefused { operation, reason } => {
                ObservedOutput::TxRefused { operation, reason }
            }
            ServiceOutput::Notice(notice) => ObservedOutput::Notice(notice),
            ServiceOutput::AccessPoint(command) => ObservedOutput::AccessPoint(command),
            ServiceOutput::CommandRefused(refused) => {
                let request = ObservedRequest::of(&refused.request);
                self.credits
                    .borrow_mut()
                    .restore(refused.request.into_parts().1);
                ObservedOutput::CommandRefused(ObservedCommandRefused {
                    request,
                    reason: refused.reason,
                })
            }
            ServiceOutput::CommandCompleted(terminal) => {
                let request = ObservedRequest::of(&terminal.request);
                self.credits
                    .borrow_mut()
                    .restore(terminal.request.into_parts().1);
                ObservedOutput::CommandCompleted(ObservedCommandTerminal {
                    request,
                    outcome: terminal.outcome,
                })
            }
        };
        self.outputs.borrow_mut().push(observed);
    }

    /// The next frame of `subtype` the station sent after the first
    /// `seen`, advancing the clock to the machines' deadlines while it
    /// waits.
    async fn sent(&self, seen: &mut usize, subtype: u16) -> AirFrame {
        for _ in 0..1000 {
            for _ in 0..20 {
                yield_now().await;
            }
            let sent = self.air.sent();
            while *seen < sent.len() {
                let frame = sent[*seen].clone();
                *seen += 1;
                if kind(&frame) == subtype {
                    return frame;
                }
            }
            self.clock.advance_to_next_deadline();
        }
        panic!("the station never sent {subtype:#06x}");
    }

    /// Wait until the service told the application `notice`.
    async fn notice(&self, notice: StaNotice) {
        for _ in 0..1000 {
            yield_now().await;
            if self
                .outputs
                .borrow()
                .iter()
                .any(|output| matches!(output, ObservedOutput::Notice(seen) if *seen == notice))
            {
                return;
            }
        }
        panic!("no {notice:?}");
    }

    /// The Ethernet frames the network stack received, once the service
    /// had the chance to deliver what the air holds.
    async fn delivered(&self) -> Vec<Vec<u8>> {
        for _ in 0..50 {
            yield_now().await;
        }
        let delivered = self.delivered.borrow();
        let mut frames = Vec::new();
        while let Some(msdu) = delivered.try_receive() {
            frames.push(msdu.bytes().collect());
        }
        frames
    }

    /// Begin the 4-way handshake after association and return the derived
    /// key once the station has answered Message 1 with Message 2.
    async fn start_handshake(&self, seen: &mut usize) -> oer_ieee80211_rsn::Ptk {
        let message1 = RsnTxFrame::<512>::message1(Akm::Psk, LOCAL, 1, ANONCE).unwrap();
        self.air.receive(&eapol_frame(message1.as_bytes()), meta());
        let message2 = self.sent(seen, DATA).await;
        assert_eq!(message2.rate, dsss1());
        Pmk::from_bytes(PMK).derive_ptk(
            Akm::Psk,
            PtkContext {
                authenticator_address: BSSID,
                supplicant_address: LOCAL,
                authenticator_nonce: ANONCE,
                supplicant_nonce: SNONCE,
            },
        )
    }

    async fn handshake(&self, seen: &mut usize) -> oer_ieee80211_rsn::Ptk {
        let ptk = self.start_handshake(seen).await;
        self.air.receive(&message3(&ptk), meta());
        // Message 4: the association's record, and unprotected.
        let message4 = self.sent(seen, DATA).await;
        assert_eq!(message4.rate, dsss1());
        assert_eq!(message4.key, None);
        assert_eq!(message4.plaintext[1] & 0x40, 0);
        self.notice(StaNotice::Connected {
            bssid: oer_ieee80211_wire::address::Bssid::new(
                oer_ieee80211_wire::address::IndividualAddress::new(BSSID).unwrap(),
            )
            .unwrap(),
            channel: 6,
        })
        .await;
        ptk
    }

    async fn command(&self, command: ServiceCommand<'id, ModelCaps24, CommandPath<Id>>) {
        let mut sender = self
            .commands
            .take()
            .expect("one application script owns the sender");
        sender.ready().await.send(command);
        self.commands.set(Some(sender));
    }

    /// The network stack sends one Ethernet frame to `destination`.
    fn send_msdu(&self, destination: [u8; 6]) {
        assert!(self.try_send_msdu(destination), "the queue has room");
    }

    /// The network stack sends one Ethernet frame to `destination` if its
    /// queue has room.
    fn try_send_msdu(&self, destination: [u8; 6]) -> bool {
        self.try_send_msdu_at(destination, 0)
    }

    /// The network stack sends one Ethernet frame of user `priority` to
    /// `destination` if its queue has room.
    fn try_send_msdu_at(&self, destination: [u8; 6], priority: u8) -> bool {
        let packet = TxMsdu {
            priority: WmmUserPriority::new(priority).unwrap(),
            frame: EthernetFrame::from_parts(oer_ieee80211_wire::data::EthernetFrameParts {
                destination,
                source: LOCAL,
                ether_type: 0x0800,
                payload: &[0xab; 100],
            })
            .unwrap(),
        };
        self.stack.borrow().try_send(packet).is_ok()
    }

    /// Scan, authenticate and associate: the probe, the Authentication and
    /// the Association Request as they went.
    async fn associate(&self, seen: &mut usize, protected: bool) -> [AirFrame; 3] {
        self.associate_with(seen, beacon(protected), protected)
            .await
    }

    /// [`Self::associate`] after `beacon`.
    async fn associate_with(
        &self,
        seen: &mut usize,
        beacon: Vec<u8>,
        protected: bool,
    ) -> [AirFrame; 3] {
        self.station_command(1, StaCommand::Connect).await;
        let probe = self.sent(seen, PROBE_REQUEST).await;
        self.air.receive(&beacon, meta());
        let authentication = self.sent(seen, AUTHENTICATION).await;
        let response = authentication_response(&authentication.plaintext)
            .expect("the test AP requires a valid wire Authentication Request");
        self.air.receive(&response, meta());
        let association = self.sent(seen, ASSOCIATION_REQUEST).await;
        self.air.receive(&association_response(protected), meta());
        [probe, authentication, association]
    }
}

/// The body that runs the service beside the test's script.
struct Body<'w, 'b, S, const GROUPS: usize> {
    world: &'w World<'b>,
    transmit: MsduReceiver<'b, TxMsdu, 2>,
    receive: MsduSender<'b, EthernetFrame, 2>,
    config: TestConfig,
    script: S,
    refuse_initial_enable: bool,
    placed: bool,
}

/// A test's script: the access point and the application, on the world.
type Script<'w> = Pin<Box<dyn Future<Output = ()> + 'w>>;

impl<'scope, 'w, 'b, S, const GROUPS: usize>
    PortBody<LocalId<'scope>, ModelBackend<LocalId<'scope>, ModelCaps24, ServiceContexts>>
    for Body<'w, 'b, S, GROUPS>
where
    S: for<'a, 'id> FnOnce(&'a Application<'a, 'b, 'id, LocalId<'scope>>) -> Script<'a>,
{
    type Output = ();
    async fn run<'p, 'id>(
        self,
        owner: Owner<
            'p,
            'id,
            LocalId<'scope>,
            ModelBackend<LocalId<'scope>, ModelCaps24, ServiceContexts>,
            Disabled,
        >,
        resources: ModelResources<'id, ModelBackend<LocalId<'scope>, ModelCaps24, ServiceContexts>>,
    ) -> Ended<LocalId<'scope>, ModelBackend<LocalId<'scope>, ModelCaps24, ServiceContexts>, ()>
    where
        ModelBackend<LocalId<'scope>, ModelCaps24, ServiceContexts>: 'id,
        LocalId<'scope>: 'id,
        'id: 'p,
    {
        {
            let ModelResources {
                connection_epoch,
                station_commands,
                terminal_access,
                tickets,
                small,
                large,
                station_association,
                station_pairwise,
                station_group,
                station_group_permission,
                gate,
                arbiter_authority,
                ..
            } = resources;
            let mut tickets = tickets.into_iter();
            let station_tickets: [_; 4] = core::array::from_fn(|_| tickets.next().unwrap());
            let data_tickets: [_; 4] = core::array::from_fn(|_| tickets.next().unwrap());
            let small: [_; 4] = small.try_into().ok().unwrap();
            let large: [_; 4] = large.try_into().ok().unwrap();

            let channel = SupportedChannel::new(
                oer_radio_regulatory::TEST_DOMAIN
                    .permit(channel6())
                    .unwrap(),
            )
            .unwrap();
            let (arbiter, place, absence) =
                PortArbiter::<ServicePort<LocalId<'scope>>>::new(
                    ArbiterPolicy::DEFAULT.checked(
                        <ModelBackend<LocalId<'scope>, ModelCaps24, ServiceContexts> as LowerMacBackend<LocalId<'scope>>>::PUBLICATION_BOUND
                    ).unwrap(),
                    channel,
                    arbiter_authority,
                    gate,
                );
            let (config, security) = self.config;
            let resources = StationResources::<_, 4, 4, GROUPS> {
                configure: station_commands.configure,
                epochs: oer_ieee80211_sta::epoch::LinkEpochSource::new(connection_epoch),
                tickets: station_tickets,
                buffers: small,
                place,
                absence_place: absence,
                keys: oer_ieee80211_port::association::reunite_vacant(
                    station_association,
                    oer_ieee80211_port::association::VacantKeyBank::new(
                        station_pairwise,
                        station_group,
                    ),
                ),
                group_permission: station_group_permission,
            };
            let (mut station, credits) = match security {
                StaSecurity::Open => Station::new_open(config, resources),
                StaSecurity::Personal { password, pmk } => {
                    Station::new_personal(config, password, pmk, resources, GroupCapacity::new())
                }
            };
            let mut credits = CommandCredits::new(credits);
            if self.refuse_initial_enable {
                station
                    .command(
                        Instant::EPOCH,
                        StaRequest::new(901, credits.take(StaCommand::Connect)),
                    )
                    .unwrap();
                self.world.air.refuse_enable();
            }
            let service = StationService::<ServicePort<LocalId<'scope>>, _, GROUPS>::new(
                station,
                StaData::new(data_tickets, large, MODEL_TX_QUEUE_DEPTH),
                arbiter,
                RecordingRates {
                    inner: EspressifRateSelection::<ModelCaps24>::new(),
                    completions: &self.world.completions,
                    refuse_rate: &self.world.refuse_rate,
                    fail_submission: &self.world.fail_submission,
                    air: &self.world.air,
                },
                terminal_access,
            );
            let mut command_queue = MsduQueue::<
                ServiceCommand<'id, ModelCaps24, CommandPath<LocalId<'scope>>>,
                4,
            >::new();
            let (application, commands) = command_queue.split();
            let application = Application {
                world: self.world,
                commands: Cell::new(Some(application)),
                credits: RefCell::new(credits),
            };
            let world = &application;
            let mut output = |output| world.record_output(output);
            let mut io = ServiceIo {
                commands,
                transmit: self.transmit,
                receive: self.receive,
                output: &mut output,
            };
            let script = self.script;
            let script = async move {
                script(world).await;
                world.command(ServiceCommand::Stop).await;
            };
            let completed = if self.placed {
                let mut service_storage = core::mem::MaybeUninit::uninit();
                let service =
                    oer_memory::Placed::init(&mut service_storage, |place| place.write(service));
                let mut completed_storage = core::mem::MaybeUninit::uninit();
                let (completed, ()) = join(
                    StationService::run_placed(
                        service,
                        &mut completed_storage,
                        owner,
                        &mut io,
                        &*world.clock,
                    ),
                    script,
                )
                .await;
                completed.read()
            } else {
                join(service.run(owner, &mut io, &*world.clock), script)
                    .await
                    .0
            };
            let completed = if let Some(protected) = world.restart_stopped_service.get() {
                let stopped = completed.into_stopped().ok().expect("ordinary Stop");
                let RestartedService {
                    owner,
                    service,
                    deferred_eapol,
                    notice,
                } = stopped
                    .restart(channel)
                    .ok()
                    .expect("complete resource bank");
                assert!(matches!(
                    deferred_eapol,
                    oer_ieee80211_sta::machine::DeferredEapol::Empty
                ));
                assert!(notice.is_none());
                world.outputs.borrow_mut().clear();
                let (completed, ()) = join(service.run(owner, &mut io, &*world.clock), async {
                    let mut seen = world.air.sent().len();
                    world.associate(&mut seen, protected).await;
                    if protected {
                        world.handshake(&mut seen).await;
                    } else {
                        world
                            .notice(StaNotice::Connected {
                                bssid: oer_ieee80211_wire::address::Bssid::new(
                                    IndividualAddress::new(BSSID).unwrap(),
                                )
                                .unwrap(),
                                channel: 6,
                            })
                            .await;
                    }
                    world.send_msdu(ACCESS_POINT_ADDRESS);
                    let data = world.sent(&mut seen, DATA).await;
                    assert_eq!(data.plaintext[4..10], BSSID);
                    assert_eq!(data.key.is_some(), protected);
                    world.command(ServiceCommand::Stop).await;
                })
                .await;
                completed
            } else {
                completed
            };
            let ServiceCompleted {
                end,
                retired: mut service,
            } = completed;
            match world.expected_stream_end.get() {
                Some(
                    expected @ (ExpectedStreamEnd::Drained | ExpectedStreamEnd::SubmissionFailed),
                ) => {
                    assert!(matches!(
                        &end,
                        ServiceEnd::Poisoned {
                            fault: ServiceFault::Backend(_) | ServiceFault::Submission(_),
                            ..
                        }
                    ));
                    assert_eq!(
                        matches!(
                            &end,
                            ServiceEnd::Poisoned {
                                fault: ServiceFault::Submission(_),
                                ..
                            }
                        ),
                        matches!(expected, ExpectedStreamEnd::SubmissionFailed)
                    );
                    assert!(service._pending.admissions.first().is_none());
                    assert_eq!(
                        service.data.available(WmmAccessCategory::BestEffort),
                        u32::from(MODEL_TX_QUEUE_DEPTH.get())
                    );
                    assert_eq!(world.air.disable_calls(), 1);
                }
                Some(ExpectedStreamEnd::FalseDrained) => {
                    let ServiceEnd::Quarantined {
                        fault: ServiceFault::DrainedWithOutstanding { missing, .. },
                        ..
                    } = &end
                    else {
                        panic!("an unreturned TX ticket contradicts Drained")
                    };
                    assert!(matches!(missing, PendingTerminal::Transmit(_)));
                    assert_eq!(service._pending.admissions.first(), Some(*missing));
                    assert_eq!(world.air.disable_calls(), 0);
                }
                Some(ExpectedStreamEnd::Quarantined) => {
                    assert!(matches!(
                        &end,
                        ServiceEnd::Quarantined {
                            fault: ServiceFault::Backend(_),
                            ..
                        }
                    ));
                    assert!(matches!(
                        service._pending.admissions.first(),
                        Some(PendingTerminal::Transmit(_))
                    ));
                    assert_eq!(world.air.disable_calls(), 0);
                }
                None => {}
            }
            if world.expect_queued_command_on_poison.get() {
                assert!(matches!(&end, ServiceEnd::Quarantined { .. }));
                assert!(
                    matches!(
                        io.commands.try_receive(),
                        Some(ServiceCommand::WantAccessPoint)
                    ),
                    "a simultaneously ready poisoned event must not consume the command"
                );
                assert!(matches!(
                    io.commands.try_receive(),
                    Some(ServiceCommand::Stop)
                ));
            }
            world.counters.set(Some(service.counters));
            world
                .disables_at_body_end
                .set(Some(world.air.disable_calls()));
            assert_eq!(service.station.lost_tokens(), 0);
            assert_eq!(service.data.counters().lost_tokens, 0);
            if matches!(&end, ServiceEnd::Stopped(_) | ServiceEnd::Failed { .. }) {
                assert_eq!(
                    service.station.frame_credits(),
                    (4, 4),
                    "all station frame owners return before teardown"
                );
                assert_eq!(
                    service.station.key_owners(),
                    (1, GROUPS, true),
                    "all key owners return before teardown"
                );
                assert!(service._pending.group.is_none());
                for category in WmmAccessCategory::ALL {
                    assert_eq!(
                        service.data.available(category),
                        u32::from(MODEL_TX_QUEUE_DEPTH.get()),
                        "disable returns every attempt"
                    );
                }
            }
            if self.refuse_initial_enable {
                assert!(matches!(
                    &end,
                    ServiceEnd::Failed {
                        fault: ServiceFault::EnableRefused(()),
                        ..
                    }
                ));
                assert!(
                    matches!(&end, ServiceEnd::Failed { .. }),
                    "refusal preserves a Disabled backend"
                );
                assert!(
                    service.station.poll_output().is_none(),
                    "all request terminals were delivered once"
                );
                assert_eq!(world.outputs.borrow().iter().filter(|output| matches!(output,
                    ObservedOutput::CommandCompleted(terminal) if terminal.request.correlation() == 901
                    && terminal.outcome == oer_ieee80211_sta::machine::StaCommandOutcome::ConnectFailed(
                        oer_ieee80211_sta::machine::StaDisconnectReason::Arbiter(oer_ieee80211_arbiter::LeaveReason::PortFailed))
                )).count(), 1);
            }
            match end {
                ServiceEnd::Stopped(owner) if world.drop_stopped_service.get() => {
                    drop(service);
                    let Ok(owner) = owner.enable().await else {
                        panic!("dropping a stopped service must leave its disabled owner usable");
                    };
                    let Ok(owner) = owner
                        .disable(&mut |_| core::ops::ControlFlow::Continue(()))
                        .await
                    else {
                        panic!("the next epoch must also disable");
                    };
                    Ended::uninstall(owner, (), ())
                }
                ServiceEnd::Stopped(owner) => Ended::uninstall(owner, (), ()),
                ServiceEnd::Failed { owner, .. } => owner.uninstall((), ()),
                ServiceEnd::Quarantined { request, .. } => Ended::fail_stop(request, ()),
                ServiceEnd::Poisoned {
                    owner: poisoned, ..
                } => Ended::uninstall(poisoned, (), ()),
            }
        }
    }
}

/// Run the service on a 2.4 GHz model with `security`, the test playing
/// the access point by `script`.
fn run<S>(security: StaSecurity, script: S)
where
    S: for<'w, 'b, 'id, 'scope> FnOnce(&'w Application<'w, 'b, 'id, LocalId<'scope>>) -> Script<'w>
        + Send,
{
    run_with(config(security), script);
}

/// Run the service on a 2.4 GHz model with `config`.
fn run_with<S>(config: TestConfig, script: S) -> ServiceCounters
where
    S: for<'w, 'b, 'id, 'scope> FnOnce(&'w Application<'w, 'b, 'id, LocalId<'scope>>) -> Script<'w>
        + Send,
{
    run_with_groups::<S, 2>(config, script)
}

fn run_with_groups<S, const GROUPS: usize>(config: TestConfig, script: S) -> ServiceCounters
where
    S: for<'w, 'b, 'id, 'scope> FnOnce(&'w Application<'w, 'b, 'id, LocalId<'scope>>) -> Script<'w>
        + Send,
{
    run_case::<S, GROUPS>(config, script, false)
}

/// The service future lives on the test's stack in a debug build and fills
/// the default 2 MiB, so every case runs on a thread with a stack of its own.
fn run_case<S, const GROUPS: usize>(
    config: TestConfig,
    script: S,
    refuse_initial_enable: bool,
) -> ServiceCounters
where
    S: for<'w, 'b, 'id, 'scope> FnOnce(&'w Application<'w, 'b, 'id, LocalId<'scope>>) -> Script<'w>
        + Send,
{
    run_case_with_placement::<S, GROUPS>(config, script, refuse_initial_enable, false)
}

fn run_case_with_placement<S, const GROUPS: usize>(
    config: TestConfig,
    script: S,
    refuse_initial_enable: bool,
    placed: bool,
) -> ServiceCounters
where
    S: for<'w, 'b, 'id, 'scope> FnOnce(&'w Application<'w, 'b, 'id, LocalId<'scope>>) -> Script<'w>
        + Send,
{
    std::thread::scope(|scope| {
        std::thread::Builder::new()
            .stack_size(16 << 20)
            .spawn_scoped(scope, move || {
                run_case_on_this_thread::<S, GROUPS>(config, script, refuse_initial_enable, placed)
            })
            .expect("a thread with a stack for the case")
            .join()
            .unwrap_or_else(|panic| std::panic::resume_unwind(panic))
    })
}

fn run_case_on_this_thread<S, const GROUPS: usize>(
    config: TestConfig,
    script: S,
    refuse_initial_enable: bool,
    placed: bool,
) -> ServiceCounters
where
    S: for<'w, 'b, 'id, 'scope> FnOnce(&'w Application<'w, 'b, 'id, LocalId<'scope>>) -> Script<'w>
        + Send,
{
    block_on(with_local_slot(async |claim| {
        let mut slot = claim.place();
        let clock = std::rc::Rc::new(VirtualClock::<8>::new());
        let (backend, air) = ModelBackend::<_, ModelCaps24, ServiceContexts>::new(
            channel6(),
            [
                IndividualAddress::new(LOCAL).unwrap(),
                IndividualAddress::new(ACCESS_POINT_ADDRESS).unwrap(),
            ],
            clock.clone(),
        );
        let mut transmit = MsduQueue::<TxMsdu, 2>::new();
        let mut receive = MsduQueue::<EthernetFrame, 2>::new();
        let (stack, transmit) = transmit.split();
        let (receive, delivered) = receive.split();
        let world = World {
            air,
            clock,
            outputs: RefCell::new(Vec::new()),
            completions: RefCell::new(Vec::new()),
            refuse_rate: Cell::new(false),
            fail_submission: Cell::new(false),
            counters: Cell::new(None),
            disables_at_body_end: Cell::new(None),
            expect_queued_command_on_poison: Cell::new(false),
            expected_stream_end: Cell::new(None),
            drop_stopped_service: Cell::new(false),
            restart_stopped_service: Cell::new(None),
            stack: RefCell::new(stack),
            delivered: RefCell::new(delivered),
        };
        let uninstalled = oer_radio_port::LocalExecution
            .scope(
                (&mut slot)
                    .register(backend)
                    .ok()
                    .expect("fresh local model slot"),
                async |frontend| {
                    run_port(
                        frontend,
                        Body::<S, GROUPS> {
                            world: &world,
                            transmit,
                            receive,
                            config,
                            script,
                            refuse_initial_enable,
                            placed,
                        },
                    )
                    .await
                },
            )
            .await;
        match uninstalled {
            oer_radio_port::PortEnd::Uninstalled { .. } => {}
            oer_radio_port::PortEnd::FailStopped { outcome, .. } => {
                assert!(matches!(
                    outcome.outcome(),
                    oer_radio_port::Abandon::Stopped(_)
                ));
                assert_eq!(
                    world.air.disable_calls(),
                    world
                        .disables_at_body_end
                        .get()
                        .expect("body recorded disable progress"),
                    "P2 quarantine never starts another disable"
                );
            }
            oer_radio_port::PortEnd::Abandoned { .. } => panic!("service must return its output"),
        }
        let counters = world.counters.get().expect("service returned its counters");
        slot.end_model_world();
        counters
    }))
}

fn dsss1() -> PhyRate {
    PhyRate::Legacy(LegacyRate::Dsss1M)
}

#[test]
fn refused_initial_enable_finishes_the_accepted_connect_and_uninstalls_without_reset() {
    run_case::<_, 2>(
        config(StaSecurity::Open),
        |world| {
            Box::pin(async move {
                world
                    .notice(StaNotice::Disconnected(
                        oer_ieee80211_sta::machine::StaDisconnectReason::Arbiter(
                            oer_ieee80211_arbiter::LeaveReason::PortFailed,
                        ),
                    ))
                    .await;
                assert!(world.air.sent().is_empty());
            })
        },
        true,
    );
}

#[test]
fn port_failure_during_scan_returns_one_connect_terminal_and_one_lifecycle_notice() {
    use oer_ieee80211_sta::machine::{StaCommandOutcome, StaDisconnectReason};
    run(StaSecurity::Open, |world| {
        Box::pin(async move {
            let mut seen = 0;
            world.station_command(777, StaCommand::Connect).await;
            world.sent(&mut seen, PROBE_REQUEST).await;
            world.air.poison();
            let reason =
                StaDisconnectReason::Arbiter(oer_ieee80211_arbiter::LeaveReason::PortFailed);
            world.notice(StaNotice::Disconnected(reason)).await;
            let outputs = world.outputs.borrow();
            assert_eq!(
                outputs
                    .iter()
                    .filter(|output| matches!(output,
            ObservedOutput::CommandCompleted(terminal) if terminal.request.correlation() == 777
                && terminal.outcome == StaCommandOutcome::ConnectFailed(reason)))
                    .count(),
                1
            );
            assert_eq!(
                outputs
                    .iter()
                    .filter(|output| matches!(
                        output,
                        ObservedOutput::Notice(StaNotice::Disconnected(_))
                    ))
                    .count(),
                1
            );
        })
    });
}

#[test]
fn event_end_checks_outstanding_tx_and_quarantines_without_disable() {
    use oer_ieee80211_sta::machine::StaDisconnectReason;
    for end in [
        ExpectedStreamEnd::Drained,
        ExpectedStreamEnd::SubmissionFailed,
        ExpectedStreamEnd::FalseDrained,
        ExpectedStreamEnd::Quarantined,
    ] {
        run(StaSecurity::Open, |world| {
            Box::pin(async move {
                let mut seen = 0;
                world.associate(&mut seen, false).await;
                world
                    .notice(StaNotice::Connected {
                        bssid: oer_ieee80211_wire::address::Bssid::new(
                            IndividualAddress::new(BSSID).unwrap(),
                        )
                        .unwrap(),
                        channel: 6,
                    })
                    .await;
                world
                    .air
                    .hold_completion(WmmAccessCategory::BestEffort, true);
                world.send_msdu([2, 0, 0, 0, 0, 7]);
                world.sent(&mut seen, DATA).await;
                world.expected_stream_end.set(Some(end));
                match end {
                    ExpectedStreamEnd::Drained => {
                        world.air.fail_after_terminals();
                        world
                            .air
                            .hold_completion(WmmAccessCategory::BestEffort, false);
                    }
                    ExpectedStreamEnd::SubmissionFailed => {
                        world.fail_submission.set(true);
                        world.send_msdu([2, 0, 0, 0, 0, 8]);
                        for _ in 0..20 {
                            yield_now().await;
                        }
                        assert!(
                            !world.fail_submission.get(),
                            "the second attempt reached admission"
                        );
                        world
                            .air
                            .hold_completion(WmmAccessCategory::BestEffort, false);
                    }
                    ExpectedStreamEnd::FalseDrained => world.air.falsely_claim_drained(),
                    ExpectedStreamEnd::Quarantined => world.air.poison(),
                }
                world
                    .notice(StaNotice::Disconnected(StaDisconnectReason::Arbiter(
                        oer_ieee80211_arbiter::LeaveReason::PortFailed,
                    )))
                    .await;
            })
        });
    }
}

#[test]
fn poisoned_disable_after_stop_finishes_the_connected_lifecycle() {
    use oer_ieee80211_sta::machine::StaDisconnectReason;
    run(StaSecurity::Open, |world| {
        Box::pin(async move {
            let mut seen = 0;
            world.associate(&mut seen, false).await;
            world
                .notice(StaNotice::Connected {
                    bssid: oer_ieee80211_wire::address::Bssid::new(
                        IndividualAddress::new(BSSID).unwrap(),
                    )
                    .unwrap(),
                    channel: 6,
                })
                .await;
            world
                .air
                .hold_completion(WmmAccessCategory::BestEffort, true);
            world.send_msdu([2, 0, 0, 0, 0, 7]);
            world.sent(&mut seen, DATA).await;
            world.command(ServiceCommand::Stop).await;
            for _ in 0..4 {
                yield_now().await;
            }
            world.air.poison();
            let reason =
                StaDisconnectReason::Arbiter(oer_ieee80211_arbiter::LeaveReason::PortFailed);
            world.notice(StaNotice::Disconnected(reason)).await;
            let outputs = world.outputs.borrow();
            assert_eq!(
                outputs
                    .iter()
                    .filter(|output| matches!(
                        output,
                        ObservedOutput::Notice(StaNotice::Disconnected(_))
                    ))
                    .count(),
                1
            );
            assert_eq!(outputs.iter().filter(|output| matches!(output,
            ObservedOutput::CommandCompleted(terminal) if terminal.request.command() == StaCommand::Connect)).count(), 1);
        })
    });
}

#[test]
fn a_command_ready_with_a_poisoned_event_stays_owned_by_its_queue() {
    run(StaSecurity::Open, |world| {
        Box::pin(async move {
            // Let the initial enable finish, leaving serve waiting for its inputs.
            for _ in 0..4 {
                yield_now().await;
            }
            world.expect_queued_command_on_poison.set(true);
            world.command(ServiceCommand::WantAccessPoint).await;
            world.air.poison();
        })
    });
}

#[test]
fn on_an_open_network_the_join_goes_on_the_default_record_and_then_the_associations() {
    run(StaSecurity::Open, |world| {
        Box::pin(async move {
            let mut seen = 0;
            let [probe, authentication, association] = world.associate(&mut seen, false).await;
            // Before the connection: the probe once at 1 Mb/s, the join on the
            // default control record, 1 Mb/s DSSS.
            assert_eq!(probe.rate, dsss1());
            assert_eq!(authentication.rate, dsss1());
            assert_eq!(association.rate, dsss1());
            world
                .notice(StaNotice::Connected {
                    bssid: oer_ieee80211_wire::address::Bssid::new(
                        oer_ieee80211_wire::address::IndividualAddress::new(BSSID).unwrap(),
                    )
                    .unwrap(),
                    channel: 6,
                })
                .await;
            // A BSS without WMM: the station's interface contends by the
            // DCF, the access point's interface keeps the standard defaults.
            assert_eq!(
                world.air.edca(oer_ieee80211_port::VifRole::Station),
                oer_ieee80211_port::EdcaParameterSet::from_wmm(
                    oer_ieee80211_wire::extensions::wmm::WmmParameterSet::DCF
                )
            );
            assert_eq!(
                world.air.edca(oer_ieee80211_port::VifRole::AccessPoint),
                oer_ieee80211_port::EdcaParameterSet::STANDARD_DEFAULT
            );
            // Connected at once on an open network: data at the rate control's
            // rate, the next control frame on the association's record.
            world.send_msdu([0x02, 0, 0, 0, 0, 0x07]);
            let data = world.sent(&mut seen, DATA).await;
            let peer = oer_ieee80211_upper_mac::rate_control::RatePeer {
                phy: PhyMode::Legacy,
            legacy_rates: oer_ieee80211_upper_mac::rate_control::NegotiatedLegacyRates::from_rate_elements(
                &[0x82, 0x84, 0x8b, 0x96], &[],
            ).unwrap(),
                ht_capabilities: None,
                he_capabilities: None,
                he_peer_state: None,
            };
            assert_eq!(
                data.rate,
                EspressifRateControl::for_peer((), &peer, meta().link_metric()).mpdu_rate()
            );
            world.station_command(2, StaCommand::Disconnect).await;
            let deauthentication = world.sent(&mut seen, DEAUTHENTICATION).await;
            assert_eq!(deauthentication.rate, dsss1());
        })
    });
}

#[test]
fn on_wpa2_the_join_goes_on_the_default_record_and_the_handshake_on_the_associations() {
    run(
        StaSecurity::Personal {
            password: oer_ieee80211_rsn::sae::SaePassword::new(b"password").unwrap(),
            pmk: Pmk::from_bytes(PMK),
        },
        |world| {
            Box::pin(async move {
                let mut seen = 0;
                let [_, authentication, association] = world.associate(&mut seen, true).await;
                assert_eq!(authentication.rate, dsss1());
                assert_eq!(association.rate, dsss1());
                // Associated: the rates start from the association, so the
                // handshake's Message 2 goes on its B-only record, 1 Mb/s DSSS.
                let ptk = world.handshake(&mut seen).await;
                // Connected: Group Message 2 goes under the pairwise key the
                // link lends, with the Protected bit, on the association's
                // record.
                world
                    .air
                    .receive(&protected_group_message1(&ptk, 3, 2), decrypted());
                let group_message2 = world.sent(&mut seen, DATA).await;
                assert!(
                    matches!(group_message2.key, Some((_, true))),
                    "{group_message2:?}"
                );
                assert_ne!(group_message2.plaintext[1] & 0x40, 0);
                assert!(group_message2.packet_number.is_some());
                assert_eq!(group_message2.rate, dsss1());
                world.station_command(2, StaCommand::Disconnect).await;
                let deauthentication = world.sent(&mut seen, DEAUTHENTICATION).await;
                assert_eq!(deauthentication.rate, dsss1());
            })
        },
    );
}

#[test]
fn a_one_slot_station_completes_group_replacement_through_the_service_event_stream() {
    use oer_ieee80211_sta::machine::StaDisconnectReason;
    run_with_groups::<_, 1>(
        config(StaSecurity::Personal {
            password: oer_ieee80211_rsn::sae::SaePassword::new(b"password").unwrap(),
            pmk: Pmk::from_bytes(PMK),
        }),
        |world| {
            Box::pin(async move {
                let mut seen = 0;
                world.associate(&mut seen, true).await;
                let ptk = world.handshake(&mut seen).await;
                world
                    .air
                    .receive(&protected_group_message1(&ptk, 3, 2), decrypted());
                let response = world.sent(&mut seen, DATA).await;
                assert!(matches!(response.key, Some((_, true))));
                world.station_command(2, StaCommand::Disconnect).await;
                world.sent(&mut seen, DEAUTHENTICATION).await;
                world
                    .notice(StaNotice::Disconnected(StaDisconnectReason::Requested))
                    .await;
            })
        },
    );
}

fn message3(ptk: &oer_ieee80211_rsn::Ptk) -> Vec<u8> {
    let mut plain = Vec::new();
    plain.extend_from_slice(&RSN);
    plain.extend_from_slice(&[0xdd, 22, 0x00, 0x0f, 0xac, 0x01, 0x01, 0x00]);
    plain.extend_from_slice(&[0x99; 16]);
    plain.extend_from_slice(&[0xdd, 0]);
    let wrapped = oer_ieee80211_cipher::aes::software_aes128_key_wrap(ptk.kek(), &plain).unwrap();
    let message3 =
        RsnTxFrame::<512>::message3(Akm::Psk, LOCAL, 2, ANONCE, [0; 8], wrapped.as_bytes())
            .unwrap()
            .authenticate(ptk);
    eapol_frame(message3.as_bytes())
}

#[test]
fn refusing_m4_releases_handshake_resources_without_connecting_and_service_can_reconnect() {
    run(
        StaSecurity::Personal {
            password: oer_ieee80211_rsn::sae::SaePassword::new(b"password").unwrap(),
            pmk: Pmk::from_bytes(PMK),
        },
        |world| {
            Box::pin(async move {
                let mut seen = 0;
                world.associate(&mut seen, true).await;
                let ptk = world.start_handshake(&mut seen).await;
                let sent_before = world.air.sent().len();
                for _ in 0..8 {
                    yield_now().await;
                }
                let statistics_before = world.completions.borrow().len();
                world.refuse_rate.set(true);
                world.air.receive(&message3(&ptk), meta());
                world
                    .notice(StaNotice::Disconnected(
                        oer_ieee80211_sta::machine::StaDisconnectReason::HandshakeFailed,
                    ))
                    .await;
                assert!(world.outputs.borrow().iter().any(|output| matches!(
                    output,
                    ObservedOutput::TxRefused {
                        operation: TxOperation::Station,
                        reason: TxRefusal::NoRate
                    }
                )));
                assert!(!world.outputs.borrow().iter().any(|output| matches!(
                    output,
                    ObservedOutput::Notice(StaNotice::Connected { .. })
                )));
                let sent = world.air.sent();
                assert!(
                    sent[sent_before..]
                        .iter()
                        .all(|frame| kind(frame) == DEAUTHENTICATION)
                );
                // Only accepted frames (the leave's deauthentication) update rates.
                assert_eq!(
                    world.completions.borrow().len() - statistics_before,
                    sent.len() - sent_before
                );
                world.associate(&mut seen, true).await;
                world.handshake(&mut seen).await;
            })
        },
    );
}

#[test]
fn a_data_preparation_refusal_releases_admission_and_the_next_msdu_can_go() {
    run(
        StaSecurity::Personal {
            password: oer_ieee80211_rsn::sae::SaePassword::new(b"password").unwrap(),
            pmk: Pmk::from_bytes(PMK),
        },
        |world| {
            Box::pin(async move {
                let mut seen = 0;
                world.associate(&mut seen, true).await;
                world.handshake(&mut seen).await;
                let sent_before = world.air.sent().len();
                for _ in 0..8 {
                    yield_now().await;
                }
                let statistics_before = world.completions.borrow().len();
                world.refuse_rate.set(true);
                world.send_msdu(ACCESS_POINT_ADDRESS);
                for _ in 0..50 {
                    yield_now().await;
                }
                assert!(world.outputs.borrow().iter().any(|output| matches!(
                    output,
                    ObservedOutput::TxRefused {
                        operation: TxOperation::Data,
                        reason: TxRefusal::NoRate
                    }
                )));
                assert_eq!(world.air.sent().len(), sent_before);
                assert_eq!(world.completions.borrow().len(), statistics_before);
                world.send_msdu(ACCESS_POINT_ADDRESS);
                let frame = world.sent(&mut seen, DATA).await;
                assert!(frame.packet_number.is_some());
            })
        },
    );
}

/// A protected data MPDU from the DS to the station, as the hardware
/// hands it up once it decrypted and verified it: the CCMP header of
/// `packet_number` stays, the MIC is gone.
fn protected_data(packet_number: u8, source: [u8; 6], payload: &[u8]) -> Vec<u8> {
    let mut frame = std::vec![0_u8; 24];
    frame[0..2].copy_from_slice(&0x4208_u16.to_le_bytes());
    frame[4..10].copy_from_slice(&LOCAL);
    frame[10..16].copy_from_slice(&BSSID);
    frame[16..22].copy_from_slice(&source);
    frame[22..24].copy_from_slice(&(u16::from(packet_number) << 4).to_le_bytes());
    frame.extend_from_slice(&[packet_number, 0, 0, 0x20, 0, 0, 0, 0]);
    frame.extend_from_slice(&[0xaa, 0xaa, 0x03, 0, 0, 0, 0x08, 0x00]);
    frame.extend_from_slice(payload);
    frame
}

fn decrypted() -> ModelRxMeta {
    meta().with_crypto(oer_ieee80211_port::RxEvidence::HardwareObserved(
        oer_ieee80211_port::RxCryptoStatus::DecryptedAndIntegrityVerified,
    ))
}

/// The whole path on the model, the test playing the access point and the
/// network stack: scan, join, the 4-way handshake, data both ways under
/// the pairwise key, and the disconnection, after which no data goes.
#[test]
fn a_station_scans_joins_handshakes_carries_data_both_ways_and_disconnects() {
    run(
        StaSecurity::Personal {
            password: oer_ieee80211_rsn::sae::SaePassword::new(b"password").unwrap(),
            pmk: Pmk::from_bytes(PMK),
        },
        |world| {
            Box::pin(async move {
                let mut seen = 0;
                world.associate(&mut seen, true).await;
                world.handshake(&mut seen).await;

                // TX: the stack's MSDU goes as one protected MPDU under the
                // pairwise key, to the access point, its payload after the
                // CCMP header's room and the LLC/SNAP header.
                let destination = [0x02, 0, 0, 0, 0, 0x07];
                world.send_msdu(destination);
                let data = world.sent(&mut seen, DATA).await;
                assert!(matches!(data.key, Some((_, true))), "{data:?}");
                assert!(data.packet_number.is_some());
                assert_ne!(data.plaintext[1] & 0x40, 0);
                assert_eq!(data.plaintext[1] & 0x03, 0x01, "to the DS");
                assert_eq!(data.plaintext[4..10], BSSID);
                assert_eq!(data.plaintext[10..16], LOCAL);
                assert_eq!(data.plaintext[16..22], destination);
                assert_eq!(
                    data.plaintext[32..40],
                    [0xaa, 0xaa, 0x03, 0, 0, 0, 0x08, 0x00]
                );
                assert_eq!(data.plaintext[40..], [0xab; 100]);

                // RX: a decrypted MPDU reaches the stack as its Ethernet
                // frame; its replay, an unverified one and an unprotected
                // one do not.
                let source = [0x02, 0, 0, 0, 0, 0x08];
                world
                    .air
                    .receive(&protected_data(1, source, &[0xcd; 40]), decrypted());
                let mut expected = Vec::new();
                expected.extend_from_slice(&LOCAL);
                expected.extend_from_slice(&source);
                expected.extend_from_slice(&[0x08, 0x00]);
                expected.extend_from_slice(&[0xcd; 40]);
                assert_eq!(world.delivered().await, [expected]);
                world
                    .air
                    .receive(&protected_data(1, source, &[0xcd; 40]), decrypted());
                world
                    .air
                    .receive(&protected_data(2, source, &[0xcd; 40]), meta());
                let mut plain = protected_data(3, source, &[0xcd; 40]);
                plain[1] &= !0x40;
                plain.drain(24..32);
                world.air.receive(&plain, meta());
                assert!(world.delivered().await.is_empty());
                assert_eq!(world.air.pending_rx(), 0, "every buffer recycled");

                // The disconnection: Deauthentication, the notice, and no
                // data after it.
                world.station_command(2, StaCommand::Disconnect).await;
                let deauthentication = world.sent(&mut seen, DEAUTHENTICATION).await;
                assert_eq!(deauthentication.plaintext[4..10], BSSID);
                world
                    .notice(StaNotice::Disconnected(
                        oer_ieee80211_sta::machine::StaDisconnectReason::Requested,
                    ))
                    .await;
                world.send_msdu(destination);
                for _ in 0..5 {
                    for _ in 0..20 {
                        yield_now().await;
                    }
                    world.clock.advance_to_next_deadline();
                }
                assert!(
                    world.air.sent()[seen..]
                        .iter()
                        .all(|frame| kind(frame) != DATA),
                    "no data after the disconnection"
                );
            })
        },
    );
}

const NULL: u16 = 0x0048;

/// Data takes every token of its own and the station's frames still go:
/// with the best-effort queue held, the stack fills the data machine's
/// tickets and buffers and its own queue, and the group handshake's EAPOL,
/// the Null before a background scan's absence and the Deauthentication
/// still reach the air.
#[test]
fn data_takes_all_its_tokens_and_eapol_null_and_deauthentication_still_go() {
    // A second channel, so the background scan leaves the home channel
    // and announces its absence with a Null.
    let mut config = config(StaSecurity::Personal {
        password: oer_ieee80211_rsn::sae::SaePassword::new(b"password").unwrap(),
        pmk: Pmk::from_bytes(PMK),
    });
    config
        .0
        .channels
        .push(
            SupportedChannel::new(
                oer_radio_regulatory::TEST_DOMAIN
                    .permit(Channel::ghz2_4(1, ChannelWidth::Mhz20).unwrap())
                    .unwrap(),
            )
            .unwrap(),
        )
        .unwrap();
    run_with(config, |world| {
        Box::pin(async move {
            let mut seen = 0;
            world.associate(&mut seen, true).await;
            let ptk = world.handshake(&mut seen).await;
            world.air.hold(WmmAccessCategory::BestEffort, true);
            // The data machine takes four MSDUs, its four tickets and
            // buffers; two more wait in the stack's queue, which then
            // refuses.
            let mut sent = 0;
            while world.try_send_msdu([0x02, 0, 0, 0, 0, 0x07]) {
                sent += 1;
                for _ in 0..20 {
                    yield_now().await;
                }
            }
            assert_eq!(sent, 4 + 2);
            assert!(
                world.air.sent()[seen..]
                    .iter()
                    .all(|frame| kind(frame) != DATA)
            );

            world
                .air
                .receive(&protected_group_message1(&ptk, 3, 2), decrypted());
            let group_message2 = world.sent(&mut seen, DATA).await;
            // Protected after the handshake: the EtherType follows the
            // CCMP header's room.
            assert_ne!(group_message2.plaintext[1] & 0x40, 0);
            assert_eq!(group_message2.plaintext[38..40], [0x88, 0x8e], "EAPOL");

            world.station_command(3, StaCommand::BackgroundScan).await;
            let null = world.sent(&mut seen, NULL).await;
            assert_ne!(null.plaintext[1] & 0x10, 0, "PM=1 before the absence");

            world.station_command(2, StaCommand::Disconnect).await;
            world.sent(&mut seen, DEAUTHENTICATION).await;
        })
    });
}

/// A station deadline reaches the station: an Authentication without a
/// response goes again once its wait passed, and the service does not spin
/// on the passed deadline meanwhile.
#[test]
fn a_passed_station_deadline_ticks_the_station() {
    run(StaSecurity::Open, |world| {
        Box::pin(async move {
            let mut seen = 0;
            world.station_command(1, StaCommand::Connect).await;
            world.sent(&mut seen, PROBE_REQUEST).await;
            world.air.receive(&beacon(false), meta());
            world.sent(&mut seen, AUTHENTICATION).await;
            // No response: the wait passes and the station tries again.
            world.sent(&mut seen, AUTHENTICATION).await;
        })
    });
}

/// The data queue belongs to the link: a disconnection during a background
/// scan's absence, while the port holds the link's attempts, leaves none of
/// them to go after the reconnection, and every data token comes back.
#[test]
fn a_disconnection_during_an_absence_leaves_no_old_frame_to_go_after_the_reconnection() {
    let mut config = config(StaSecurity::Open);
    config
        .0
        .channels
        .push(
            SupportedChannel::new(
                oer_radio_regulatory::TEST_DOMAIN
                    .permit(Channel::ghz2_4(1, ChannelWidth::Mhz20).unwrap())
                    .unwrap(),
            )
            .unwrap(),
        )
        .unwrap();
    run_with(config, |world| {
        Box::pin(async move {
            const OLD: [u8; 6] = [0x02, 0, 0, 0, 0, 0x0a];
            const NEW: [u8; 6] = [0x02, 0, 0, 0, 0, 0x0b];
            let to = |destination: [u8; 6]| {
                move |frame: &AirFrame| {
                    kind(frame) == DATA && frame.plaintext[16..22] == destination
                }
            };
            let mut seen = 0;
            world.associate(&mut seen, false).await;
            world
                .notice(StaNotice::Connected {
                    bssid: oer_ieee80211_wire::address::Bssid::new(
                        oer_ieee80211_wire::address::IndividualAddress::new(BSSID).unwrap(),
                    )
                    .unwrap(),
                    channel: 6,
                })
                .await;
            // The link's four MPDUs wait at the port.
            world.air.hold(WmmAccessCategory::BestEffort, true);
            for _ in 0..4 {
                world.send_msdu(OLD);
                for _ in 0..20 {
                    yield_now().await;
                }
            }
            // The absence starts; the station leaves during it.
            world.station_command(3, StaCommand::BackgroundScan).await;
            world.sent(&mut seen, NULL).await;
            world.station_command(2, StaCommand::Disconnect).await;
            // Leaving waits for the absence to end before sending the
            // Deauthentication. `sent` advances the virtual clock to that
            // deadline; waiting only for a notice leaves time stopped.
            world.sent(&mut seen, DEAUTHENTICATION).await;
            world
                .notice(StaNotice::Disconnected(
                    oer_ieee80211_sta::machine::StaDisconnectReason::Requested,
                ))
                .await;
            // Reconnected: every data token is back, so the machine takes
            // four MSDUs again and two more wait in the stack's queue.
            world.associate(&mut seen, false).await;
            let mut sent = 0;
            while world.try_send_msdu(NEW) {
                sent += 1;
                for _ in 0..20 {
                    yield_now().await;
                }
            }
            assert_eq!(sent, 4 + 2);
            // The queue opens: only the new link's frames go.
            world.air.hold(WmmAccessCategory::BestEffort, false);
            for _ in 0..6 {
                world.sent(&mut seen, DATA).await;
            }
            for _ in 0..5 {
                for _ in 0..20 {
                    yield_now().await;
                }
                world.clock.advance_to_next_deadline();
            }
            let air = world.air.sent();
            assert!(!air.iter().any(to(OLD)), "no frame of the old link went");
            assert_eq!(air.iter().filter(|frame| to(NEW)(frame)).count(), 6);
        })
    });
}

/// Beside an access point on the upstream's channel, a background scan's
/// absence starts after one of its TBTTs; MSDUs the stack sends during the
/// absence do not go while the station is away and all go once it is back,
/// none lost.
#[test]
fn beside_an_access_point_msdus_survive_an_absence() {
    use oer_time::Clock as _;
    let mut config = config(StaSecurity::Open);
    config
        .0
        .channels
        .push(
            SupportedChannel::new(
                oer_radio_regulatory::TEST_DOMAIN
                    .permit(Channel::ghz2_4(1, ChannelWidth::Mhz20).unwrap())
                    .unwrap(),
            )
            .unwrap(),
        )
        .unwrap();
    run_with(config, |world| {
        Box::pin(async move {
            let mut seen = 0;
            world.associate(&mut seen, false).await;
            world
                .notice(StaNotice::Connected {
                    bssid: oer_ieee80211_wire::address::Bssid::new(
                        oer_ieee80211_wire::address::IndividualAddress::new(BSSID).unwrap(),
                    )
                    .unwrap(),
                    channel: 6,
                })
                .await;
            let tbtt = world.clock.now().saturating_add(Duration::from_millis(5));
            world
                .command(ServiceCommand::AccessPoint(AccessPointFact::Running {
                    channel: SupportedChannel::new(
                        oer_radio_regulatory::TEST_DOMAIN
                            .permit(channel6())
                            .unwrap(),
                    )
                    .unwrap(),
                    next_tbtt: tbtt,
                    beacon_interval: oer_ieee80211_arbiter::coordinator::BeaconInterval::new(
                        Duration::from_micros(102_400),
                    )
                    .unwrap(),
                }))
                .await;
            world.station_command(3, StaCommand::BackgroundScan).await;
            let away = world.sent(&mut seen, NULL).await;
            assert_ne!(away.plaintext[1] & 0x10, 0, "PM=1 before the absence");
            assert!(world.clock.now() >= tbtt, "the absence follows a TBTT");
            let left = seen;
            let destinations = [[0x02, 0, 0, 0, 0, 0x0c], [0x02, 0, 0, 0, 0, 0x0d]];
            for destination in destinations {
                world.send_msdu(destination);
            }
            let back = world.sent(&mut seen, NULL).await;
            assert_eq!(back.plaintext[1] & 0x10, 0, "PM=0 once back");
            let returned = seen;
            assert!(
                world.air.sent()[left..returned]
                    .iter()
                    .all(|frame| kind(frame) != DATA),
                "no data while away"
            );
            for _ in destinations {
                world.sent(&mut seen, DATA).await;
            }
            let air = world.air.sent();
            for destination in destinations {
                assert_eq!(
                    air[returned..]
                        .iter()
                        .filter(
                            |frame| kind(frame) == DATA && frame.plaintext[16..22] == destination
                        )
                        .count(),
                    1,
                    "{destination:02x?} went once"
                );
            }
        })
    });
}

const QOS_DATA: u16 = 0x0088;

/// A WMM Parameter element: the network is a QoS BSS.
const WMM: [u8; 26] = [
    0xdd, 24, 0x00, 0x50, 0xf2, 0x02, 0x01, 0x01, 0x00, 0x00, 0x03, 0xa4, 0x00, 0x00, 0x27, 0xa4,
    0x00, 0x00, 0x42, 0x43, 0x5e, 0x00, 0x62, 0x32, 0x2f, 0x00,
];

/// Whether `frame` is a protected QoS Data frame carrying EAPOL.
fn carries_eapol(frame: &AirFrame) -> bool {
    kind(frame) == QOS_DATA && frame.plaintext.get(40..42) == Some(&[0x88, 0x8e][..])
}

/// The data attempts the air saw from `from` before the first frame
/// `until` takes.
fn data_before(air: &[AirFrame], from: usize, until: impl Fn(&AirFrame) -> bool) -> usize {
    air[from..]
        .iter()
        .take_while(|frame| !until(frame))
        .filter(|frame| kind(frame) == QOS_DATA && !carries_eapol(frame))
        .count()
}

#[test]
fn interleaved_station_and_data_terminals_keep_their_owner_and_voice_order() {
    run(
        StaSecurity::Personal {
            password: oer_ieee80211_rsn::sae::SaePassword::new(b"password").unwrap(),
            pmk: Pmk::from_bytes(PMK),
        },
        |world| {
            Box::pin(async move {
                let mut beacon = beacon(true);
                beacon.extend_from_slice(&WMM);
                let mut seen = 0;
                world.associate_with(&mut seen, beacon, true).await;
                let ptk = world.handshake(&mut seen).await;
                let baseline = world.completions.borrow().len();
                for category in WmmAccessCategory::ALL {
                    world.air.hold(category, true);
                }
                // Two TIDs share Voice, while Video and BE can complete first.
                // Distinct destinations let the script observe the original
                // order; the service never reads them to route a terminal.
                for (index, priority) in [6, 0, 7, 4].into_iter().enumerate() {
                    assert!(world.try_send_msdu_at([2, 0, 0, 0, 0, index as u8], priority));
                    for _ in 0..20 {
                        yield_now().await;
                    }
                }
                world
                    .air
                    .receive(&protected_group_message1(&ptk, 3, 2), decrypted());
                for _ in 0..20 {
                    yield_now().await;
                }
                assert_eq!(world.completions.borrow().len(), baseline);

                for (category, destination) in [
                    (WmmAccessCategory::Video, 3),
                    (WmmAccessCategory::BestEffort, 1),
                ] {
                    world.air.hold(category, false);
                    let frame = world.sent(&mut seen, QOS_DATA).await;
                    assert_eq!(frame.plaintext[21], destination);
                    assert!(!carries_eapol(&frame));
                }
                world.air.hold(WmmAccessCategory::Voice, false);
                for (destination, tid) in [(0, 6), (2, 7)] {
                    let frame = world.sent(&mut seen, QOS_DATA).await;
                    assert_eq!(frame.plaintext[21], destination);
                    assert_eq!(frame.plaintext[24] & 0x0f, tid);
                    assert!(!carries_eapol(&frame));
                }
                assert!(carries_eapol(&world.sent(&mut seen, QOS_DATA).await));
                assert_eq!(
                    &world.completions.borrow()[baseline..],
                    &[
                        FrameRates::Data,
                        FrameRates::Data,
                        FrameRates::Data,
                        FrameRates::Data,
                        FrameRates::Control,
                    ]
                );

                // All four data owners and the station owner are reusable.
                for priority in [0, 4, 6, 7] {
                    assert!(world.try_send_msdu_at([2, 0, 0, 0, 1, priority], priority));
                    let frame = world.sent(&mut seen, QOS_DATA).await;
                    assert_eq!(frame.plaintext[21], priority);
                }
                world.station_command(2, StaCommand::Disconnect).await;
                world.sent(&mut seen, DEAUTHENTICATION).await;
            })
        },
    );
}

/// The station's frames share the Voice queue with data of priority 6–7
/// and get no queue of their own; the service keeps at most the backend's
/// queue depth of data attempts at the port, so once a held Voice queue is
/// released EAPOL and Deauthentication go after at most depth + 1 data
/// attempts.
#[test]
fn eapol_and_deauthentication_wait_for_at_most_the_queue_depth_and_one_data_attempt() {
    run(
        StaSecurity::Personal {
            password: oer_ieee80211_rsn::sae::SaePassword::new(b"password").unwrap(),
            pmk: Pmk::from_bytes(PMK),
        },
        |world| {
            Box::pin(async move {
                let bound = usize::from(MODEL_TX_QUEUE_DEPTH.get()) + 1;
                let destination = [0x02, 0, 0, 0, 0, 0x07];
                let mut beacon = beacon(true);
                beacon.extend_from_slice(&WMM);
                let mut seen = 0;
                world.associate_with(&mut seen, beacon, true).await;
                let ptk = world.handshake(&mut seen).await;
                // Voice held, data of priority 6 fills it to the limit:
                // the data machine's tickets, then the stack's queue.
                let fill = async || {
                    world.air.hold(WmmAccessCategory::Voice, true);
                    let mut sent = 0;
                    while world.try_send_msdu_at(destination, 6) {
                        sent += 1;
                        for _ in 0..20 {
                            yield_now().await;
                        }
                    }
                    assert_eq!(sent, 4 + 2);
                };
                fill().await;
                world
                    .air
                    .receive(&protected_group_message1(&ptk, 3, 2), decrypted());
                for _ in 0..20 {
                    yield_now().await;
                }
                let released = world.air.sent().len();
                world.air.hold(WmmAccessCategory::Voice, false);
                while !world.air.sent()[released..].iter().any(carries_eapol) {
                    world.sent(&mut seen, QOS_DATA).await;
                }
                let before = data_before(&world.air.sent(), released, carries_eapol);
                assert!(before <= bound, "EAPOL after {before} data attempts");

                // The same for the Deauthentication, the queue full again.
                for _ in 0..6 {
                    world.sent(&mut seen, QOS_DATA).await;
                }
                fill().await;
                world.station_command(2, StaCommand::Disconnect).await;
                for _ in 0..20 {
                    yield_now().await;
                }
                let released = world.air.sent().len();
                world.air.hold(WmmAccessCategory::Voice, false);
                world.sent(&mut seen, DEAUTHENTICATION).await;
                let before = data_before(&world.air.sent(), released, |frame| {
                    kind(frame) == DEAUTHENTICATION
                });
                assert!(
                    before <= bound,
                    "Deauthentication after {before} data attempts"
                );
            })
        },
    );
}

#[test]
fn continuous_rx_cannot_starve_data_tx_or_application_commands() {
    run(StaSecurity::Open, |world| {
        Box::pin(async move {
            let mut seen = 0;
            world.associate(&mut seen, false).await;
            world
                .notice(StaNotice::Connected {
                    bssid: oer_ieee80211_wire::address::Bssid::new(
                        oer_ieee80211_wire::address::IndividualAddress::new(BSSID).unwrap(),
                    )
                    .unwrap(),
                    channel: 6,
                })
                .await;
            let first = world.air.sent().len();
            world.air.receive_continuously(&beacon(false), meta());
            world.send_msdu(ACCESS_POINT_ADDRESS);
            world.command(ServiceCommand::WantAccessPoint).await;
            let mut progressed = false;
            for _ in 0..64 {
                yield_now().await;
                let tx = world
                    .air
                    .sent()
                    .iter()
                    .skip(first)
                    .any(|frame| kind(frame) == DATA);
                let command = world.outputs.borrow().iter().any(|output| {
                    matches!(
                        output,
                        ObservedOutput::AccessPoint(AccessPointCommand::Start(_))
                    )
                });
                if tx && command {
                    progressed = true;
                    break;
                }
            }
            assert!(
                progressed,
                "ready RX cannot consume the TX/command share of a batch"
            );
            world.air.stop_continuous_receive();
        })
    });
}

#[test]
fn lost_port_events_are_counted_and_the_service_keeps_running() {
    let counters = run_with(config(StaSecurity::Open), |world| {
        Box::pin(async move {
            let mut seen = 0;
            world.associate(&mut seen, false).await;
            world
                .notice(StaNotice::Connected {
                    bssid: oer_ieee80211_wire::address::Bssid::new(
                        oer_ieee80211_wire::address::IndividualAddress::new(BSSID).unwrap(),
                    )
                    .unwrap(),
                    channel: 6,
                })
                .await;
            for _ in 0..MODEL_RX_EVENTS + 5 {
                world.air.receive(&beacon(false), meta());
            }
            for _ in 0..64 {
                yield_now().await;
            }
            world.send_msdu(ACCESS_POINT_ADDRESS);
            let data = world.sent(&mut seen, DATA).await;
            assert_eq!(data.plaintext[4..10], BSSID);
        })
    });
    assert_eq!(counters.events_lost.get(), 1);
}

#[test]
fn application_requests_return_correlated_refusals_and_unique_completions() {
    use oer_ieee80211_sta::machine::{CommandRefusal, StaCommandOutcome};
    run_with(config(StaSecurity::Open), |world| {
        Box::pin(async move {
            world.station_command(91, StaCommand::BackgroundScan).await;
            for _ in 0..16 {
                yield_now().await;
            }
            assert!(world.outputs.borrow().iter().any(|output| matches!(output,
                ObservedOutput::CommandRefused(ObservedCommandRefused { request, reason: CommandRefusal::NotConnected })
                    if request.correlation() == 91 && request.command() == StaCommand::BackgroundScan)));
            let mut seen = 0;
            world.associate(&mut seen, false).await;
            for _ in 0..16 {
                yield_now().await;
            }
            world.station_command(92, StaCommand::Connect).await;
            for _ in 0..16 {
                yield_now().await;
            }
            let outputs = world.outputs.borrow();
            assert_eq!(outputs.iter().filter(|output| matches!(output,
                ObservedOutput::CommandCompleted(ObservedCommandTerminal { request, outcome: StaCommandOutcome::Connected { .. } })
                    if request.correlation() == 1)).count(), 1);
            assert!(outputs.iter().any(|output| matches!(output,
                ObservedOutput::CommandRefused(ObservedCommandRefused { request, reason: CommandRefusal::AlreadyConnected })
                    if request.correlation() == 92)));
        })
    });
}

#[test]
fn protected_qos_ht_group_message_rekeys_once_after_integrity_and_replay_admission() {
    use {
        oer_ieee80211_upper_mac::receive::RxDropReason,
        oer_ieee80211_wire::ccmp::{CcmpHeader, CcmpKeyId, CcmpPacketNumber},
    };
    let counters = run_with(
        config(StaSecurity::Personal {
            password: oer_ieee80211_rsn::sae::SaePassword::new(b"password").unwrap(),
            pmk: Pmk::from_bytes(PMK),
        }),
        |world| {
            Box::pin(async move {
                let mut seen = 0;
                world.associate(&mut seen, true).await;
                let ptk = world.handshake(&mut seen).await;
                let plain = group_message1(&ptk, 3, 2);
                let mut protected = plain[..24].to_vec();
                protected[..2].copy_from_slice(&0xc288_u16.to_le_bytes());
                protected.extend_from_slice(&[7, 0, 0, 0, 0, 0]);
                protected.extend_from_slice(
                    &CcmpHeader::new(
                        CcmpPacketNumber::new(1).unwrap(),
                        CcmpKeyId::new(0).unwrap(),
                    )
                    .encode(),
                );
                protected.extend_from_slice(&plain[24..]);
                world.air.receive(&protected, meta());
                for _ in 0..20 {
                    yield_now().await;
                }
                assert!(world.outputs.borrow().iter().any(|output| matches!(
                    output,
                    ObservedOutput::RxFrameDropped(oer_ieee80211_sta::machine::StaRxError::Frame(
                        RxDropReason::Integrity
                    ))
                )));
                world.air.receive(&protected, decrypted());
                let reply = world.sent(&mut seen, DATA).await;
                assert!(reply.key.is_some());
                let header =
                    oer_ieee80211_wire::frame::DecodedHeader::decode(&reply.plaintext).unwrap();
                assert_eq!(header.body()[14..16], [0x88, 0x8e]);
                let after = world.air.sent().len();
                world.air.receive(&protected, decrypted());
                for _ in 0..20 {
                    yield_now().await;
                }
                assert_eq!(world.air.sent().len(), after);
                assert!(world.outputs.borrow().iter().any(|output| matches!(
                    output,
                    ObservedOutput::RxFrameDropped(oer_ieee80211_sta::machine::StaRxError::Frame(
                        RxDropReason::Replay
                    ))
                )));
            })
        },
    );
    assert_eq!(counters.rx_frames_dropped.get(), 2);
}

#[test]
fn message4_waits_for_the_actual_group_installation_terminal() {
    run(
        StaSecurity::Personal {
            password: oer_ieee80211_rsn::sae::SaePassword::new(b"password").unwrap(),
            pmk: Pmk::from_bytes(PMK),
        },
        |world| {
            Box::pin(async move {
                let mut seen = 0;
                world.associate(&mut seen, true).await;
                let ptk = world.start_handshake(&mut seen).await;
                let before = world.air.sent().len();
                world.air.hold_group_commands(true);
                world.air.receive(&message3(&ptk), meta());
                for _ in 0..20 {
                    yield_now().await;
                }
                assert_eq!(world.air.group_writes(), 0);
                assert_eq!(
                    world.air.sent().len(),
                    before,
                    "acceptance alone cannot authorize M4"
                );
                assert!(
                    !world.outputs.borrow().iter().any(|out| matches!(
                        out,
                        ObservedOutput::Notice(StaNotice::Connected { .. })
                    ))
                );
                world.air.hold_group_commands(false);
                world.sent(&mut seen, DATA).await;
                assert_eq!(world.air.group_writes(), 1);
            })
        },
    );
}

#[test]
fn stop_or_fault_with_an_accepted_group_install_keeps_owners_and_sends_no_message4() {
    use oer_ieee80211_sta::machine::{StaCommandOutcome, StaDisconnectReason};
    for poisoned in [false, true] {
        run(
            StaSecurity::Personal {
                password: oer_ieee80211_rsn::sae::SaePassword::new(b"password").unwrap(),
                pmk: Pmk::from_bytes(PMK),
            },
            |world| {
                Box::pin(async move {
                    let mut seen = 0;
                    world.associate(&mut seen, true).await;
                    let ptk = world.start_handshake(&mut seen).await;
                    let before = world.air.sent().len();
                    world.air.hold_group_commands(true);
                    world.air.receive(&message3(&ptk), meta());
                    for _ in 0..20 {
                        yield_now().await;
                    }
                    assert_eq!(world.air.group_writes(), 0);
                    assert_eq!(world.air.sent().len(), before);
                    let reason = if poisoned {
                        world.air.poison();
                        StaDisconnectReason::Arbiter(oer_ieee80211_arbiter::LeaveReason::PortFailed)
                    } else {
                        world.command(ServiceCommand::Stop).await;
                        StaDisconnectReason::Requested
                    };
                    world.notice(StaNotice::Disconnected(reason)).await;
                    assert_eq!(
                        world.air.sent().len(),
                        before,
                        "finalization must not send a queued M4 or deauthentication"
                    );
                    let outputs = world.outputs.borrow();
                    assert_eq!(
                        outputs
                            .iter()
                            .filter(|output| matches!(output,
                ObservedOutput::CommandCompleted(terminal) if terminal.request.correlation() == 1
                    && terminal.outcome == StaCommandOutcome::ConnectFailed(reason)))
                            .count(),
                        1
                    );
                    assert_eq!(
                        outputs
                            .iter()
                            .filter(|output| matches!(
                                output,
                                ObservedOutput::Notice(StaNotice::Disconnected(_))
                            ))
                            .count(),
                        1
                    );
                })
            },
        );
    }
}

#[test]
fn an_extended_ofdm_rate_set_reaches_the_data_and_control_plans() {
    run(StaSecurity::Open, |world| {
        Box::pin(async move {
            let mut seen = 0;
            let mut advertisement = beacon(false);
            advertisement.extend_from_slice(&[50, 8, 12, 18, 24, 36, 48, 72, 96, 108]);
            world.associate_with(&mut seen, advertisement, false).await;
            world
                .notice(StaNotice::Connected {
                    bssid: oer_ieee80211_wire::address::Bssid::new(
                        IndividualAddress::new(BSSID).unwrap(),
                    )
                    .unwrap(),
                    channel: 6,
                })
                .await;
            world.send_msdu([0x02, 0, 0, 0, 0, 0x07]);
            let data = world.sent(&mut seen, DATA).await;
            assert_eq!(data.rate, PhyRate::Legacy(LegacyRate::Ofdm54M));
            world.station_command(2, StaCommand::Disconnect).await;
            let disconnect = world.sent(&mut seen, DEAUTHENTICATION).await;
            assert_eq!(disconnect.rate, PhyRate::Legacy(LegacyRate::Ofdm6M));
        })
    });
}

#[test]
fn negotiated_pmf_checks_received_management_and_keeps_the_tx_key_through_disconnect() {
    for peer_disconnect in [false, true] {
        run(
            StaSecurity::Personal {
                password: oer_ieee80211_rsn::sae::SaePassword::new(b"password").unwrap(),
                pmk: Pmk::from_bytes(PMK),
            },
            |world| {
                Box::pin(async move {
                    let mut seen = 0;
                    let mut rsn = RSN;
                    rsn[20] |= 0x80;
                    let mut advertised = beacon(true);
                    let start = advertised
                        .windows(RSN.len())
                        .position(|bytes| bytes == RSN)
                        .unwrap();
                    advertised[start..start + RSN.len()].copy_from_slice(&rsn);
                    world.associate_with(&mut seen, advertised, true).await;
                    let ptk = world.start_handshake(&mut seen).await;
                    let mut plain = Vec::from(rsn);
                    plain.extend_from_slice(&[0xdd, 22, 0, 0x0f, 0xac, 1, 1, 0]);
                    plain.extend_from_slice(&[0x99; 16]);
                    plain.extend_from_slice(&[0xdd, 28, 0, 0x0f, 0xac, 9, 4, 0, 1, 0, 0, 0, 0, 0]);
                    plain.extend_from_slice(&[0x77; 16]);
                    plain.extend_from_slice(&[0xdd, 0, 0, 0]);
                    let wrapped =
                        oer_ieee80211_cipher::aes::software_aes128_key_wrap(ptk.kek(), &plain)
                            .unwrap();
                    let m3 = RsnTxFrame::<512>::message3(
                        Akm::Psk,
                        LOCAL,
                        2,
                        ANONCE,
                        [0; 8],
                        wrapped.as_bytes(),
                    )
                    .unwrap()
                    .authenticate(&ptk);
                    world.air.receive(&eapol_frame(m3.as_bytes()), meta());
                    let _message4 = world.sent(&mut seen, DATA).await;
                    world
                        .notice(StaNotice::Connected {
                            bssid: oer_ieee80211_wire::address::Bssid::new(
                                IndividualAddress::new(BSSID).unwrap(),
                            )
                            .unwrap(),
                            channel: 6,
                        })
                        .await;
                    let mut forged = std::vec![0;26];
                    forged[0] = 0xc0;
                    forged[4..10].copy_from_slice(&LOCAL);
                    forged[10..16].copy_from_slice(&BSSID);
                    forged[16..22].copy_from_slice(&BSSID);
                    forged[24] = 7;
                    world.air.receive(&forged, decrypted());
                    let query = world.sent(&mut seen, 0x00d0).await;
                    assert_eq!(query.plaintext[..2], [0xd0, 0x40]);
                    assert_eq!(query.plaintext[32..34], [8, 0]);
                    assert!(matches!(query.key, Some((_, true))));
                    assert_eq!(query.packet_number, Some(1));
                    assert!(
                        !world.outputs.borrow().iter().any(|o| matches!(
                            o,
                            ObservedOutput::Notice(StaNotice::Disconnected(_))
                        ))
                    );
                    if peer_disconnect {
                        // Independent Python AESCCM fixture; same public test PTK
                        // as the full handshake, management nonce and PN 20.
                        let hex = "c040000002000012345630055c11223330055c112233000014000020000000003f30f05c26540ce829d1";
                        let valid: Vec<_> = hex
                            .as_bytes()
                            .chunks_exact(2)
                            .map(|p| {
                                u8::from_str_radix(core::str::from_utf8(p).unwrap(), 16).unwrap()
                            })
                            .collect();
                        world.air.receive(&valid, meta());
                        world
                            .notice(StaNotice::Disconnected(
                                oer_ieee80211_sta::machine::StaDisconnectReason::Peer {
                                    reason_code: 7,
                                },
                            ))
                            .await;
                    } else {
                        world.station_command(2, StaCommand::Disconnect).await;
                        let deauthentication = world.sent(&mut seen, DEAUTHENTICATION).await;
                        assert_eq!(deauthentication.plaintext[..2], [0xc0, 0x40]);
                        assert!(matches!(deauthentication.key, Some((_, true))));
                        assert_eq!(deauthentication.packet_number, Some(2));
                        world
                            .notice(StaNotice::Disconnected(
                                oer_ieee80211_sta::machine::StaDisconnectReason::Requested,
                            ))
                            .await;
                    }
                })
            },
        );
    }
}

#[test]
fn stopped_service_returns_drained_rx_before_the_disabled_owner() {
    run(StaSecurity::Open, |world| {
        Box::pin(async move {
            for _ in 0..4 {
                yield_now().await;
            }
            world.drop_stopped_service.set(true);
            for _ in 0..3 {
                world.air.receive(&beacon(false), meta());
            }
            world.command(ServiceCommand::Stop).await;
            for _ in 0..6 {
                yield_now().await;
            }
        })
    });
}

#[test]
fn stopped_service_reconnects_and_sends_data_on_the_same_installation() {
    for protected in [false, true] {
        let security = if protected {
            StaSecurity::Personal {
                password: oer_ieee80211_rsn::sae::SaePassword::new(b"password").unwrap(),
                pmk: Pmk::from_bytes(PMK),
            }
        } else {
            StaSecurity::Open
        };
        run(security, |world| {
            Box::pin(async move {
                let mut seen = 0;
                world.associate(&mut seen, protected).await;
                if protected {
                    world.handshake(&mut seen).await;
                } else {
                    world
                        .notice(StaNotice::Connected {
                            bssid: oer_ieee80211_wire::address::Bssid::new(
                                IndividualAddress::new(BSSID).unwrap(),
                            )
                            .unwrap(),
                            channel: 6,
                        })
                        .await;
                }
                world.restart_stopped_service.set(Some(protected));
            })
        });
    }
}

#[test]
fn placed_service_retires_its_banks_on_stop_drained_and_quarantined_ports() {
    for end in [
        None,
        Some(ExpectedStreamEnd::Drained),
        Some(ExpectedStreamEnd::FalseDrained),
        Some(ExpectedStreamEnd::Quarantined),
    ] {
        run_case_with_placement::<_, 2>(
            config(StaSecurity::Open),
            |world| {
                Box::pin(async move {
                    let mut seen = 0;
                    world.associate(&mut seen, false).await;
                    world
                        .notice(StaNotice::Connected {
                            bssid: oer_ieee80211_wire::address::Bssid::new(
                                IndividualAddress::new(BSSID).unwrap(),
                            )
                            .unwrap(),
                            channel: 6,
                        })
                        .await;
                    if let Some(end) = end {
                        world
                            .air
                            .hold_completion(WmmAccessCategory::BestEffort, true);
                        world.send_msdu([2, 0, 0, 0, 0, 7]);
                        world.sent(&mut seen, DATA).await;
                        world.expected_stream_end.set(Some(end));
                        match end {
                            ExpectedStreamEnd::Drained => {
                                world.air.fail_after_terminals();
                                world
                                    .air
                                    .hold_completion(WmmAccessCategory::BestEffort, false);
                            }
                            ExpectedStreamEnd::FalseDrained => world.air.falsely_claim_drained(),
                            ExpectedStreamEnd::Quarantined => world.air.poison(),
                            ExpectedStreamEnd::SubmissionFailed => unreachable!(),
                        }
                        world
                            .notice(StaNotice::Disconnected(
                                oer_ieee80211_sta::machine::StaDisconnectReason::Arbiter(
                                    oer_ieee80211_arbiter::LeaveReason::PortFailed,
                                ),
                            ))
                            .await;
                    }
                })
            },
            false,
            true,
        );
    }
}
