//! The ESP32-S31's side of [`crate::observe`]: the counters' statics, the
//! observers the library's executors, station, RF actor and PC sampler
//! report to, and the readers.
//!
//! Executors: the application executor (hart 0, thread mode), the radio
//! executor (hart 0, P2) and, with `group`, hart 1's executor (thread mode),
//! each at its index. Hart 1's executor reports nothing without `group`.
//!
//! The readers run in thread mode, on either hart: a reader that yields
//! never stays above a writer it preempted.

#[cfg(not(oer_trace))]
compile_error!(
    "the `observe` feature reports through `oer_trace`: the image declares `instrumentation = [\"trace\"]`"
);

use core::future::Future;
use core::marker::PhantomData;
use core::pin::Pin;
use core::sync::atomic::{AtomicU32, Ordering};
use core::task::{Context, Poll};

use embassy_futures::yield_now;
use oer_chips_esp32s31_hal::peripherals::cache_counters::{Buses, CacheCounters};
use oer_chips_esp32s31_hal::peripherals::timg1_alarm::{Timg1AlarmInterrupt, Timg1AlarmParts};
#[cfg(feature = "group")]
use oer_chips_esp32s31_hal::peripherals::timg1_alarm::{Timg1Timer1Interrupt, Timg1Timer1Parts};
use oer_chips_esp32s31_platform_runtime::sampler::{
    self, Ended, SamplerTimer, StartError, Storage, Timer,
};
use oer_chips_esp32s31_radio_ieee80211_station::observation as station;
use oer_chips_family_platform_interrupt_table::Entry;
use oer_portable_memory::ClaimCell;
use static_cell::ConstStaticCell;

use super::{Boot, Error, SamplerToken};
use crate::observe::addba::{Addba, AddbaCounter, Outcome, Request};
use crate::observe::connect::{
    Connect, ConnectCounter, Heard, Probe, ScanStep, Stage, Step, Visit,
};
use crate::observe::cpu::{Boundary, CpuCounter, ExecutorSample, Phase};
use crate::observe::link::{Containment, LinkCounter, Positions, Rate, Settled, TxEnd};
use crate::observe::parts::{ImageNames, LibraryPart, PARTS, PartCounter, PartReading};
use crate::observe::tracking::{Branches, Ran, Tick, TrackingCounter, TrackingSample};
use crate::observe::{Bridge, BridgeCounter, Concurrent};

pub use crate::observe::link::{Link, TxAmpdu};
pub use oer_chips_esp32s31_hal::peripherals::cache_counters::{
    DataBus, Geometry as CacheGeometry, InstructionBus, Level as CacheLevel,
    Snapshot as CacheSnapshot,
};
pub use oer_chips_esp32s31_radio_ieee80211_station::WifiRxReport;
pub use oer_chips_esp32s31_radio_ieee80211_station::observation::StationRxKind;
pub use oer_chips_esp32s31_radio_ieee80211_station::{
    MacBank, MacSample, RxDescriptorCounts, RxDeviceView, RxHardwareView, RxProgress, RxRingView,
    RxSessionView, RxStallReading, RxStallResume, RxStallSnapshot, RxStallTicket, RxWalkerView,
};
pub use oer_portable_trace::profile::{PcSample, Thinning};

/// The library's executors.
pub const EXECUTORS: usize = 3;
/// The application executor's index among [`executors`]' samples.
pub const APPLICATION: usize = 0;
/// The radio executor's.
pub const RADIO: usize = 1;
/// Hart 1's executor's.
pub const HART1: usize = 2;

static EXECUTOR_COUNTERS: [CpuCounter; EXECUTORS] = [const { CpuCounter::new() }; EXECUTORS];
static PART_COUNTERS: [PartCounter; PARTS] = [const { PartCounter::new() }; PARTS];
static NAMES: ImageNames = ImageNames::new();
/// The epoch of the parts' and the tracking's longest and extremes.
static EPOCH: AtomicU32 = AtomicU32::new(0);
static TRACKING: TrackingCounter = TrackingCounter::new();
static LINK: LinkCounter = LinkCounter::new();
static CONNECT: ConnectCounter = ConnectCounter::new();
static BRIDGE: BridgeCounter = BridgeCounter::new();
/// The ADDBA Requests the station took; the station service on hart 0 is
/// their one writer.
static ADDBA: AddbaCounter = AddbaCounter::new();

