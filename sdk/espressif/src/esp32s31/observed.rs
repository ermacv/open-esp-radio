//! The ESP32-S31's side of [`crate::observe`]: the counters' statics, the
//! observers the library's executors, station, RF actor and PC sampler
//! report to, and the readers.
//!
//! Executors: the application executor (hart 0, thread mode), the radio
//! executor (hart 0, P2) and, with `hart1`, hart 1's executor (thread mode),
//! each at its index. Hart 1's executor reports nothing without `hart1`.
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
use oer_esp32s31_compose::sampler::{self, Ended, StartError, Storage, Timer};
use oer_esp32s31_hal::peripherals::cache_counters::{Buses, CacheCounters};
use oer_esp32s31_hal::peripherals::timg1_alarm::{Timg1AlarmInterrupt, Timg1AlarmParts};
use oer_esp32s31_wifi::observation as station;
use oer_memory::ClaimCell;
use static_cell::ConstStaticCell;

use super::{Boot, Error, SamplerToken};
use crate::observe::cpu::{Boundary, CpuCounter, ExecutorSample, Phase};
use crate::observe::link::{Containment, LinkCounter, Rate, Settled, TxEnd};
use crate::observe::parts::{ImageNames, LibraryPart, PARTS, PartCounter, PartReading};
use crate::observe::tracking::{Branches, Ran, Tick, TrackingCounter, TrackingSample};
use crate::observe::{Bridge, BridgeCounter, Concurrent};

pub use crate::observe::link::{Link, TxAmpdu};
pub use oer_esp32s31_hal::peripherals::cache_counters::{
    DataBus, Geometry as CacheGeometry, InstructionBus, Level as CacheLevel,
    Snapshot as CacheSnapshot,
};
pub use oer_esp32s31_wifi::WifiRxReport;
pub use oer_esp32s31_wifi::observation::StationRxKind;
pub use oer_trace::profile::{PcSample, Thinning};

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
static BRIDGE: BridgeCounter = BridgeCounter::new();

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
    /// The receive driver's counters, once the station's receive path was
    /// bound.
    pub driver: Option<WifiRxReport>,
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
        driver: oer_esp32s31_wifi::rx_counters(),
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