const STATION_KINDS: usize = StationRxKind::ALL.len();
/// The station's receive decisions by kind; the station service on hart 0
/// is their one writer.
static STATION: [AtomicU32; STATION_KINDS] = [const { AtomicU32::new(0) }; STATION_KINDS];
/// Receive agreements started, and the latest one's window.
static STARTED: AtomicU32 = AtomicU32::new(0);
static WINDOW: AtomicU32 = AtomicU32::new(0);

/// The executors' activity clock: the time driver's microseconds, wrapping
/// at 2^32.
pub fn now_us() -> u32 {
    let [b0, b1, b2, b3, ..] = embassy_time::Instant::now().as_micros().to_le_bytes();
    u32::from_le_bytes([b0, b1, b2, b3])
}

/// Take one reading of a counter, sampling again after a yield while its
/// writer writes.
async fn settled<T>(mut read: impl FnMut() -> Result<T, Concurrent>) -> T {
    loop {
        match read() {
            Ok(value) => return value,
            Err(Concurrent) => yield_now().await,
        }
    }
}

/// Every executor's CPU since boot, at [`APPLICATION`], [`RADIO`] and
/// [`HART1`].
pub async fn executors() -> [ExecutorSample; EXECUTORS] {
    let mut samples = [ExecutorSample::default(); EXECUTORS];
    for (sample, counter) in samples.iter_mut().zip(&EXECUTOR_COUNTERS) {
        *sample = settled(|| counter.sample(now_us())).await;
    }
    samples
}

/// Start a new epoch of the parts' longest polls and the tracking's
/// extremes; returns it.
pub fn start_epoch() -> u32 {
    EPOCH.fetch_add(1, Ordering::Relaxed).wrapping_add(1)
}

/// Every part's polls: the library's in [`LibraryPart`] order, then the
/// image's in the order it named them, with the longest polls of the
/// current epoch.
pub async fn parts() -> [PartReading; PARTS] {
    let epoch = EPOCH.load(Ordering::Relaxed);
    let mut readings = [PartReading::default(); PARTS];
    for (number, (reading, counter)) in readings.iter_mut().zip(&PART_COUNTERS).enumerate() {
        *reading = PartReading {
            name: NAMES.name(number),
            sample: settled(|| counter.sample(now_us(), epoch)).await,
        };
    }
    readings
}

/// The shared RF actor's tracking ticks, with the current epoch's
/// extremes.
pub async fn tracking() -> TrackingSample {
    let epoch = EPOCH.load(Ordering::Relaxed);
    settled(|| TRACKING.sample(epoch)).await
}

/// The ADDBA Requests the station took since boot: each TID's last and
/// the requests by outcome.
pub async fn addba_requests() -> Addba {
    settled(|| ADDBA.read()).await
}

/// The station's receive decisions.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StationRxCounts {
    counts: [u32; STATION_KINDS],
    /// The receive agreements started: their kind's events carry the
    /// agreement's window, not a count.
    pub agreements_started: u32,
    /// The latest agreement's window.
    pub agreement_window: u32,
}

impl StationRxCounts {
    /// The decisions of `kind`; zero for [`StationRxKind::AgreementStarted`],
    /// counted in [`Self::agreements_started`].
    pub fn count(&self, kind: StationRxKind) -> u32 {
        self.counts
            .get(usize::from(kind as u8))
            .copied()
            .unwrap_or(0)
    }
}

/// What the station, its driver and the network bridge counted, each word
/// read on its own.
#[derive(Clone, Copy, Debug)]
pub struct Counters {
    pub bridge: Bridge,
    pub station_rx: StationRxCounts,
    pub ampdu: TxAmpdu,
    pub link: Link,
    /// The answered aggregates by position.
    pub positions: Positions,
    /// The receive driver's counters, once the station's receive path was
    /// bound.
    pub driver: Option<WifiRxReport>,
    /// The MAC's statistics at their last sample, and its frequency offset.
    pub mac: oer_chips_esp32s31_radio_ieee80211_station::MacCounters,
}

/// The counts now.
pub fn counters() -> Counters {
    let load = |count: &AtomicU32| count.load(Ordering::Relaxed);
    Counters {
        bridge: BRIDGE.read(),
        station_rx: StationRxCounts {
            counts: STATION.each_ref().map(load),
            agreements_started: load(&STARTED),
            agreement_window: load(&WINDOW),
        },
        ampdu: LINK.ampdu(),
        link: LINK.link(),
        positions: LINK.positions(),
        driver: oer_chips_esp32s31_radio_ieee80211_station::rx_counters(),
        mac: oer_chips_esp32s31_radio_ieee80211_station::mac_counters(),
    }
}

/// One of the image's named parts, to observe one future's polls.
pub struct Part {
    number: usize,
}

impl Part {
    /// Name one of the image's parts.
    ///
    /// # Errors
    /// [`Error::ImageParts`] once the image named every one of its parts.
    pub fn claim(name: &'static str) -> Result<Self, Error> {
        NAMES
            .claim(name)
            .map(|number| Self { number })
            .ok_or(Error::ImageParts)
    }

    /// `future`, pinned where it was made, with each of its polls observed
    /// as this part's: `part.polled(core::pin::pin!(future)).await`.
    pub fn polled<F: Future>(self, future: Pin<&mut F>) -> Polled<'_, F> {
        Polled {
            number: self.number,
            future,
        }
    }
}

/// A future whose polls one part observes; the executor that polls it is
/// the part's one writer. It holds the future it observes by its pin, so
/// that future stays where it was made.
pub struct Polled<'a, F> {
    number: usize,
    future: Pin<&'a mut F>,
}

impl<'a, F> Polled<'a, F> {
    /// `future`, observed as the library's `part`.
    pub(crate) fn library(part: LibraryPart, future: Pin<&'a mut F>) -> Self {
        Self {
            number: part as usize,
            future,
        }
    }
}

impl<F: Future> Future for Polled<'_, F> {
    type Output = F::Output;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<F::Output> {
        let counter = PART_COUNTERS.get(self.number);
        if let Some(counter) = counter {
            counter.observe(now_us(), false, EPOCH.load(Ordering::Relaxed));
        }
        let poll = self.future.as_mut().poll(cx);
        if let Some(counter) = counter {
            counter.observe(now_us(), true, EPOCH.load(Ordering::Relaxed));
        }
        poll
    }
}

/// The observer of executor `SLOT`'s activity.
#[derive(Clone, Copy, Default)]
pub(crate) struct ExecutorObserver<const SLOT: usize>;

impl<const SLOT: usize> oer_portable_trace::Observer for ExecutorObserver<SLOT> {
    fn event<E: oer_portable_trace::Event + Copy + 'static>(&self, event: E) {
        use oer_chips_family_platform_executor::observation::{Activity, Phase as Executor};
        const {
            if SLOT >= EXECUTORS {
                panic!("the observer's executor is one of the library's");
            }
        }
        if let (Some(activity), Some(counter)) = (Activity::of(&event), EXECUTOR_COUNTERS.get(SLOT))
        {
            counter.observe(Boundary {
                at_us: activity.at_micros,
                executor: activity.executor,
                hart: activity.hart,
                level: activity.level,
                phase: match activity.phase {
                    Executor::WorkStart => Phase::WorkStart,
                    Executor::WorkEnd => Phase::WorkEnd,
                    Executor::IdleStart => Phase::IdleStart,
                    Executor::IdleEnd => Phase::IdleEnd,
                },
            });
        }
    }
}

/// The observer of executor `SLOT`, for its task set.
pub(crate) const fn executor_observer<const SLOT: usize>() -> ExecutorObserver<SLOT> {
    ExecutorObserver
}

/// The library's observer of the station service, its datapath actor and
/// driver, the shared RF actor and the PC sampler: the datapath actor's
/// spans, the service's receive decisions and link counts, the RF actor's
/// tracking ticks and the sampler's samples.
#[derive(Clone, Copy, Default)]
pub struct Observer;

impl Observer {
    fn decoded<E: oer_portable_trace::Event + Copy, D: oer_portable_trace::Event>(
        event: E,
    ) -> Option<D> {
        (E::KIND == D::KIND)
            .then(|| D::decode(event.encode()))
            .flatten()
    }
}