impl<const SLOT: usize> oer_trace::Observer for ExecutorObserver<SLOT> {
    fn event<E: oer_trace::Event + Copy + 'static>(&self, event: E) {
        use oer_espressif_executor::observation::{Activity, Phase as Executor};
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

/// The library's observer of the station service, its driver, the shared
/// RF actor and the PC sampler: the service's spans, receive decisions and
/// link counts, the actor's tracking ticks and the sampler's samples.
#[derive(Clone, Copy, Default)]
pub struct Observer;

impl Observer {
    fn decoded<E: oer_trace::Event + Copy, D: oer_trace::Event>(event: E) -> Option<D> {
        (E::KIND == D::KIND)
            .then(|| D::decode(event.encode()))
            .flatten()
    }
}

impl oer_trace::Observer for Observer {
    fn event<E: oer_trace::Event + Copy + 'static>(&self, event: E) {
        if let Some(span) = Self::decoded::<E, station::ServiceSpan>(event)
            && let Some(counter) = PART_COUNTERS.get(span_part(span.span) as usize)
        {
            counter.observe(now_us(), span.end, EPOCH.load(Ordering::Relaxed));
        }
        if let Some(decision) = Self::decoded::<E, station::StationRx>(event) {
            receive_decision(decision);
        }
        if let Some(settled) = Self::decoded::<E, station::StationTxAmpdu>(event) {
            if settled.block_ack_request {
                LINK.block_ack_request();
            } else {
                LINK.settled(Settled {
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
        if let Some(tick) = Self::decoded::<E, station::TrackingTick>(event) {
            TRACKING.observe(tracking_tick(tick), EPOCH.load(Ordering::Relaxed));
        }
        if let Some(sample) = Self::decoded::<E, PcSample>(event) {
            let _ = KEPT.lend(|buffer| buffer.record(sample));
        }
        if let Some(ret) = Self::decoded::<E, oer_trace::profile::ReturnSample>(event) {
            let _ = KEPT.lend(|buffer| buffer.record_return(ret));
        }
    }
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

const fn span_part(span: station::Span) -> LibraryPart {
    match span {
        station::Span::Step => LibraryPart::Step,
        station::Span::Pending => LibraryPart::Pending,
        station::Span::Sources => LibraryPart::Sources,
        station::Span::Received => LibraryPart::Received,
        station::Span::Deliver => LibraryPart::Deliver,
        station::Span::Queue => LibraryPart::Queue,
        station::Span::Terminal => LibraryPart::Terminal,
        station::Span::Transmit => LibraryPart::Transmit,
        station::Span::Recycle => LibraryPart::Recycle,
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

/// The network bridge took a frame from the station.
#[cfg(feature = "wifi")]
pub(crate) fn bridge_received(_: oer_embassy_net::EthernetFrameView<'_>) {
    BRIDGE.received();
}

/// The stack refused a frame from the station for `reason`.
#[cfg(feature = "wifi")]
pub(crate) fn bridge_refused(reason: oer_embassy_net::RxEnqueueError) {
    BRIDGE.refused(reason);
}

/// The bridge copied a frame the station lent in place.
#[cfg(feature = "wifi")]
pub(crate) fn bridge_copied(_: oer_embassy_net::ethernet::LentCopy) {
    BRIDGE.copied();
}

/// The samples one session keeps.
pub const SAMPLES: usize = 2048;

oer_espressif_staged_runtime::placed_static! {
    static SAMPLER: Storage<SamplerToken, Observer> = Storage::new(), in critical_data "pc_sampler";
    static BUFFER: ConstStaticCell<Thinning<SAMPLES>> = ConstStaticCell::new(Thinning::new()), in critical_data "pc_samples";
    /// The buffer, lent to the observer in the sampler's handler while a
    /// session runs and to the sampler's owner once it ended.
    static KEPT: ClaimCell<&'static mut Thinning<SAMPLES>> = ClaimCell::new(), in critical_data "pc_samples_claim";
}

/// The sampler's interrupt entry (TIMG1 T0).
pub(crate) fn on_sampler(part: &mut Timg1AlarmInterrupt) {
    SAMPLER.interrupt(part);
}

/// What the boot hart observes only there: its cycle and retired
/// instruction counters, the L1 cache's counters and the PC sampler, which
/// samples it. It stays on the boot hart.
pub struct Hart0 {
    cache: CacheCounters,
    sampler: Sampler,
    _hart: PhantomData<*mut ()>,
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
        let cycles = oer_riscv::cycle64();
        let instructions = oer_riscv::instret64();
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

/// The PC sampler on TIMG1 T0, at P4 above every other level of the boot
/// hart: one sample of the interrupted PC and `ra` every period while a
/// session runs, into one buffer of [`SAMPLES`].
pub struct Sampler {
    sampler: sampler::Sampler<Timer<SamplerToken>>,
    period_us: u32,
    ended: Option<Ended>,
}

impl Sampler {
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
        if !self.available() || KEPT.lend(|buffer| buffer.restart(period_us)).is_err() {
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
            let _ = KEPT.lend(|buffer| buffer.normalize());
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
        KEPT.lend(|buffer| read(buffer)).ok()
    }
}

/// Stop and mask TIMG1 T0, bind the sampler's handler and install its
/// buffer: the boot hart's handle. Fails the boot when its storage was
/// taken before.
pub(crate) fn hart0(
    timg1: Timg1AlarmParts,
    cache: CacheCounters,
    boot: &mut Boot,
    token: SamplerToken,
) -> Hart0 {
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