impl oer_portable_trace::Observer for Observer {
    fn event<E: oer_portable_trace::Event + Copy + 'static>(&self, event: E) {
        if let Some(span) = Self::decoded::<E, station::ActorSpan>(event)
            && let Some(counter) = PART_COUNTERS.get(span_part(span.span) as usize)
        {
            counter.observe(now_us(), span.end, EPOCH.load(Ordering::Relaxed));
        }
        if let Some(decision) = Self::decoded::<E, station::StationRx>(event) {
            receive_decision(decision);
        }
        if let Some(request) = Self::decoded::<E, station::StationAddba>(event) {
            ADDBA.record(request.tid, addba_request(request));
        }
        if let Some(answered) = Self::decoded::<E, station::StationTxHoles>(event) {
            LINK.answered(answered.members, answered.holes);
        }
        if let Some(settled) = Self::decoded::<E, station::StationTxAmpdu>(event) {
            if settled.block_ack_request {
                LINK.block_ack_request();
            } else {
                LINK.settled(Settled {
                    rate: rate(settled.rate),
                    members: settled.members,
                    refused: settled.refused,
                    acknowledged: settled.acknowledged,
                    retried: settled.retried,
                    dropped: settled.dropped,
                });
            }
        }
        if let Some(data) = Self::decoded::<E, station::RxData>(event) {
            LINK.received(
                rate(data.rate),
                match data.containment {
                    station::Containment::Unknown => Containment::Unknown,
                    station::Containment::Single => Containment::Single,
                    station::Containment::InAmpdu => Containment::InAmpdu,
                },
            );
        }
        if let Some(data) = Self::decoded::<E, station::TxData>(event) {
            LINK.sent(
                rate(data.rate),
                match data.end {
                    station::TxEnd::Delivered => TxEnd::Delivered,
                    station::TxEnd::AckTimeout => TxEnd::AckTimeout,
                    station::TxEnd::Other => TxEnd::Other,
                    station::TxEnd::Unsent => TxEnd::Unsent,
                },
                data.transmissions,
            );
        }
        if let Some(step) = Self::decoded::<E, station::StationConnectStep>(event) {
            CONNECT.step(
                step.at_micros,
                connect_step(step.step),
                (step.channel != 0).then_some(step.channel),
            );
        }
        if let Some(heard) = Self::decoded::<E, station::StationScanHeard>(event) {
            CONNECT.heard(
                heard.at_micros,
                Heard {
                    own: heard.heard.own,
                    probe_response: heard.heard.probe_response,
                    to_us: heard.heard.to_us,
                    rssi: heard.heard.rssi,
                },
            );
        }
        if let Some(probe) = Self::decoded::<E, station::StationScanProbe>(event) {
            CONNECT.probe(
                probe.at_micros,
                match probe.step {
                    station::ScanProbeStep::Built => Probe::Built,
                    station::ScanProbeStep::Transmitted => Probe::Transmitted,
                    station::ScanProbeStep::NotTransmitted => Probe::NotTransmitted,
                    station::ScanProbeStep::Unknown => Probe::Unknown,
                },
            );
        }
        if let Some(tick) = Self::decoded::<E, station::TrackingTick>(event) {
            TRACKING.observe(tracking_tick(tick), EPOCH.load(Ordering::Relaxed));
        }
        if let Some(sample) = Self::decoded::<E, PcSample>(event) {
            let _ = KEPT.lend(|buffer| buffer.record(sample));
        }
        if let Some(ret) = Self::decoded::<E, oer_portable_trace::profile::ReturnSample>(event) {
            let _ = KEPT.lend(|buffer| buffer.record_return(ret));
        }
    }
}

/// The library's name of the station's connect step.
const fn connect_step(step: station::ConnectStep) -> Step {
    use station::ConnectStep as Source;
    match step {
        Source::Rest => Step::Rest,
        Source::ScanAsk => Step::Scan(ScanStep::Ask),
        Source::ScanTune => Step::Scan(ScanStep::Tune),
        Source::ScanConfigure => Step::Scan(ScanStep::Configure),
        Source::ScanProbe => Step::Scan(ScanStep::Probe),
        Source::ScanListen => Step::Scan(ScanStep::Listen),
        Source::JoinAsk => Step::Join(Stage::Ask),
        Source::JoinTune => Step::Join(Stage::Tune),
        Source::JoinConfigure => Step::Join(Stage::Configure),
        Source::Authenticate => Step::Join(Stage::Authenticate),
        Source::Associate => Step::Join(Stage::Associate),
        Source::StartLink => Step::Join(Stage::StartLink),
        Source::Handshake => Step::Join(Stage::Handshake),
        Source::Connected => Step::Join(Stage::Connected),
    }
}

/// The last connect as a whole.
pub fn connect() -> Connect {
    CONNECT.read()
}

/// Visit `index` of the last connect's scan.
pub fn connect_visit(index: usize) -> Option<Visit> {
    CONNECT.visit(index)
}

/// The station service's sink: its receive decisions go to the library's
/// observer.
#[cfg(feature = "wifi")]
pub(crate) type StationSink = station::TracedStationEvents<Observer>;
#[cfg(feature = "wifi")]
pub(crate) const STATION_SINK: StationSink = station::TracedStationEvents(Observer);

fn receive_decision(decision: station::StationRx) {
    let bump = |count: &AtomicU32, by: u32| {
        count.store(
            count.load(Ordering::Relaxed).wrapping_add(by),
            Ordering::Relaxed,
        );
    };
    if decision.kind == StationRxKind::AgreementStarted {
        bump(&STARTED, 1);
        WINDOW.store(decision.count, Ordering::Relaxed);
    } else if let Some(count) = STATION.get(usize::from(decision.kind as u8)) {
        bump(count, decision.count);
    }
}

/// The library's record of one ADDBA Request the station took.
const fn addba_request(request: station::StationAddba) -> Request {
    use station::AddbaOutcome as Source;
    Request {
        requested: request.requested,
        outcome: match request.outcome {
            Source::Accepted { window } => Outcome::Accepted { window },
            Source::Repeated { window } => Outcome::Repeated { window },
            Source::Refused { status } => Outcome::Refused { status },
            Source::Unanswered => Outcome::Unanswered,
        },
    }
}

/// The part a datapath actor's span counts in.
const fn span_part(span: station::Span) -> LibraryPart {
    match span {
        station::Span::Turn => LibraryPart::Turn,
        station::Span::Transmit => LibraryPart::Transmit,
        station::Span::Receive => LibraryPart::Receive,
        station::Span::Intake => LibraryPart::Intake,
        station::Span::Messages => LibraryPart::Messages,
    }
}

const fn rate(rate: station::RateClass) -> Rate {
    match rate {
        station::RateClass::Ht {
            mcs,
            width40,
            short_gi,
        } => Rate::Ht {
            mcs,
            width40,
            short_gi,
        },
        station::RateClass::NonHt => Rate::NonHt,
        station::RateClass::He => Rate::He,
        station::RateClass::Unknown => Rate::Unknown,
    }
}

fn tracking_tick(observed: station::TrackingTick) -> Tick {
    Tick {
        ran: match observed.ran {
            station::TickRan::Tracked(branches) => Ran::Tracked(Branches {
                inhibited: branches.inhibited,
                rfpll_corrected: branches.rfpll_corrected,
                wifi_i2c_changed: branches.wifi_i2c_changed,
                wifi_tx_power: branches.wifi_tx_power,
                bluetooth_ieee802154_tx_power: branches.bluetooth_ieee802154_tx_power,
                calibration_common: branches.calibration_common,
                calibration_transmit: branches.calibration_transmit,
            }),
            station::TickRan::NotDue => Ran::NotDue,
            station::TickRan::Unavailable => Ran::Unavailable,
        },
        temperature: observed.temperature,
        micros: observed.micros,
    }
}

/// The network core's turns; the network's task is their one writer.
#[cfg(feature = "network")]
static NETWORK: crate::observe::network::NetworkCounter =
    crate::observe::network::NetworkCounter::new();

/// What the network core's turns did since boot, each word read on its own.
#[cfg(feature = "network")]
pub fn network_turns() -> crate::observe::network::NetworkTurns {
    NETWORK.read()
}

// The last bin of a reservation's size ends at the look-ahead.
#[cfg(feature = "network")]
const _: () = {
    if oer_protocols_network_runtime::RESERVE_AHEAD > 16 {
        panic!("a reservation's bins end at 16 blocks");
    }
};

/// The network service's sink: its turns go to [`network_turns`], and the
/// core's drops go nowhere, as without `observe`.
#[cfg(feature = "network")]
#[derive(Default)]
pub(crate) struct NetworkSink(crate::observe::network::TurnTally);

#[cfg(feature = "network")]
impl oer_portable_events::EventSink<oer_protocols_network_runtime::ServiceEvent> for NetworkSink {
    // Out of the service's turn: inlined, the counting grows the turn past
    // its inlining into `run`, and its tail merging there joined two waker
    // calls whose scope the image's stack proof then cannot name.
    #[inline(never)]
    fn event(&mut self, event: oer_protocols_network_runtime::ServiceEvent) {
        use crate::observe::network::{Local, Publish, Request};
        use oer_protocols_network_runtime::{
            Interest, LocalEvent, PublishEnd, RequestKind, ServiceEvent,
        };
        let tally = &mut self.0;
        match event {
            ServiceEvent::Took(kind) => tally.took(
                &NETWORK,
                match kind {
                    RequestKind::Reserve { blocks } => Request::Reserve { blocks },
                    RequestKind::Release => Request::Release,
                    RequestKind::Send => Request::Send,
                    RequestKind::Close => Request::Close,
                },
            ),
            ServiceEvent::BudgetSpent => tally.budget_spent(),
            ServiceEvent::IntakeFull => tally.intake_full(),
            ServiceEvent::Accepted => tally.accepted(),
            ServiceEvent::Published(end) => tally.published(match end {
                PublishEnd::Chose => Publish::Chose,
                PublishEnd::Refused => Publish::Refused,
                PublishEnd::Budget => Publish::Budget,
                PublishEnd::Deferred => Publish::Deferred,
                PublishEnd::Fault => Publish::Fault,
            }),
            ServiceEvent::TurnEnded(end) => {
                tally.ended(&NETWORK, end.blocks_free, end.credits_free)
            }
            ServiceEvent::Yielded => tally.yielded(&NETWORK),
            ServiceEvent::Waited { ready } => tally.waited(&NETWORK, ready),
            ServiceEvent::Local(local) => tally.local(
                &NETWORK,
                match local {
                    LocalEvent::Offered => Local::Offered,
                    LocalEvent::Refused => Local::Refused,
                    LocalEvent::Held => Local::Held,
                    LocalEvent::Declined => Local::Declined,
                    LocalEvent::Withdrawn => Local::Withdrawn,
                    LocalEvent::Waits(Interest::Offers) => Local::WaitsOffers,
                    LocalEvent::Waits(Interest::Changes) => Local::WaitsChanges,
                },
            ),
        }
    }
}

#[cfg(feature = "network")]
impl oer_portable_events::EventSink<oer_protocols_network_core::NetEvent> for NetworkSink {
    const ENABLED: bool = false;

    fn event(&mut self, _: oer_protocols_network_core::NetEvent) {}
}

/// The network bridge took a frame from the station.
#[cfg(feature = "wifi")]
pub(crate) fn bridge_received(_: oer_portable_embassy_net::EthernetFrameView<'_>) {
    BRIDGE.received();
}

/// The stack refused a frame from the station for `reason`.
#[cfg(feature = "wifi")]
pub(crate) fn bridge_refused(reason: oer_portable_embassy_net::RxEnqueueError) {
    BRIDGE.refused(reason);
}

/// The bridge copied a frame the station lent in place.
#[cfg(feature = "wifi")]
pub(crate) fn bridge_copied(_: oer_portable_embassy_net::ethernet::LentCopy) {
    BRIDGE.copied();
}

/// The samples one session of one hart's sampler keeps.
#[cfg(not(feature = "group"))]
pub const SAMPLES: usize = 2048;
/// The samples one session of one hart's sampler keeps: hart 1's sampler
/// shares the one hart's SRAM between both.
#[cfg(feature = "group")]
pub const SAMPLES: usize = 1024;

/// A sampler's buffer, lent to the observer in the sampler's handler while
/// a session runs and to the sampler's owner once it ended.
type Kept = ClaimCell<&'static mut Thinning<SAMPLES>>;

oer_chips_family_platform_runtime::placed_static! {
    static SAMPLER: Storage<SamplerToken, Observer> = Storage::new(), in critical_data "pc_sampler";
    static BUFFER: ConstStaticCell<Thinning<SAMPLES>> = ConstStaticCell::new(Thinning::new()), in critical_data "pc_samples";
    static KEPT: Kept = ClaimCell::new(), in critical_data "pc_samples_claim";
}

#[cfg(feature = "group")]
oer_chips_family_platform_runtime::placed_static! {
    static SAMPLER1: Storage<super::Sampler1Token, Hart1Samples, sampler::Timer1> = Storage::new(), in critical_data "pc_sampler1";
    static BUFFER1: ConstStaticCell<Thinning<SAMPLES>> = ConstStaticCell::new(Thinning::new()), in critical_data "pc_samples1";
    static KEPT1: Kept = ClaimCell::new(), in critical_data "pc_samples1_claim";
}

/// The sampler's interrupt entry (TIMG1 T0).
pub(crate) fn on_sampler(part: &mut Timg1AlarmInterrupt) {
    SAMPLER.interrupt(part);
}

/// Hart 1's sampler's interrupt entry (TIMG1 T1).
#[cfg(feature = "group")]
pub(crate) fn on_sampler1(part: &mut Timg1Timer1Interrupt) {
    SAMPLER1.interrupt(part);
}

/// Hart 1's sampler's observer: its samples go to hart 1's buffer.
#[cfg(feature = "group")]
#[derive(Clone, Copy, Default)]
pub struct Hart1Samples;

#[cfg(feature = "group")]
impl oer_portable_trace::Observer for Hart1Samples {
    fn event<E: oer_portable_trace::Event + Copy + 'static>(&self, event: E) {
        if let Some(sample) = Observer::decoded::<E, PcSample>(event) {
            let _ = KEPT1.lend(|buffer| buffer.record(sample));
        }
        if let Some(ret) = Observer::decoded::<E, oer_portable_trace::profile::ReturnSample>(event)
        {
            let _ = KEPT1.lend(|buffer| buffer.record_return(ret));
        }
    }
}

/// What the boot hart observes only there: its cycle and retired
/// instruction counters, the L1 cache's counters and the PC sampler, which
/// samples it. It stays on the boot hart.
pub struct Hart0 {
    cache: CacheCounters,
    sampler: Sampler,
    _hart: PhantomData<*mut ()>,
}

/// What hart 1 observes only there: its cycle and retired instruction
/// counters and its PC sampler. It stays on hart 1.
#[cfg(feature = "group")]
pub struct Hart1 {
    sampler: Hart1Sampler,
    _hart: PhantomData<*mut ()>,
}

#[cfg(feature = "group")]
impl Hart1 {
    /// Hart 1's own counters now.
    pub fn counters(&mut self) -> HartClock {
        HartClock {
            cycles: oer_portable_riscv::cycle64(),
            instructions: oer_portable_riscv::instret64(),
        }
    }

    /// Hart 1's PC sampler.
    pub fn sampler(&mut self) -> &mut Hart1Sampler {
        &mut self.sampler
    }
}

/// Hart 1's PC sampler, on TIMG1 T1 at P4 above every other level of
/// hart 1.
#[cfg(feature = "group")]
pub type Hart1Sampler = Sampler<Timer<super::Sampler1Token, sampler::Timer1>>;

/// A hart's own cycle and retired instruction counters at one instant.
#[cfg(feature = "group")]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct HartClock {
    pub cycles: u64,
    pub instructions: u64,
}

/// The boot hart's counters at one instant.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct HartCounters {
    pub cycles: u64,
    pub instructions: u64,
    pub cache: CacheSnapshot,
    pub cache_geometry: CacheGeometry,
}

impl Hart0 {
    /// The boot hart's counters and the cache's; with `start`, clear and run
    /// every bus's cache counters first.
    pub fn counters(&mut self, start: bool) -> HartCounters {
        if start {
            self.cache.start(Buses::ALL);
        }
        let cycles = oer_portable_riscv::cycle64();
        let instructions = oer_portable_riscv::instret64();
        HartCounters {
            cycles,
            instructions,
            cache: self.cache.snapshot(),
            cache_geometry: self.cache.geometry(),
        }
    }

    /// The PC sampler.
    pub fn sampler(&mut self) -> &mut Sampler {
        &mut self.sampler
    }

    /// Ask the station's radio device for a stall snapshot of its receive
    /// path: it takes it at its next turn, before that turn's receive
    /// service, and records how the path went on after the service. The
    /// station's device runs on the boot hart, whose task the request wakes.
    pub fn request_rx_snapshot(&mut self) -> RxStallTicket {
        oer_chips_esp32s31_radio_ieee80211_station::request_rx_snapshot()
    }

    /// The snapshot taken for `ticket`; `None` before the device took it.
    pub fn rx_snapshot(&mut self, ticket: RxStallTicket) -> Option<RxStallReading> {
        oer_chips_esp32s31_radio_ieee80211_station::rx_snapshot(ticket)
    }
}

/// Why the sampler did not start or stop.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SamplerError {
    /// A session runs, or the handler kept the last one.
    Unavailable,
    /// The timer refused the period.
    Period,
    /// The handler did not hand the session back: the sampler is gone for
    /// the rest of the image's run.
    Lost,
}

/// How the last session ended.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Session {
    pub period_us: u32,
    /// The ticks it reported, and whether the alarm refused a target
    /// before the stop (the window's end then has no samples).
    pub ticks: u32,
    pub cut: bool,
    /// Ticks the handler saw with no session.
    pub stray: u32,
}

/// A PC sampler, by default hart 0's on TIMG1 T0, at P4 above every other
/// level of its hart: one sample of the interrupted PC and `ra` every
/// period while a session runs, into its one buffer of [`SAMPLES`].
pub struct Sampler<A: 'static = Timer<SamplerToken>> {
    sampler: sampler::Sampler<A>,
    kept: &'static Kept,
    period_us: u32,
    ended: Option<Ended>,
}

impl<I: Entry<Part = T::Interrupt> + 'static, T: SamplerTimer> Sampler<Timer<I, T>> {
    /// Whether a session can start.
    pub fn available(&self) -> bool {
        self.sampler.available()
    }

    /// Forget the last session's samples and sample every `period_us`.
    ///
    /// # Errors
    /// [`SamplerError::Unavailable`] or [`SamplerError::Period`]; no
    /// session runs.
    pub fn start(&mut self, period_us: u32) -> Result<(), SamplerError> {
        if !self.available() || self.kept.lend(|buffer| buffer.restart(period_us)).is_err() {
            return Err(SamplerError::Unavailable);
        }
        self.ended = None;
        self.period_us = period_us;
        self.sampler
            .start_masked(period_us)
            .map_err(|error| match error {
                StartError::Unavailable => SamplerError::Unavailable,
                StartError::Period => SamplerError::Period,
            })
    }

    /// Ask the running session to stop at its next tick.
    pub fn request_stop(&self) {
        self.sampler.request_stop();
    }

    /// Stop the running session, take it back and normalize its samples to
    /// one stride.
    ///
    /// # Errors
    /// [`SamplerError::Lost`] when the handler kept the session.
    pub async fn stop(&mut self) -> Result<(), SamplerError> {
        let ended = self
            .sampler
            .stop(self.period_us)
            .await
            .map_err(|sampler::Lost| SamplerError::Lost)?;
        if let Some(ended) = ended {
            let _ = self.kept.lend(|buffer| buffer.normalize());
            self.ended = Some(ended);
        }
        Ok(())
    }

    /// How the last session ended; none before one ended.
    pub fn session(&self) -> Option<Session> {
        self.ended.map(|ended| Session {
            period_us: self.period_us,
            ticks: ended.ticks,
            cut: ended.cut,
            stray: self.sampler.shared().stray(),
        })
    }

    /// Read the last session's samples; none before one ended.
    pub fn samples<R>(&self, read: impl FnOnce(&Thinning<SAMPLES>) -> R) -> Option<R> {
        self.ended?;
        self.kept.lend(|buffer| read(buffer)).ok()
    }
}

/// Stop and mask TIMG1 T0, bind the sampler's handler and install its
/// buffer: the boot hart's handle. Fails the boot when its storage was
/// taken before.
pub(crate) fn hart0(
    #[cfg_attr(not(feature = "group"), allow(unused_mut))] mut timg1: Timg1AlarmParts,
    cache: CacheCounters,
    boot: &mut Boot,
    token: SamplerToken,
) -> Hart0 {
    // T1's lane opens here, before hart 1 binds its sampler: T0's program
    // part is the enable word's one owner.
    #[cfg(feature = "group")]
    timg1.program.open_timer1();
    let (Some(sampler), Some(buffer)) =
        (SAMPLER.init(timg1, boot.router(), token), BUFFER.try_take())
    else {
        super::fail(Error::Storage.name());
    };
    if KEPT.install(buffer).is_err() {
        super::fail(Error::Storage.name());
    }
    Hart0 {
        cache,
        sampler: Sampler {
            sampler,
            kept: &KEPT,
            period_us: 0,
            ended: None,
        },
        _hart: PhantomData,
    }
}

/// Stop TIMG1 T1, bind hart 1's sampler on hart 1's `router` and install
/// its buffer: hart 1's handle. Fails the boot when its storage was taken
/// before.
#[cfg(feature = "group")]
pub(crate) fn hart1(
    timg1: Timg1Timer1Parts,
    router: &mut oer_chips_family_platform_interrupt_table::Router<
        impl oer_chips_family_platform_interrupt_table::Controller,
        1,
    >,
    token: super::Sampler1Token,
) -> Hart1 {
    let (Some(sampler), Some(buffer)) = (SAMPLER1.init(timg1, router, token), BUFFER1.try_take())
    else {
        super::fail(Error::Storage.name());
    };
    if KEPT1.install(buffer).is_err() {
        super::fail(Error::Storage.name());
    }
    Hart1 {
        sampler: Sampler {
            sampler,
            kept: &KEPT1,
            period_us: 0,
            ended: None,
        },
        _hart: PhantomData,
    }
}

impl Boot {
    /// What only the boot hart observes, once.
    ///
    /// # Errors
    /// [`Error::ObserveTaken`] when the boot took it before.
    pub fn observe(&mut self) -> Result<Hart0, Error> {
        self.observe.take().ok_or(Error::ObserveTaken)
    }
}
