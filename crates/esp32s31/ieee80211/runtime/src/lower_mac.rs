//! The ESP32-S31 Wi-Fi backend on the owner shape of the lower-MAC port
//! (`oer_ieee80211_lower_mac::owner`, `oer-ieee80211-port`).
//!
//! [`Esp32s31LowerMac`] is the runtime a composition places in a `static`:
//! the sans-IO [`LowerMacCore`] of `oer-esp32s31-ieee80211` and its register
//! owner in one blocking mutex over a caller-chosen raw mutex, the event
//! queues, and the entries the interrupts and the receive producer call
//! ([`Esp32s31LowerMac::on_interrupt`], [`Esp32s31LowerMac::on_power_interrupt`],
//! [`Esp32s31LowerMac::on_received`]). Those entries only post: they run
//! the core's service under the lock and queue its events.
//!
//! [`Esp32s31LowerMac::install`] installs a disabled core with its register
//! owner, its retune and the install's access-point address, and returns
//! the [`Esp32s31Backend`], which the composition hands to
//! [`run_port`](oer_ieee80211_port::run_port) with the port's body. The
//! backend is direct ([`PortBackend::Held`]): its held part keeps the
//! transmit permits ([`TxPermits`]) and the receive Block Ack agreements,
//! the parts reach the core synchronously, and its driver
//! ([`PortBackend::drive`]) serves what the core runs in software: the
//! publication watchdog of the attempts in flight, the interrupt causes
//! deferred while a retune held the registers, and the permits of every
//! attempt whose completion the core reported, which it gives back before
//! the event consumer hands the completion out.
//!
//! `Enable` is one bounded step of the owner: it enables the core and, when
//! the core asks for it after a channel change, retunes the PHY through the
//! [`LowerMacRetune`] it holds, under the shared radio's lease
//! ([`Esp32s31PortRetune`]). A refused retune leaves the port disabled
//! ([`RetuneRefused`]). The core's lifecycle terminals reach the owner's
//! transitions, never the event consumer.
//!
//! Each of the four ordinary EDCA queues holds one attempt, which the
//! queue permit of [`TxPermits`] proves; completions leave in whatever
//! order the queues end, correlated by `TxId`. Received frames hold their
//! receive unit (on the target, a staging-pool DMA buffer) until the
//! consumer drops it. A fault poisons the port: the queued events are still
//! reported, then [`Poisoned`] with the [`LowerMacFault`] at every call.
//!
//! The receive Block Ack slots are one pool both roles share, as the
//! hardware's banks are (owner decision, Claude Code chat with common,
//! 2026-10-12).
//!
//! This backend has no A-MPDU, beacon-timing or monitor operation: the
//! owner-shape contract has none yet, and the product datapath keeps its
//! direct path until it moves onto the port ( step 2).

use core::{
    cell::{Cell, RefCell},
    convert::Infallible,
    future::{Future, poll_fn, ready},
    marker::PhantomData,
    ops::Range,
    task::{Poll, Waker},
};

use embassy_futures::select::{Either3, select, select3};
use embassy_sync::{
    blocking_mutex::{Mutex, raw::RawMutex},
    channel::Channel,
    signal::Signal,
};
use oer_esp32s31_hal::types::MacPowerInterruptObservation;
use oer_esp32s31_ieee80211::{
    lower_mac::{
        CheckedKeyScope, Esp32s31TxBuffer, LifecycleStart, LowerMacCore, LowerMacFault,
        LowerMacHardware, LowerMacSink, MpduValues, esp32s31_lower_mac_capabilities,
    },
    ordinary_tx::{WifiTxEntropy, WifiTxPowerProfile},
    tx::WifiTxWake,
};
use oer_esp32s31_ieee80211_mac::{
    rx::{
        NormalizedRxFrame, RxError, RxIngressConfig, pool::NetworkRxFrame, view_normalized_rx_frame,
    },
    tx::TxPhyRate,
};
use oer_ieee80211_lower_mac::{
    AccessPoint, AccessPointClient, AttemptKey, Backoff, BssColor, CancelError, Ccmp128Key,
    ClockError, EventsLost, GroupKeyId, Ieee80211Capabilities, Ieee80211Instant, Ieee80211Radio,
    IndividualAddress, InterfaceRole, KeyHandle, LifecycleEvent, LowerMacBackend,
    LowerMacCapabilities, LowerMacClient, LowerMacControl, LowerMacEvent, LowerMacEvents,
    LowerMacSetting, LowerMacSetup, MacAddress, PhaseAttempt, Poisoned, PortResult, ReceiveFilter,
    RxBeaconPriority, RxBlockAckAgreement, RxBuffer, RxEvidence, RxMeta, Station, StationClient,
    StationReceive, SupportedBlockAck, SupportedChannel, SupportedEdca, SupportedFilter, TbttEvent,
    Tid, TxBody, TxBuffer, TxCompletion, TxFrame, TxFrameBody, TxGated, TxId, TxPower, TxSeats,
    VifConfig, VifId, VifRole,
};
use oer_ieee80211_port::{
    EnableResult, Lent, PhaseParts, PortBackend, RadioPortClient, RadioPortEvents, RadioWork,
    Retire, Work,
};
use oer_ieee80211_runtime::lower_mac_owner::{
    BufferPermit, QueuePermit, TxBufferKind, TxPermits, TxQueueKind,
};
use oer_radio_permit_pool::FifoPermitPool;
use oer_radio_port::{Mint, Seat, Slot};
use oer_time::Duration;

use crate::mac_clock::ReceptionClock;
use oer_ieee80211_mac::channel::WifiChannel;

/// The radio system's coexistence priorities, as its last released guard
/// left them: the source a core asks for the air with
/// ([`LowerMacCore::new`]).
#[cfg(target_arch = "riscv32")]
pub struct RadioCoex<'a>(pub &'a oer_esp32s31_radio_runtime::WifiCoexViewCell);

#[cfg(target_arch = "riscv32")]
impl oer_esp32s31_ieee80211::lower_mac::RadioCoexPriorities for RadioCoex<'_> {
    fn beacon_window_pti(&self) -> oer_esp32s31_hal::types::MacPti {
        oer_esp32s31_hal::types::MacPti::new(u32::from(self.0.get().beacon_pti.value()))
            .expect("coexistence priorities are four-bit values")
    }

    fn connection_frame_priorities(
        &self,
    ) -> oer_esp32s31_ieee80211::lower_mac::ConnectionFramePriorities {
        let view = self.0.get();
        let packet = view.connection_pti.value();
        oer_esp32s31_ieee80211::lower_mac::ConnectionFramePriorities {
            packet,
            scheduler: packet.min(view.slice_pti.value()),
        }
    }
}

/// Attempt completions the port holds at most: one per queue, twice over
/// for completions the consumer has not taken yet.
pub const COMPLETION_CAPACITY: usize = 8;
/// Lifecycle terminals the core owes the owner's transitions at most.
const LIFECYCLE_CAPACITY: usize = 4;
/// TBTT events waiting for the consumer, a loss marker included.
pub const TBTT_CAPACITY: usize = 2;
/// Transmit queues of the ESP32-S31: the four ordinary EDCA queues.
pub const TX_QUEUES: usize = 4;
/// Pairwise key slots of the station: its access point's.
pub const STATION_PAIRWISE_KEYS: usize = 1;
/// Group key slots of the station.
pub const STATION_GROUP_KEYS: usize = 1;
/// Pairwise key slots of the access point: one per association
/// identifier, which a slot's index plus one is.
pub const ACCESS_POINT_PAIRWISE_KEYS: usize = 15;
/// Group key slots of the access point.
pub const ACCESS_POINT_GROUP_KEYS: usize = 1;
/// Receive Block Ack banks, which both roles share.
pub const RX_BLOCK_ACK_SLOTS: usize = 8;
/// A BlockAckReq without its FCS: Frame Control, Duration, RA, TA, BAR
/// Control and Starting Sequence Control.
const BLOCK_ACK_REQUEST_LEN: usize = 20;
/// How long a driver asked to retire may take to return.
const RETIRE_BOUND: Duration = Duration::from_millis(10);

const COMPLETION_SLOTS: usize = COMPLETION_CAPACITY + 1;
const LIFECYCLE_SLOTS: usize = LIFECYCLE_CAPACITY + 1;

/// The interface of a role: the station's is zero, the access point's one.
const fn vif_of(role: VifRole) -> VifId {
    match role {
        VifRole::Station => VifId(0),
        VifRole::AccessPoint => VifId(1),
    }
}

/// The PHY retune of an `Enable` after a channel change, over the port's
/// register owner `H`. The ESP32-S31 retunes through
/// `switch_published_esp32s31_wifi_channel` under the shared-radio lease
/// ([`Esp32s31PortRetune`]).
pub trait LowerMacRetune<H> {
    /// Tune to `channel` through `hardware`; `false` when the PHY refused.
    fn retune(&mut self, hardware: &mut H, channel: WifiChannel) -> impl Future<Output = bool>;
}

/// The radio refused the retune of an `Enable`: the port stays disabled on
/// the channel it was tuned to.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RetuneRefused;

/// One unit of the receive producer: an owned receive buffer whose MPDU and
/// hardware metadata the port reads in place. Dropping it returns the
/// buffer to its producer.
pub trait LowerMacRxUnit {
    /// The whole receive buffer, the hardware's prefix included.
    fn buffer(&self) -> &[u8];
    /// The MPDU and metadata the hardware reported for the unit.
    fn normalized(&self) -> Result<NormalizedRxFrame<'_>, RxError>;
}

/// A staging-pool DMA buffer, the target's receive unit, with the ingress
/// configuration of the ring it came from.
pub struct Esp32s31StagedRx<'pool, const SLOTS: usize, const CAPACITY: usize> {
    frame: NetworkRxFrame<'pool, SLOTS, CAPACITY>,
    config: RxIngressConfig,
}

impl<'pool, const SLOTS: usize, const CAPACITY: usize> Esp32s31StagedRx<'pool, SLOTS, CAPACITY> {
    pub const fn new(
        frame: NetworkRxFrame<'pool, SLOTS, CAPACITY>,
        config: RxIngressConfig,
    ) -> Self {
        Self { frame, config }
    }

    /// The staged frame back, for its producer to keep.
    pub fn into_frame(self) -> NetworkRxFrame<'pool, SLOTS, CAPACITY> {
        self.frame
    }
}

impl<const SLOTS: usize, const CAPACITY: usize> LowerMacRxUnit
    for Esp32s31StagedRx<'_, SLOTS, CAPACITY>
{
    fn buffer(&self) -> &[u8] {
        self.frame.segment().buffer
    }

    fn normalized(&self) -> Result<NormalizedRxFrame<'_>, RxError> {
        view_normalized_rx_frame(&self.frame.segment(), self.config)
    }
}

/// A received MPDU the port lends: the receive unit itself, with where the
/// MPDU lies in its buffer. Nothing is copied.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Esp32s31RxBuffer<U> {
    unit: U,
    mpdu: Range<usize>,
}

impl<U: LowerMacRxUnit> RxBuffer for Esp32s31RxBuffer<U> {
    fn bytes(&self) -> &[u8] {
        // The port located the MPDU when it received the unit.
        self.unit
            .buffer()
            .get(self.mpdu.clone())
            .unwrap_or_default()
    }
}

/// One owned event of the port. An attempt's completion owns the body of
/// its attempt.
#[derive(Debug)]
pub enum Esp32s31LowerMacEvent<U, O> {
    /// A received MPDU.
    Received {
        frame: Esp32s31RxBuffer<U>,
        meta: RxMeta,
    },
    TxCompleted(TxCompletion, Option<O>),
    /// A station TBTT.
    Tbtt(TbttEvent),
}

/// A bounded queue whose overflow is reported in its order: a loss marker
/// takes the place of the first dropped entry. Each entry keeps the
/// occurrence number it was queued with, so the port merges its queues in
/// the order the events occurred.
struct EventQueue<M: RawMutex, T, const N: usize> {
    entries: Channel<M, Result<T, EventsLost>, N>,
    /// The occurrence of each queued entry, in the same order.
    occurrences: Channel<M, u64, N>,
    /// The occurrence of the first dropped entry whose marker is not queued
    /// yet.
    lost: Mutex<M, Cell<Option<u64>>>,
}

impl<M: RawMutex, T, const N: usize> EventQueue<M, T, N> {
    const fn new() -> Self {
        Self {
            entries: Channel::new(),
            occurrences: Channel::new(),
            lost: Mutex::new(Cell::new(None)),
        }
    }

    /// Queue the marker of a pending loss, if there is room for it.
    fn queue_marker(&self, lost: &Cell<Option<u64>>) -> bool {
        let Some(first) = lost.get() else {
            return true;
        };
        if self.entries.try_send(Err(EventsLost)).is_err() {
            return false;
        }
        let _ = self.occurrences.try_send(first);
        lost.set(None);
        true
    }

    /// Queue `entry`, which occurred as `occurrence`, behind a pending loss
    /// marker; drop it and remember the loss when there is no room.
    fn push(&self, occurrence: u64, entry: T) {
        self.lost.lock(|lost| {
            if !self.queue_marker(lost) {
                return;
            }
            if self.entries.try_send(Ok(entry)).is_err() {
                lost.set(Some(occurrence));
            } else {
                let _ = self.occurrences.try_send(occurrence);
            }
        });
    }

    /// Queue `entry`, which occurred as `occurrence`, behind a pending loss
    /// marker, or hand it back when there is no room for both: nothing is
    /// lost.
    fn try_push(&self, occurrence: u64, entry: T) -> Result<(), T> {
        self.lost.lock(|lost| {
            let marker = usize::from(lost.get().is_some());
            if self.entries.free_capacity() < marker + 1 {
                return Err(entry);
            }
            self.queue_marker(lost);
            self.entries
                .try_send(Ok(entry))
                .map(|()| {
                    let _ = self.occurrences.try_send(occurrence);
                })
                .map_err(|error| match error {
                    embassy_sync::channel::TrySendError::Full(Ok(entry)) => entry,
                    embassy_sync::channel::TrySendError::Full(Err(_)) => {
                        unreachable!("an entry is sent as `Ok`")
                    }
                })
        })
    }

    /// Entries that fit now.
    fn room(&self) -> usize {
        self.lost.lock(|lost| {
            self.entries
                .free_capacity()
                .saturating_sub(usize::from(lost.get().is_some()))
        })
    }

    /// The occurrence of the entry [`Self::take`] would return next.
    fn head(&self) -> Option<u64> {
        self.lost
            .lock(|lost| self.occurrences.try_peek().ok().or(lost.get()))
    }

    /// The next entry, or the loss marker after the last entry.
    fn take(&self) -> Option<Result<T, EventsLost>> {
        self.lost.lock(|lost| match self.entries.try_receive() {
            Ok(entry) => {
                let _ = self.occurrences.try_receive();
                Some(entry)
            }
            Err(_) if lost.take().is_some() => Some(Err(EventsLost)),
            Err(_) => None,
        })
    }

    /// Discard every entry and a pending loss: the stream of the port that
    /// left ends here.
    fn discard(&self) {
        self.lost.lock(|lost| {
            while let Ok(_entry) = self.entries.try_receive() {}
            while let Ok(_occurrence) = self.occurrences.try_receive() {}
            lost.set(None);
        });
    }
}

/// An event the port queues; a completion takes the body of its attempt
/// only as the consumer takes it, so the queues hold no body.
#[derive(Debug)]
enum QueuedEvent<U> {
    Received {
        frame: Esp32s31RxBuffer<U>,
        meta: RxMeta,
    },
    TxCompleted(TxCompletion),
    Tbtt(TbttEvent),
}

/// The queues of the port, one per kind of event, and the lifecycle
/// terminals the owner's transitions read.
struct Queues<M: RawMutex, const EVENTS: usize, U: LowerMacRxUnit> {
    completions: EventQueue<M, TxCompletion, COMPLETION_SLOTS>,
    lifecycle: EventQueue<M, LifecycleEvent, LIFECYCLE_SLOTS>,
    tbtt: EventQueue<M, TbttEvent, TBTT_CAPACITY>,
    received: EventQueue<M, QueuedEvent<U>, EVENTS>,
    /// The occurrence number of the next event, one sequence for every
    /// queue: the stream is merged in this order. Its lock also orders the
    /// queues: a producer numbers and queues an event, and the consumer
    /// picks and takes the earliest head, each under it.
    next_occurrence: Mutex<M, Cell<u64>>,
    /// Raised when the consumer must look again without a new entry: a
    /// poisoning.
    changed: Signal<M, ()>,
}

impl<M: RawMutex, const EVENTS: usize, U: LowerMacRxUnit> Queues<M, EVENTS, U> {
    const fn new() -> Self {
        Self {
            completions: EventQueue::new(),
            lifecycle: EventQueue::new(),
            tbtt: EventQueue::new(),
            received: EventQueue::new(),
            next_occurrence: Mutex::new(Cell::new(0)),
            changed: Signal::new(),
        }
    }

    /// Queue an event happening now through `queue`, which receives its
    /// occurrence number, under the order lock.
    fn ordered<R>(&self, queue: impl FnOnce(u64) -> R) -> R {
        self.next_occurrence.lock(|next| {
            let occurrence = next.get();
            next.set(occurrence.wrapping_add(1));
            queue(occurrence)
        })
    }

    /// The next queued event of the consumer's stream, in the order the
    /// events occurred.
    fn take(&self) -> Option<Result<QueuedEvent<U>, EventsLost>> {
        self.next_occurrence.lock(|_| {
            let heads = [
                self.completions.head(),
                self.tbtt.head(),
                self.received.head(),
            ];
            let (first, _) = heads
                .iter()
                .enumerate()
                .filter_map(|(queue, head)| Some((queue, (*head)?)))
                .min_by_key(|(_, occurrence)| *occurrence)?;
            match first {
                0 => Some(self.completions.take()?.map(QueuedEvent::TxCompleted)),
                1 => Some(self.tbtt.take()?.map(QueuedEvent::Tbtt)),
                _ => self.received.take(),
            }
        })
    }

    /// Wait until a queue of the stream may have an entry or
    /// [`Self::changed`] rose.
    async fn wait(&self) {
        select3(
            self.completions.entries.ready_to_receive(),
            select(
                self.tbtt.entries.ready_to_receive(),
                self.received.entries.ready_to_receive(),
            ),
            self.changed.wait(),
        )
        .await;
    }

    /// End the stream of the port that left.
    fn discard(&self) {
        self.completions.discard();
        self.lifecycle.discard();
        self.tbtt.discard();
        self.received.discard();
    }
}

/// The installed core and its register owner.
struct Installed<'slot, P, E, T, H, const BUFFER_SIZE: usize, const TX_BUFFERS: usize> {
    core: LowerMacCore<'slot, P, E, T, BUFFER_SIZE, TX_BUFFERS>,
    /// `None` while lent to the retune of an `Enable`.
    hardware: Option<H>,
    /// The access point's address, which the install gave.
    access_point: MacAddress,
}

/// The attempts whose completion the core reported and whose permits the
/// driver has not given back yet.
type Ended = [Option<TxId>; COMPLETION_CAPACITY];

/// The sink of one locked entry: events go to their queues, and every
/// completion's attempt to the ended set.
struct QueueSink<'a, M: RawMutex, const EVENTS: usize, U: LowerMacRxUnit> {
    queues: &'a Queues<M, EVENTS, U>,
    ended: &'a Mutex<M, RefCell<Ended>>,
}

impl<M: RawMutex, const EVENTS: usize, U: LowerMacRxUnit> LowerMacSink
    for QueueSink<'_, M, EVENTS, U>
{
    fn tx_completed(&mut self, completion: TxCompletion) {
        let queues = self.queues;
        self.ended.lock(|ended| {
            if let Some(free) = ended.borrow_mut().iter_mut().find(|id| id.is_none()) {
                *free = Some(completion.id);
            }
        });
        queues.ordered(|occurrence| queues.completions.push(occurrence, completion));
    }

    fn lifecycle(&mut self, event: LifecycleEvent) {
        let queues = self.queues;
        queues.ordered(|occurrence| queues.lifecycle.push(occurrence, event));
    }

    fn tbtt(&mut self, event: TbttEvent) {
        let queues = self.queues;
        queues.ordered(|occurrence| queues.tbtt.push(occurrence, event));
    }
}

/// Interrupt causes that arrived while the retune held the registers.
#[derive(Clone, Copy)]
struct Deferred {
    events: u32,
    station_tbtt: bool,
}

impl Deferred {
    const NONE: Self = Self {
        events: 0,
        station_tbtt: false,
    };
}

/// The wakers of the futures that wait for the port to settle: a closing
/// gate, a key removal, an interface configuration.
const SETTLE_WAITERS: usize = 4;

/// The ESP32-S31 lower MAC's runtime: the installed core, its queues and
/// its interrupt and receive entries. `TX_BUFFERS` counts the transmit
/// buffers the core lends, `EVENTS` bounds the received frames waiting for
/// the consumer (a loss marker included), `U` is the unit of the receive
/// producer, which a received frame holds until the consumer drops it, and
/// `O` is the body an attempt carries.
pub struct Esp32s31LowerMac<
    'slot,
    M: RawMutex,
    P,
    E,
    T,
    H,
    R,
    const BUFFER_SIZE: usize,
    const TX_BUFFERS: usize,
    const EVENTS: usize,
    U: LowerMacRxUnit,
    O = Infallible,
> {
    #[expect(
        clippy::type_complexity,
        reason = "the mutex retains the installed core's concrete hardware owner types"
    )]
    installed: Mutex<M, RefCell<Option<Installed<'slot, P, E, T, H, BUFFER_SIZE, TX_BUFFERS>>>>,
    retune: Mutex<M, RefCell<Option<R>>>,
    deferred: Mutex<M, Cell<Deferred>>,
    fault: Mutex<M, Cell<Option<LowerMacFault>>>,
    queues: Queues<M, EVENTS, U>,
    ended: Mutex<M, RefCell<Ended>>,
    /// Raised when a deadline, an install, a completion or a fault may have
    /// started, so the driver rearms.
    wake: Signal<M, ()>,
    settle: Mutex<M, RefCell<embassy_sync::waitqueue::MultiWakerRegistration<SETTLE_WAITERS>>>,
    /// The image's monotonic time the driver's watchdog waits on; the same
    /// time the installed core's transmit owner reads, and the MAC clock.
    timer: T,
    /// The bodies of admitted attempts until their completion events carry
    /// them.
    #[expect(
        clippy::type_complexity,
        reason = "bounded attempt storage pairs each admitted transmit ID with its owned body"
    )]
    bodies: Mutex<M, RefCell<[Option<(TxId, O)>; COMPLETION_CAPACITY]>>,
    next_id: Mutex<M, Cell<u32>>,
}

/// The core, its register owner and the retune of `Enable`.
pub struct Esp32s31LowerMacParts<
    'slot,
    P,
    E,
    T,
    H,
    R,
    const BUFFER_SIZE: usize,
    const TX_BUFFERS: usize,
> {
    pub core: LowerMacCore<'slot, P, E, T, BUFFER_SIZE, TX_BUFFERS>,
    pub hardware: H,
    pub retune: R,
}

impl<
    'slot,
    M: RawMutex,
    P,
    E,
    T,
    H,
    R,
    const BUFFER_SIZE: usize,
    const TX_BUFFERS: usize,
    const EVENTS: usize,
    U: LowerMacRxUnit,
    O: TxBody,
> Esp32s31LowerMac<'slot, M, P, E, T, H, R, BUFFER_SIZE, TX_BUFFERS, EVENTS, U, O>
where
    P: WifiTxPowerProfile,
    E: WifiTxEntropy,
    T: oer_time::Timer + ReceptionClock,
    H: LowerMacHardware,
    R: LowerMacRetune<H>,
{
    /// An empty runtime whose driver waits on `timer`, suitable for a
    /// `static`.
    pub const fn new(timer: T) -> Self {
        Self {
            timer,
            installed: Mutex::new(RefCell::new(None)),
            retune: Mutex::new(RefCell::new(None)),
            deferred: Mutex::new(Cell::new(Deferred::NONE)),
            fault: Mutex::new(Cell::new(None)),
            queues: Queues::new(),
            ended: Mutex::new(RefCell::new([None; COMPLETION_CAPACITY])),
            wake: Signal::new(),
            settle: Mutex::new(RefCell::new(
                embassy_sync::waitqueue::MultiWakerRegistration::new(),
            )),
            bodies: Mutex::new(RefCell::new([const { None }; COMPLETION_CAPACITY])),
            next_id: Mutex::new(Cell::new(0)),
        }
    }

    /// Install a disabled core with its register owner and retune, on
    /// `channel`, its access-point interface at `access_point`, and return
    /// the backend, whose stream starts empty.
    ///
    /// # Errors
    ///
    /// Returns the parts unchanged if a backend is already installed.
    #[allow(
        clippy::result_large_err,
        clippy::type_complexity,
        reason = "the no-alloc runtime returns the unconsumed owners by value"
    )]
    pub fn install(
        &self,
        parts: Esp32s31LowerMacParts<'slot, P, E, T, H, R, BUFFER_SIZE, TX_BUFFERS>,
        channel: SupportedChannel<Esp32s31Caps<BUFFER_SIZE>>,
        access_point: IndividualAddress,
    ) -> Result<
        Esp32s31Backend<'_, 'slot, M, P, E, T, H, R, BUFFER_SIZE, TX_BUFFERS, EVENTS, U, O>,
        Esp32s31LowerMacParts<'slot, P, E, T, H, R, BUFFER_SIZE, TX_BUFFERS>,
    > {
        let Esp32s31LowerMacParts {
            mut core,
            hardware,
            retune,
        } = parts;
        let refused = self.installed.lock(|installed| {
            let mut installed = installed.borrow_mut();
            if installed.is_some() {
                return Some((core, hardware));
            }
            // A supported channel on a disabled core is always taken.
            let _ = core.set_channel(channel.channel());
            *installed = Some(Installed {
                core,
                hardware: Some(hardware),
                access_point: access_point.get(),
            });
            None
        });
        if let Some((core, hardware)) = refused {
            return Err(Esp32s31LowerMacParts {
                core,
                hardware,
                retune,
            });
        }
        self.retune.lock(|slot| *slot.borrow_mut() = Some(retune));
        self.fault.lock(|fault| fault.set(None));
        self.wake.signal(());
        Ok(Esp32s31Backend { runtime: self })
    }

    /// Take the core and its register owner back and discard queued events;
    /// `None`, and nothing changed, unless the core is disabled and idle
    /// with its registers home.
    #[allow(
        clippy::type_complexity,
        reason = "the uninstalled owners are returned as they were installed"
    )]
    fn uninstall(&self) -> Option<(LowerMacCore<'slot, P, E, T, BUFFER_SIZE, TX_BUFFERS>, H)> {
        let (core, hardware) = self.installed.lock(|installed| {
            let mut installed = installed.borrow_mut();
            let stopped = installed.as_ref()?;
            if !stopped.core.idle_disabled() || stopped.hardware.is_none() {
                return None;
            }
            installed
                .take()
                .and_then(|installed| Some((installed.core, installed.hardware?)))
        })?;
        self.queues.discard();
        self.bodies.lock(|bodies| {
            for held in bodies.borrow_mut().iter_mut() {
                *held = None;
            }
        });
        self.ended
            .lock(|ended| *ended.borrow_mut() = [None; COMPLETION_CAPACITY]);
        self.deferred.lock(|deferred| deferred.set(Deferred::NONE));
        Some((core, hardware))
    }

    /// Run one entry over the installed core and its registers under the
    /// lock; `None` without a core or while a retune holds the registers. A
    /// fault poisons the port.
    #[allow(
        clippy::type_complexity,
        reason = "the installed owners keep their types"
    )]
    fn with_installed<V>(
        &self,
        entry: impl FnOnce(
            &mut LowerMacCore<'slot, P, E, T, BUFFER_SIZE, TX_BUFFERS>,
            &mut H,
            MacAddress,
            &mut QueueSink<'_, M, EVENTS, U>,
        ) -> Result<V, LowerMacFault>,
    ) -> Result<Option<V>, Poisoned<LowerMacFault>> {
        if let Some(cause) = self.fault.lock(Cell::get) {
            return Err(Poisoned { cause });
        }
        let result = self.installed.lock(|installed| {
            let mut installed = installed.borrow_mut();
            let Some(installed) = installed.as_mut() else {
                return Ok(None);
            };
            let Some(hardware) = installed.hardware.as_mut() else {
                return Ok(None);
            };
            let mut sink = QueueSink {
                queues: &self.queues,
                ended: &self.ended,
            };
            entry(
                &mut installed.core,
                hardware,
                installed.access_point,
                &mut sink,
            )
            .map(Some)
        });
        let answer = result.map_err(|cause| self.poison(cause));
        self.settled();
        answer
    }

    /// As [`Self::with_installed`], for an entry of a running phase: the
    /// core is installed and its registers home.
    fn with_phase<V>(
        &self,
        entry: impl FnOnce(
            &mut LowerMacCore<'slot, P, E, T, BUFFER_SIZE, TX_BUFFERS>,
            &mut H,
            MacAddress,
            &mut QueueSink<'_, M, EVENTS, U>,
        ) -> Result<V, LowerMacFault>,
    ) -> Result<V, Poisoned<LowerMacFault>> {
        self.with_installed(entry)
            .map(|answer| answer.expect("a phase runs over the installed core"))
    }

    /// Record `cause`: the consumer learns of it after the queued events,
    /// and the driver parks.
    fn poison(&self, cause: LowerMacFault) -> Poisoned<LowerMacFault> {
        self.fault.lock(|poisoned| poisoned.set(Some(cause)));
        self.queues.changed.signal(());
        self.wake.signal(());
        Poisoned { cause }
    }

    fn poisoned(&self) -> Option<Poisoned<LowerMacFault>> {
        self.fault.lock(Cell::get).map(|cause| Poisoned { cause })
    }

    /// Wake every future waiting for the port to settle.
    fn settled(&self) {
        self.settle.lock(|settle| settle.borrow_mut().wake());
    }

    /// Wait for the port to settle again with `context`'s waker.
    fn wait_settled(&self, waker: &Waker) {
        self.settle
            .lock(|settle| settle.borrow_mut().register(waker));
    }

    /// The MAC interrupt handler's TX edges: completion, hardware timeout
    /// and collision events of `oer_esp32s31_ieee80211_mac::irq`.
    pub fn on_interrupt(&self, events: u32) {
        if let Ok(None) = self.with_installed(|core, hardware, _, sink| {
            core.service(hardware, WifiTxWake::Interrupt { events }, sink)
        }) {
            self.deferred.lock(|deferred| {
                let mut cause = deferred.get();
                cause.events |= events;
                deferred.set(cause);
            });
        }
        // A timeout starts the abort-settle deadline, and a completion's
        // permits go back through the driver.
        self.wake.signal(());
    }

    /// The power interrupt's semantic causes: a station TBTT is reported as
    /// an event while its schedule runs.
    pub fn on_power_interrupt(&self, observation: MacPowerInterruptObservation) {
        if observation.sta_tbtt()
            && let Ok(None) = self.with_installed(|core, hardware, _, sink| {
                core.station_tbtt(hardware, sink);
                Ok(())
            })
        {
            self.deferred.lock(|deferred| {
                let mut cause = deferred.get();
                cause.station_tbtt = true;
                deferred.set(cause);
            });
        }
    }

    /// One unit of the receive producer, queued as it is while the port
    /// receives and a receive rule admits its MPDU; a unit the port does not
    /// queue is dropped, which returns its buffer. A unit whose hardware
    /// report does not decode is the error.
    pub fn on_received(&self, unit: U) -> Result<(), RxError> {
        self.receive(unit, false).map(|_| ())
    }

    /// Received frames that fit in the port's queue now.
    pub fn received_room(&self) -> usize {
        self.queues.received.room()
    }

    /// As [`Self::on_received`], but a unit the port would queue and has
    /// no room for comes back: nothing is lost, and its producer keeps it
    /// until there is room.
    pub fn try_on_received(&self, unit: U) -> Result<Result<(), U>, RxError> {
        self.receive(unit, true)
    }

    /// Queue `unit` when a receive rule admits it; without room, hand it
    /// back when `keep`, else drop it as a loss.
    fn receive(&self, unit: U, keep: bool) -> Result<Result<(), U>, RxError> {
        let frame = unit.normalized()?;
        let stamp = frame
            .stamp
            .and_then(|raw| self.timer.snapshot()?.stamp(raw))
            .map_or(RxEvidence::Unavailable, RxEvidence::HardwareObserved);
        let admitted = self.with_installed(|core, hardware, _, _| {
            Ok(core.received(&*hardware, &frame).map(|(bytes, mut meta)| {
                meta.timestamp = stamp;
                (frame.mpdu_offset..frame.mpdu_offset + bytes.len(), meta)
            }))
        });
        // The enabling port, and one without a core, receives nothing.
        let Ok(Some(Some((mpdu, meta)))) = admitted else {
            return Ok(Ok(()));
        };
        let event = QueuedEvent::Received {
            frame: Esp32s31RxBuffer { unit, mpdu },
            meta,
        };
        let mut refused = None;
        let queues = &self.queues;
        queues.ordered(|occurrence| {
            if keep {
                refused = queues.received.try_push(occurrence, event).err();
            } else {
                queues.received.push(occurrence, event);
            }
        });
        Ok(match refused {
            Some(QueuedEvent::Received { frame, .. }) => Err(frame.unit),
            _ => Ok(()),
        })
    }

    /// Hold attempt `id`'s body until its completion carries it.
    fn hold_body(&self, id: TxId, body: O) {
        self.bodies.lock(|bodies| {
            let mut bodies = bodies.borrow_mut();
            let free = bodies
                .iter_mut()
                .find(|held| held.is_none())
                .expect("a body for each queue's attempt in flight");
            *free = Some((id, body));
        });
    }

    /// Take back the body of attempt `id`, whose completion is taken.
    fn unhold_body(&self, id: TxId) -> Option<O> {
        self.bodies.lock(|bodies| {
            bodies
                .borrow_mut()
                .iter_mut()
                .find(|held| held.as_ref().is_some_and(|held| held.0 == id))
                .and_then(Option::take)
                .map(|held| held.1)
        })
    }

    /// A fresh attempt identity.
    fn next_id(&self) -> TxId {
        self.next_id.lock(|next| {
            let id = next.get();
            next.set(id.wrapping_add(1));
            TxId(id)
        })
    }

    /// The attempts the core ended since the last call: their permits go
    /// back before their completions are handed out.
    fn take_ended(&self) -> Ended {
        self.ended
            .lock(|ended| core::mem::replace(&mut *ended.borrow_mut(), [None; COMPLETION_CAPACITY]))
    }

    /// The radio clock: the MAC local time, the counter of receive
    /// timestamps, affine to the monotonic clock.
    fn now(&self) -> PortResult<Ieee80211Instant, ClockError, LowerMacFault> {
        if let Some(poisoned) = self.poisoned() {
            return Err(poisoned);
        }
        Ok(self
            .timer
            .snapshot()
            .map(|snapshot| snapshot.sample().source)
            .ok_or(ClockError::Unavailable))
    }

    /// Service the interrupt causes deferred while the retune held the
    /// registers.
    fn service_deferred(&self) {
        let deferred = self
            .deferred
            .lock(|deferred| deferred.replace(Deferred::NONE));
        if deferred.events != 0 {
            self.on_interrupt(deferred.events);
        }
        if deferred.station_tbtt {
            let _ = self.with_installed(|core, hardware, _, sink| {
                core.station_tbtt(hardware, sink);
                Ok(())
            });
        }
    }

    /// The next lifecycle terminal the core reported.
    async fn lifecycle_event(&self) -> Result<LifecycleEvent, Poisoned<LowerMacFault>> {
        loop {
            if let Some(Ok(event)) = self.queues.lifecycle.take() {
                return Ok(event);
            }
            if let Some(poisoned) = self.poisoned() {
                return Err(poisoned);
            }
            select(
                self.queues.lifecycle.entries.ready_to_receive(),
                self.queues.changed.wait(),
            )
            .await;
        }
    }
}

/// The capabilities marker of an ESP32-S31 backend with transmit slots of
/// `BUFFER_SIZE` bytes.
pub struct Esp32s31Caps<const BUFFER_SIZE: usize>;

impl<const BUFFER_SIZE: usize> Ieee80211Capabilities for Esp32s31Caps<BUFFER_SIZE> {
    const CAPS: LowerMacCapabilities = esp32s31_lower_mac_capabilities(BUFFER_SIZE);
}

/// The transmit permits of one install: a pool per queue, each served
/// first in, first out, and the buffers' pool.
pub type Esp32s31TxPermits<'id, const BUFFER_SIZE: usize, const TX_BUFFERS: usize> = TxPermits<
    'id,
    Esp32s31Caps<BUFFER_SIZE>,
    FifoPermitPool<'id, TxQueueKind, 1, { oer_ieee80211_lower_mac::TX_SEATS }>,
    FifoPermitPool<'id, TxBufferKind, TX_BUFFERS, { oer_ieee80211_lower_mac::TX_SEATS }>,
    TX_QUEUES,
>;

/// The kind of a role's pairwise key slots.
pub struct Esp32s31PairwiseKeys<R>(PhantomData<fn() -> R>);
/// The kind of a role's group key slots.
pub struct Esp32s31GroupKeys<R>(PhantomData<fn() -> R>);
/// The kind of the receive Block Ack slots, which both roles share.
pub struct Esp32s31BlockAcks;

/// One receive Block Ack agreement and its slot.
struct Agreement<'id> {
    role: VifRole,
    peer: IndividualAddress,
    tid: Tid,
    slot: Slot<'id, Esp32s31BlockAcks>,
}

/// One attempt in flight: its role and the key it uses, which a key
/// removal and an interface configuration wait for.
#[derive(Clone, Copy)]
struct InFlight {
    id: TxId,
    role: VifRole,
    key: Option<KeyHandle>,
}

/// The backend's held part of one install: its transmit permits, the
/// attempts in flight and the receive Block Ack agreements.
pub struct Esp32s31Held<'id, const BUFFER_SIZE: usize, const TX_BUFFERS: usize> {
    tx: Esp32s31TxPermits<'id, BUFFER_SIZE, TX_BUFFERS>,
    in_flight: RefCell<[Option<InFlight>; TX_QUEUES]>,
    agreements: RefCell<[Option<Agreement<'id>>; RX_BLOCK_ACK_SLOTS]>,
}

impl<const BUFFER_SIZE: usize, const TX_BUFFERS: usize> Esp32s31Held<'_, BUFFER_SIZE, TX_BUFFERS> {
    /// Give back the permits of every attempt in `ended`.
    fn complete(&self, ended: Ended) {
        for id in ended.into_iter().flatten() {
            self.tx.complete(id);
            for entry in self.in_flight.borrow_mut().iter_mut() {
                if entry.is_some_and(|in_flight| in_flight.id == id) {
                    *entry = None;
                }
            }
        }
    }
}

/// The transmit seats of one interface.
pub type Esp32s31TxSeats<'id> = TxSeats<'id, TxQueueKind, TxBufferKind>;

/// The port-lifetime resources the body of one install receives: each
/// role's transmit seats and key slots, and the receive Block Ack slots
/// both roles share.
pub struct Esp32s31Resources<'id> {
    pub station_tx: Esp32s31TxSeats<'id>,
    pub access_point_tx: Esp32s31TxSeats<'id>,
    pub station_pairwise_keys: [Slot<'id, Esp32s31PairwiseKeys<Station>>; STATION_PAIRWISE_KEYS],
    pub station_group_keys: [Slot<'id, Esp32s31GroupKeys<Station>>; STATION_GROUP_KEYS],
    pub access_point_pairwise_keys:
        [Slot<'id, Esp32s31PairwiseKeys<AccessPoint>>; ACCESS_POINT_PAIRWISE_KEYS],
    pub access_point_group_keys:
        [Slot<'id, Esp32s31GroupKeys<AccessPoint>>; ACCESS_POINT_GROUP_KEYS],
    pub block_acks: [Slot<'id, Esp32s31BlockAcks>; RX_BLOCK_ACK_SLOTS],
}

/// The ESP32-S31 backend of one install; only
/// [`run_port`](oer_ieee80211_port::run_port) holds it.
pub struct Esp32s31Backend<
    'r,
    'slot,
    M: RawMutex,
    P,
    E,
    T,
    H,
    R,
    const BUFFER_SIZE: usize,
    const TX_BUFFERS: usize,
    const EVENTS: usize,
    U: LowerMacRxUnit,
    O,
> {
    runtime: &'r Esp32s31LowerMac<'slot, M, P, E, T, H, R, BUFFER_SIZE, TX_BUFFERS, EVENTS, U, O>,
}

/// The backend's disabled-only settings.
pub struct Esp32s31Setup<'a, L> {
    runtime: &'a L,
}

/// The backend's phase control.
pub struct Esp32s31Control<'a, L> {
    runtime: &'a L,
}

/// The backend's closed transmit gate; dropping it opens the gate and
/// publishes the attempts it held.
pub struct Esp32s31Gated<'g, L: GateOwner> {
    runtime: &'g L,
}

/// What a closed gate opens again through.
pub trait GateOwner {
    #[doc(hidden)]
    fn set_gate(&self, open: bool) -> bool;
}

impl<L: GateOwner> TxGated for Esp32s31Gated<'_, L> {}

impl<L: GateOwner> Drop for Esp32s31Gated<'_, L> {
    fn drop(&mut self) {
        self.runtime.set_gate(true);
    }
}

/// The backend's event consumer: the driver gave each attempt's permits
/// back before the consumer hands its completion out.
pub struct Esp32s31Events<'a, 'id, L, const BUFFER_SIZE: usize, const TX_BUFFERS: usize> {
    runtime: &'a L,
    held: &'a Esp32s31Held<'id, BUFFER_SIZE, TX_BUFFERS>,
}

/// The backend's client of the interface of role `Role`.
pub struct Esp32s31Client<'a, 'id, L, Role, const BUFFER_SIZE: usize, const TX_BUFFERS: usize> {
    runtime: &'a L,
    held: &'a Esp32s31Held<'id, BUFFER_SIZE, TX_BUFFERS>,
    role: PhantomData<fn() -> Role>,
}

/// The backend's clients: one per interface role.
pub struct Esp32s31Clients<'a, 'id, L, const BUFFER_SIZE: usize, const TX_BUFFERS: usize> {
    pub station: Esp32s31Client<'a, 'id, L, Station, BUFFER_SIZE, TX_BUFFERS>,
    pub access_point: Esp32s31Client<'a, 'id, L, AccessPoint, BUFFER_SIZE, TX_BUFFERS>,
}

/// The token of one admitted attempt.
pub struct Esp32s31Token(TxId);

impl RadioWork for Esp32s31Token {
    type Id = TxId;

    fn id(&self) -> TxId {
        self.0
    }
}

/// One frame's buffer, bound to its buffer permit: the header the request
/// wrote, then the body the caller writes.
pub struct Esp32s31PhaseBuffer<
    'c,
    'id,
    'slot,
    L: SlotOwner<'slot, BUFFER_SIZE>,
    const BUFFER_SIZE: usize,
> {
    buffer: Option<Esp32s31TxBuffer<'slot, BUFFER_SIZE>>,
    header: usize,
    no_ack: bool,
    rate: TxPhyRate,
    /// `None` once a submission moved it in with the attempt.
    permit: Option<BufferPermit<'c, 'id>>,
    runtime: &'c L,
}

/// What lends and takes back the core's transmit slots.
pub trait SlotOwner<'slot, const BUFFER_SIZE: usize> {
    #[doc(hidden)]
    fn release_slot(&self, buffer: Esp32s31TxBuffer<'slot, BUFFER_SIZE>);
}

impl<'slot, L: SlotOwner<'slot, BUFFER_SIZE>, const BUFFER_SIZE: usize> TxFrameBody
    for Esp32s31PhaseBuffer<'_, '_, 'slot, L, BUFFER_SIZE>
{
    fn body_mut(&mut self) -> &mut [u8] {
        let header = self.header;
        &mut self
            .buffer
            .as_mut()
            .expect("a lent buffer holds its slot")
            .frame_mut()[header..]
    }
}

impl<'slot, L: SlotOwner<'slot, BUFFER_SIZE>, const BUFFER_SIZE: usize> Drop
    for Esp32s31PhaseBuffer<'_, '_, 'slot, L, BUFFER_SIZE>
{
    fn drop(&mut self) {
        // A buffer dropped before its submission gives its slot back; its
        // permit goes back with it.
        if let Some(buffer) = self.buffer.take() {
            self.runtime.release_slot(buffer);
        }
    }
}

/// An installed pairwise key of a client of role `Role`; it holds its slot.
pub struct Esp32s31PairwiseKey<'id, Role> {
    handle: Option<KeyHandle>,
    slot: Slot<'id, Esp32s31PairwiseKeys<Role>>,
}

/// An installed group key of a client of role `Role`; it holds its slot.
pub struct Esp32s31GroupKey<'id, Role> {
    handle: Option<KeyHandle>,
    slot: Slot<'id, Esp32s31GroupKeys<Role>>,
}

/// The installed runtime type of one backend.
type Runtime<'slot, M, P, E, T, H, R, const B: usize, const N: usize, const EV: usize, U, O> =
    Esp32s31LowerMac<'slot, M, P, E, T, H, R, B, N, EV, U, O>;

impl<
    'slot,
    M: RawMutex,
    P,
    E,
    T,
    H,
    R,
    const B: usize,
    const N: usize,
    const EV: usize,
    U: LowerMacRxUnit,
    O: TxBody,
> SlotOwner<'slot, B> for Runtime<'slot, M, P, E, T, H, R, B, N, EV, U, O>
where
    P: WifiTxPowerProfile,
    E: WifiTxEntropy,
    T: oer_time::Timer + ReceptionClock,
    H: LowerMacHardware,
    R: LowerMacRetune<H>,
{
    fn release_slot(&self, buffer: Esp32s31TxBuffer<'slot, B>) {
        let _ = self.with_installed(|core, _, _, _| {
            core.release_tx_buffer(buffer);
            Ok(())
        });
    }
}

impl<
    'slot,
    M: RawMutex,
    P,
    E,
    T,
    H,
    R,
    const B: usize,
    const N: usize,
    const EV: usize,
    U: LowerMacRxUnit,
    O: TxBody,
> GateOwner for Runtime<'slot, M, P, E, T, H, R, B, N, EV, U, O>
where
    P: WifiTxPowerProfile,
    E: WifiTxEntropy,
    T: oer_time::Timer + ReceptionClock,
    H: LowerMacHardware,
    R: LowerMacRetune<H>,
{
    /// Open or close the core's transmit gate; `false` when the core keeps
    /// it open because an attempt is published.
    fn set_gate(&self, open: bool) -> bool {
        let set = self.with_installed(|core, hardware, _, _| {
            Ok(core
                .apply(hardware, LowerMacSetting::TxGate { open })
                .map(|setting| setting.is_ok())
                .unwrap_or(false))
        });
        // Opening the gate publishes a held attempt: its watchdog starts.
        self.wake.signal(());
        matches!(set, Ok(Some(true)))
    }
}

impl<
    'slot,
    M: RawMutex,
    P,
    E,
    T,
    H,
    R,
    const B: usize,
    const N: usize,
    const EV: usize,
    U: LowerMacRxUnit,
    O: TxBody,
> LowerMacSetup for Esp32s31Setup<'_, Runtime<'slot, M, P, E, T, H, R, B, N, EV, U, O>>
where
    P: WifiTxPowerProfile,
    E: WifiTxEntropy,
    T: oer_time::Timer + ReceptionClock,
    H: LowerMacHardware,
    R: LowerMacRetune<H>,
{
    type Caps = Esp32s31Caps<B>;

    fn set_channel(&mut self, channel: SupportedChannel<Esp32s31Caps<B>>) {
        // A supported channel on a disabled core is always taken: the next
        // `Enable` retunes.
        let _ = self.runtime.with_installed(|core, _, _, _| {
            let _ = core.set_channel(channel.channel());
            Ok(())
        });
    }
}

impl<
    'slot,
    M: RawMutex,
    P,
    E,
    T,
    H,
    R,
    const B: usize,
    const N: usize,
    const EV: usize,
    U: LowerMacRxUnit,
    O: TxBody,
> LowerMacControl for Esp32s31Control<'_, Runtime<'slot, M, P, E, T, H, R, B, N, EV, U, O>>
where
    P: WifiTxPowerProfile,
    E: WifiTxEntropy,
    T: oer_time::Timer + ReceptionClock,
    H: LowerMacHardware,
    R: LowerMacRetune<H>,
{
    type Caps = Esp32s31Caps<B>;
    type Gated<'g>
        = Esp32s31Gated<'g, Runtime<'slot, M, P, E, T, H, R, B, N, EV, U, O>>
    where
        Self: 'g;

    /// The core closes its gate once no attempt is published: until then
    /// the future waits for the published ones to end.
    fn close_gate(&mut self) -> impl Future<Output = Self::Gated<'_>> {
        let runtime = self.runtime;
        poll_fn(move |context| {
            if runtime.set_gate(false) {
                Poll::Ready(Esp32s31Gated { runtime })
            } else {
                runtime.wait_settled(context.waker());
                Poll::Pending
            }
        })
    }

    fn set_edca(&mut self, parameters: SupportedEdca<Esp32s31Caps<B>>) {
        let _ = self.runtime.with_installed(|core, _, _, _| {
            core.set_checked_edca(parameters.get());
            Ok(())
        });
    }

    fn set_rx_beacon_priority(&mut self, priority: RxBeaconPriority) {
        let _ = self.runtime.with_installed(|core, hardware, _, _| {
            let _ = core.apply(hardware, LowerMacSetting::RxBeaconPriority(priority));
            Ok(())
        });
    }
}

impl<
    'a,
    'id,
    'slot,
    M: RawMutex,
    P,
    E,
    T,
    H,
    R,
    const B: usize,
    const N: usize,
    const EV: usize,
    U: LowerMacRxUnit,
    O: TxBody,
> RadioPortEvents
    for Esp32s31Events<'a, 'id, Runtime<'slot, M, P, E, T, H, R, B, N, EV, U, O>, B, N>
where
    P: WifiTxPowerProfile,
    E: WifiTxEntropy,
    T: oer_time::Timer + ReceptionClock,
    H: LowerMacHardware,
    R: LowerMacRetune<H>,
{
    type Event = Esp32s31LowerMacEvent<U, O>;
    type Fault = LowerMacFault;

    async fn next_event(
        &mut self,
    ) -> PortResult<Esp32s31LowerMacEvent<U, O>, EventsLost, LowerMacFault> {
        let runtime = self.runtime;
        loop {
            // The permits of every ended attempt are back before its
            // completion is handed out.
            self.held.complete(runtime.take_ended());
            if let Some(event) = runtime.queues.take() {
                return Ok(event.map(|event| match event {
                    QueuedEvent::Received { frame, meta } => {
                        Esp32s31LowerMacEvent::Received { frame, meta }
                    }
                    QueuedEvent::TxCompleted(completion) => Esp32s31LowerMacEvent::TxCompleted(
                        completion,
                        runtime.unhold_body(completion.id),
                    ),
                    QueuedEvent::Tbtt(event) => Esp32s31LowerMacEvent::Tbtt(event),
                }));
            }
            if let Some(poisoned) = runtime.poisoned() {
                return Err(poisoned);
            }
            runtime.queues.wait().await;
        }
    }
}

impl<
    'a,
    'id,
    'slot,
    M: RawMutex,
    P,
    E,
    T,
    H,
    R,
    const B: usize,
    const N: usize,
    const EV: usize,
    U: LowerMacRxUnit,
    O: TxBody,
> LowerMacEvents for Esp32s31Events<'a, 'id, Runtime<'slot, M, P, E, T, H, R, B, N, EV, U, O>, B, N>
where
    P: WifiTxPowerProfile,
    E: WifiTxEntropy,
    T: oer_time::Timer + ReceptionClock,
    H: LowerMacHardware,
    R: LowerMacRetune<H>,
{
    type RxBuffer = Esp32s31RxBuffer<U>;
    type TxBodies = Option<O>;

    fn view(event: &Esp32s31LowerMacEvent<U, O>) -> LowerMacEvent<'_> {
        match event {
            Esp32s31LowerMacEvent::Received { frame, meta } => LowerMacEvent::Received {
                frame: frame.bytes(),
                meta: *meta,
            },
            Esp32s31LowerMacEvent::TxCompleted(completion, _) => {
                LowerMacEvent::TxCompleted(*completion)
            }
            Esp32s31LowerMacEvent::Tbtt(event) => LowerMacEvent::Extension {
                kind: oer_ieee80211_lower_mac::ExtensionKind::Tbtt,
                vif: event.tbtt.vif,
            },
        }
    }

    fn into_received(
        event: Esp32s31LowerMacEvent<U, O>,
    ) -> Result<(Esp32s31RxBuffer<U>, RxMeta), Esp32s31LowerMacEvent<U, O>> {
        match event {
            Esp32s31LowerMacEvent::Received { frame, meta } => Ok((frame, meta)),
            event => Err(event),
        }
    }

    fn into_completed(
        event: Esp32s31LowerMacEvent<U, O>,
    ) -> Result<(TxCompletion, Option<O>), Esp32s31LowerMacEvent<U, O>> {
        match event {
            Esp32s31LowerMacEvent::TxCompleted(completion, body) => Ok((completion, body)),
            event => Err(event),
        }
    }
}

impl<
    'a,
    'id,
    'slot,
    M: RawMutex,
    P,
    E,
    T,
    H,
    R,
    Role,
    const B: usize,
    const N: usize,
    const EV: usize,
    U: LowerMacRxUnit,
    O: TxBody,
> RadioPortClient
    for Esp32s31Client<'a, 'id, Runtime<'slot, M, P, E, T, H, R, B, N, EV, U, O>, Role, B, N>
where
    P: WifiTxPowerProfile,
    E: WifiTxEntropy,
    T: oer_time::Timer + ReceptionClock,
    H: LowerMacHardware,
    R: LowerMacRetune<H>,
{
    type Token = Esp32s31Token;
    type Domain = Ieee80211Radio;
    type Fault = LowerMacFault;

    fn now(
        &self,
    ) -> impl Future<Output = PortResult<Ieee80211Instant, ClockError, LowerMacFault>> + '_ {
        ready(self.runtime.now())
    }

    fn end(
        &self,
        token: Esp32s31Token,
    ) -> impl Future<Output = PortResult<(), CancelError, LowerMacFault>> + '_ {
        ready(
            self.runtime
                .with_phase(|core, _, _, sink| Ok(core.cancel(token.0, sink))),
        )
    }
}

/// The role a client's type names, with what its keys install as.
pub trait Esp32s31Role: InterfaceRole {
    #[doc(hidden)]
    fn pairwise_scope(peer: IndividualAddress, slot: u8) -> CheckedKeyScope;
    #[doc(hidden)]
    fn group_scope(key_id: GroupKeyId) -> CheckedKeyScope;
}

impl Esp32s31Role for Station {
    fn pairwise_scope(peer: IndividualAddress, _: u8) -> CheckedKeyScope {
        CheckedKeyScope::StationPairwise { peer: peer.get() }
    }

    fn group_scope(key_id: GroupKeyId) -> CheckedKeyScope {
        CheckedKeyScope::StationGroup {
            key_id: key_id.get(),
        }
    }
}

impl Esp32s31Role for AccessPoint {
    /// An access point's pairwise slot is its association identifier
    /// ([owner decision on ](https://github.com/the predecessor repository/issues/581#issuecomment-)):
    /// the slot's index plus one.
    fn pairwise_scope(peer: IndividualAddress, slot: u8) -> CheckedKeyScope {
        CheckedKeyScope::AccessPointPairwise {
            peer: peer.get(),
            association_id: u16::from(slot) + 1,
        }
    }

    fn group_scope(key_id: GroupKeyId) -> CheckedKeyScope {
        CheckedKeyScope::AccessPointGroup {
            key_id: key_id.get(),
        }
    }
}

impl<
    'a,
    'id,
    'slot,
    M: RawMutex,
    P,
    E,
    T,
    H,
    R,
    Role: Esp32s31Role,
    const B: usize,
    const N: usize,
    const EV: usize,
    U: LowerMacRxUnit,
    O: TxBody,
> Esp32s31Client<'a, 'id, Runtime<'slot, M, P, E, T, H, R, B, N, EV, U, O>, Role, B, N>
where
    P: WifiTxPowerProfile,
    E: WifiTxEntropy,
    T: oer_time::Timer + ReceptionClock,
    H: LowerMacHardware,
    R: LowerMacRetune<H>,
{
    /// Ready once no attempt in flight matches `uses`: an attempt the
    /// closed gate holds is cancelled, a published one is waited for.
    async fn settle(&self, uses: impl Fn(&InFlight) -> bool) {
        poll_fn(|context| {
            let runtime = self.runtime;
            self.held.complete(runtime.take_ended());
            let in_flight = *self.held.in_flight.borrow();
            let held: [Option<TxId>; TX_QUEUES] =
                in_flight.map(|entry| entry.filter(&uses).map(|in_flight| in_flight.id));
            if held.iter().all(Option::is_none) {
                return Poll::Ready(());
            }
            for id in held.into_iter().flatten() {
                // An unpublished attempt ends at once; a published one ends
                // on the air.
                let _ = runtime.with_phase(|core, _, _, sink| Ok(core.cancel(id, sink)));
            }
            runtime.wait_settled(context.waker());
            Poll::Pending
        })
        .await;
    }

    /// Remove an installed key once no attempt in flight uses it.
    async fn remove_key(&self, handle: Option<KeyHandle>) {
        let Some(handle) = handle else {
            return;
        };
        self.settle(|in_flight| in_flight.key == Some(handle)).await;
        let _ = self.runtime.with_phase(|core, hardware, _, _| {
            core.remove_checked_key(hardware, handle);
            Ok(())
        });
    }
}

impl<
    'a,
    'id,
    'slot,
    M: RawMutex,
    P,
    E,
    T,
    H,
    R,
    Role: Esp32s31Role,
    const B: usize,
    const N: usize,
    const EV: usize,
    U: LowerMacRxUnit,
    O: TxBody,
> LowerMacClient<'id>
    for Esp32s31Client<'a, 'id, Runtime<'slot, M, P, E, T, H, R, B, N, EV, U, O>, Role, B, N>
where
    P: WifiTxPowerProfile,
    E: WifiTxEntropy,
    T: oer_time::Timer + ReceptionClock,
    H: LowerMacHardware,
    R: LowerMacRetune<H>,
{
    type Caps = Esp32s31Caps<B>;
    type QueueKind = TxQueueKind;
    type BufferKind = TxBufferKind;
    type Queue<'c>
        = QueuePermit<'c, 'id>
    where
        Self: 'c,
        'id: 'c;
    type TxBuffer<'c>
        = Esp32s31PhaseBuffer<'c, 'id, 'slot, Runtime<'slot, M, P, E, T, H, R, B, N, EV, U, O>, B>
    where
        Self: 'c,
        'id: 'c;
    type TxBody = O;
    type PairwiseKeyKind = Esp32s31PairwiseKeys<Role>;
    type GroupKeyKind = Esp32s31GroupKeys<Role>;
    type PairwiseKey = Esp32s31PairwiseKey<'id, Role>;
    type GroupKey = Esp32s31GroupKey<'id, Role>;
    type BlockAckKind = Esp32s31BlockAcks;

    fn channel(&self) -> oer_ieee80211_lower_mac::Channel {
        self.runtime
            .with_phase(|core, _, _, _| Ok(core.channel()))
            .unwrap_or_else(|_| panic!("a poisoned phase reads no channel"))
    }

    fn queue<'c, 's>(
        &'c self,
        seat: &'s mut Seat<'id, TxQueueKind>,
    ) -> impl Future<Output = QueuePermit<'c, 'id>> + 's
    where
        'id: 'c,
        'c: 's,
    {
        self.held.tx.queue(seat)
    }

    async fn tx_buffer<'c, 's>(
        &'c self,
        frame: TxFrame<'c, Esp32s31Caps<B>>,
        seat: &'s mut Seat<'id, TxBufferKind>,
    ) -> Self::TxBuffer<'c>
    where
        'id: 'c,
        'c: 's,
    {
        let permit = self.held.tx.buffer(seat).await;
        let runtime = self.runtime;
        let len = match &frame {
            TxFrame::Individual { frame, .. } => frame.len().get(),
            TxFrame::IndividualNoAck { frame, .. } => frame.len().get(),
            TxFrame::Group { frame, .. } => frame.len().get(),
            TxFrame::BlockAckRequest { .. } => BLOCK_ACK_REQUEST_LEN,
        };
        // A buffer permit stands for one of the core's slots.
        let mut buffer = runtime
            .with_phase(|core, _, _, _| Ok(core.tx_buffer(len)))
            .ok()
            .flatten()
            .expect("a buffer permit stands for a free slot");
        let bytes = buffer.frame_mut();
        let (header, no_ack, rate) = match frame {
            TxFrame::Individual { frame, rate } => (
                frame.header().header().encode(bytes, false),
                false,
                rate.get(),
            ),
            TxFrame::IndividualNoAck { frame, rate } => (
                frame.header().header().encode(bytes, true),
                true,
                rate.rate().get(),
            ),
            // A group frame solicits no response: its QoS Ack Policy is No
            // Ack, and the core sends it without one.
            TxFrame::Group { frame, rate } => {
                (frame.header().encode(bytes, true), false, rate.get())
            }
            TxFrame::BlockAckRequest {
                receiver,
                transmitter,
                tid,
                starting_sequence,
                rate,
            } => {
                let request = oer_ieee80211_mac::block_ack::encode_block_ack_request(
                    receiver.get(),
                    transmitter,
                    tid.get(),
                    starting_sequence,
                );
                bytes[..request.len()].copy_from_slice(&request);
                (request.len(), false, rate.rate().get())
            }
        };
        Esp32s31PhaseBuffer {
            buffer: Some(buffer),
            header,
            no_ack,
            // A phase rate is one the capabilities carry on the channel.
            rate: TxPhyRate::try_from(rate).unwrap_or_else(|_| panic!("a phase rate converts")),
            permit: Some(permit),
            runtime,
        }
    }

    fn submit<'p>(
        &'p self,
        attempt: PhaseAttempt<'p, 'id, Self>,
    ) -> Result<Work<'p, Self>, Poisoned<LowerMacFault>>
    where
        'id: 'p,
    {
        let runtime = self.runtime;
        if let Some(poisoned) = runtime.poisoned() {
            return Err(poisoned);
        }
        let PhaseAttempt {
            queue,
            mut frame,
            body,
            protection,
            key,
            power,
            backoff,
            coex,
        } = attempt;
        let key = key.and_then(|key| match key {
            AttemptKey::Pairwise(key) => key.handle,
            AttemptKey::Group(key) => key.handle,
        });
        let id = runtime.next_id();
        let mut buffer = frame.buffer.take().expect("a lent buffer holds its slot");
        if let Some(body) = &body {
            // The body ends the MPDU: its octets follow what the caller
            // wrote.
            let len = buffer.len();
            let bytes = body.bytes();
            buffer.frame_mut()[len - bytes.len()..].copy_from_slice(bytes);
        }
        if let Some(body) = body {
            runtime.hold_body(id, body);
        }
        let values = MpduValues {
            id,
            vif: vif_of(Role::ROLE),
            access_category: oer_ieee80211_lower_mac::QueueGrant::access_category(&queue),
            frame: buffer,
            no_ack: frame.no_ack,
            rate: frame.rate,
            protection,
            key,
            power_ceiling_dbm: match power.get() {
                TxPower::Calibrated => None,
                TxPower::MaxDbm(dbm) => Some(dbm),
            },
            backoff_slots: match backoff.get() {
                Backoff::Slots(slots) => slots,
                Backoff::HardwareDraw { .. } => {
                    unreachable!("the ESP32-S31 supports caller-drawn backoff only")
                }
            },
            coex: coex.get(),
        };
        runtime.with_phase(|core, hardware, _, _| core.submit_checked(hardware, values))?;
        // A publication starts its watchdog.
        runtime.wake.signal(());
        let in_flight = InFlight {
            id,
            role: Role::ROLE,
            key,
        };
        if let Some(free) = self
            .held
            .in_flight
            .borrow_mut()
            .iter_mut()
            .find(|entry| entry.is_none())
        {
            *free = Some(in_flight);
        }
        // The permit goes in with the attempt; the buffer, its slot taken,
        // drops without giving it back.
        let permit = frame.permit.take().expect("a lent buffer holds its permit");
        drop(frame);
        self.held.tx.admit(id, queue, permit);
        Ok(Work::new(self, Esp32s31Token(id)))
    }

    fn install_pairwise_key(
        &self,
        slot: Slot<'id, Esp32s31PairwiseKeys<Role>>,
        peer: IndividualAddress,
        key: Ccmp128Key,
    ) -> Esp32s31PairwiseKey<'id, Role> {
        let scope = Role::pairwise_scope(peer, slot.index());
        let handle = self
            .runtime
            .with_phase(|core, hardware, _, _| {
                core.install_checked_key(hardware, vif_of(Role::ROLE), scope, &key.0)
            })
            .ok();
        Esp32s31PairwiseKey { handle, slot }
    }

    fn install_group_key(
        &self,
        slot: Slot<'id, Esp32s31GroupKeys<Role>>,
        key_id: GroupKeyId,
        key: Ccmp128Key,
    ) -> Esp32s31GroupKey<'id, Role> {
        let scope = Role::group_scope(key_id);
        let handle = self
            .runtime
            .with_phase(|core, hardware, _, _| {
                core.install_checked_key(hardware, vif_of(Role::ROLE), scope, &key.0)
            })
            .ok();
        Esp32s31GroupKey { handle, slot }
    }

    async fn remove_pairwise_key(
        &self,
        key: Esp32s31PairwiseKey<'id, Role>,
    ) -> Slot<'id, Esp32s31PairwiseKeys<Role>> {
        self.remove_key(key.handle).await;
        key.slot
    }

    async fn remove_group_key(
        &self,
        key: Esp32s31GroupKey<'id, Role>,
    ) -> Slot<'id, Esp32s31GroupKeys<Role>> {
        self.remove_key(key.handle).await;
        key.slot
    }

    fn add_block_ack(
        &self,
        slot: Slot<'id, Esp32s31BlockAcks>,
        agreement: SupportedBlockAck<Esp32s31Caps<B>>,
    ) -> Option<Slot<'id, Esp32s31BlockAcks>> {
        let (peer, tid, start_sequence, window) = agreement.get();
        // The core replaces an agreement of the same pair in its bank.
        let _ = self.runtime.with_phase(|core, hardware, _, _| {
            core.start_checked_rx_block_ack(
                hardware,
                RxBlockAckAgreement {
                    vif: vif_of(Role::ROLE),
                    peer: peer.get(),
                    tid: tid.get(),
                    start_sequence,
                    window,
                },
            )
        });
        let mut agreements = self.held.agreements.borrow_mut();
        let replaced = agreements
            .iter_mut()
            .find(|entry| {
                entry.as_ref().is_some_and(|agreement| {
                    (agreement.role, agreement.peer, agreement.tid) == (Role::ROLE, peer, tid)
                })
            })
            .and_then(Option::take)
            .map(|agreement| agreement.slot);
        let free = agreements
            .iter_mut()
            .find(|entry| entry.is_none())
            .expect("an agreement slot stands for a free entry");
        *free = Some(Agreement {
            role: Role::ROLE,
            peer,
            tid,
            slot,
        });
        replaced
    }

    fn remove_block_ack(
        &self,
        peer: IndividualAddress,
        tid: Tid,
    ) -> Option<Slot<'id, Esp32s31BlockAcks>> {
        let slot = self
            .held
            .agreements
            .borrow_mut()
            .iter_mut()
            .find(|entry| {
                entry.as_ref().is_some_and(|agreement| {
                    (agreement.role, agreement.peer, agreement.tid) == (Role::ROLE, peer, tid)
                })
            })
            .and_then(Option::take)
            .map(|agreement| agreement.slot)?;
        let _ = self.runtime.with_phase(|core, hardware, _, _| {
            core.end_checked_rx_block_ack(hardware, vif_of(Role::ROLE), peer.get(), tid.get())
        });
        Some(slot)
    }
}

impl<
    'a,
    'id,
    'slot,
    M: RawMutex,
    P,
    E,
    T,
    H,
    R,
    const B: usize,
    const N: usize,
    const EV: usize,
    U: LowerMacRxUnit,
    O: TxBody,
> StationClient<'id>
    for Esp32s31Client<'a, 'id, Runtime<'slot, M, P, E, T, H, R, B, N, EV, U, O>, Station, B, N>
where
    P: WifiTxPowerProfile,
    E: WifiTxEntropy,
    T: oer_time::Timer + ReceptionClock,
    H: LowerMacHardware,
    R: LowerMacRetune<H>,
{
    async fn configure(&self, receive: StationReceive<Esp32s31Caps<B>>) {
        self.settle(|in_flight| in_flight.role == VifRole::Station)
            .await;
        let (bssid, receive) = match receive {
            StationReceive::Off => (None, ReceiveFilter::NONE),
            StationReceive::OtherBssManagement => (None, ReceiveFilter::OTHER_BSS_MANAGEMENT),
            StationReceive::Bss { bssid, filter } => (Some(bssid.get()), filter.get()),
        };
        let _ = self.runtime.with_phase(|core, hardware, _, _| {
            let address = core.station_address();
            core.configure_checked_vif(
                hardware,
                vif_of(VifRole::Station),
                VifConfig {
                    address,
                    role: VifRole::Station,
                    bssid,
                    receive,
                },
            );
            Ok(())
        });
    }

    fn set_bss_color(&self, color: BssColor) {
        let _ = self.runtime.with_phase(|core, _, _, _| {
            core.set_checked_he_bss_color(color.get());
            Ok(())
        });
    }
}

impl<
    'a,
    'id,
    'slot,
    M: RawMutex,
    P,
    E,
    T,
    H,
    R,
    const B: usize,
    const N: usize,
    const EV: usize,
    U: LowerMacRxUnit,
    O: TxBody,
> AccessPointClient<'id>
    for Esp32s31Client<'a, 'id, Runtime<'slot, M, P, E, T, H, R, B, N, EV, U, O>, AccessPoint, B, N>
where
    P: WifiTxPowerProfile,
    E: WifiTxEntropy,
    T: oer_time::Timer + ReceptionClock,
    H: LowerMacHardware,
    R: LowerMacRetune<H>,
{
    async fn configure(&self, filter: SupportedFilter<Esp32s31Caps<B>, AccessPoint>) {
        self.settle(|in_flight| in_flight.role == VifRole::AccessPoint)
            .await;
        let _ = self.runtime.with_phase(|core, hardware, address, _| {
            core.configure_checked_vif(
                hardware,
                vif_of(VifRole::AccessPoint),
                VifConfig {
                    address,
                    role: VifRole::AccessPoint,
                    bssid: Some(address),
                    receive: filter.get(),
                },
            );
            Ok(())
        });
    }
}

impl<
    'r,
    'slot,
    M: RawMutex,
    P,
    E,
    T,
    H,
    R,
    const B: usize,
    const N: usize,
    const EV: usize,
    U: LowerMacRxUnit,
    O: TxBody,
> PortBackend for Esp32s31Backend<'r, 'slot, M, P, E, T, H, R, B, N, EV, U, O>
where
    P: WifiTxPowerProfile,
    E: WifiTxEntropy,
    T: oer_time::Timer + ReceptionClock,
    H: LowerMacHardware,
    R: LowerMacRetune<H>,
{
    type Event = Esp32s31LowerMacEvent<U, O>;
    type Domain = Ieee80211Radio;
    type Fault = LowerMacFault;
    type Refusal = RetuneRefused;
    type Setup<'a, 'id>
        = Esp32s31Setup<'r, Runtime<'slot, M, P, E, T, H, R, B, N, EV, U, O>>
    where
        Self: 'a;
    type Events<'a, 'id>
        = Esp32s31Events<'a, 'id, Runtime<'slot, M, P, E, T, H, R, B, N, EV, U, O>, B, N>
    where
        Self: 'a,
        'id: 'a;
    type Control<'a, 'id>
        = Esp32s31Control<'r, Runtime<'slot, M, P, E, T, H, R, B, N, EV, U, O>>
    where
        Self: 'a,
        'id: 'a;
    type Clients<'a, 'id>
        = Esp32s31Clients<'a, 'id, Runtime<'slot, M, P, E, T, H, R, B, N, EV, U, O>, B, N>
    where
        Self: 'a,
        'id: 'a;
    type Resources<'id> = Esp32s31Resources<'id>;
    type Held<'id> = Esp32s31Held<'id, B, N>;
    type State = ();
    type Uninstall = ();
    type Uninstalled = (LowerMacCore<'slot, P, E, T, B, N>, H);

    async fn enable(&mut self) -> EnableResult<RetuneRefused, LowerMacFault> {
        let runtime = self.runtime;
        let start = runtime.with_installed(|core, _, _, sink| Ok(core.enable_checked(sink)))?;
        if let Some(LifecycleStart::Retune(channel)) = start {
            // The retune holds the registers for the switch; the driver
            // defers the interrupt causes meanwhile.
            let mut retune = runtime.retune.lock(|slot| slot.borrow_mut().take());
            let mut hardware = runtime.installed.lock(|installed| {
                installed
                    .borrow_mut()
                    .as_mut()
                    .and_then(|installed| installed.hardware.take())
            });
            let retuned = match (retune.as_mut(), hardware.as_mut()) {
                (Some(retune), Some(hardware)) => retune.retune(hardware, channel).await,
                _ => false,
            };
            runtime.retune.lock(|slot| *slot.borrow_mut() = retune);
            if let Some(hardware) = hardware {
                runtime.installed.lock(|installed| {
                    if let Some(installed) = installed.borrow_mut().as_mut() {
                        installed.hardware = Some(hardware);
                    }
                });
            }
            runtime.wake.signal(());
            runtime.with_installed(|core, _, _, sink| {
                core.finish_retune(retuned, sink);
                Ok(())
            })?;
        }
        Ok(match runtime.lifecycle_event().await? {
            LifecycleEvent::Enabled => Ok(()),
            _ => Err(RetuneRefused),
        })
    }

    async fn disable<'a, 'id>(
        &'a mut self,
        held: &'a Esp32s31Held<'id, B, N>,
        sink: &'a mut dyn FnMut(Esp32s31LowerMacEvent<U, O>),
    ) -> Result<(), Poisoned<LowerMacFault>>
    where
        'id: 'a,
    {
        let runtime = self.runtime;
        runtime.with_installed(|core, _, _, sink| {
            core.disable_checked(sink);
            Ok(())
        })?;
        // Every admitted attempt ends before `Disabled`.
        while runtime.lifecycle_event().await? != LifecycleEvent::Disabled {}
        loop {
            held.complete(runtime.take_ended());
            match runtime.queues.take() {
                Some(Ok(QueuedEvent::TxCompleted(completion))) => {
                    sink(Esp32s31LowerMacEvent::TxCompleted(
                        completion,
                        runtime.unhold_body(completion.id),
                    ))
                }
                Some(_) => {}
                None if held.tx.idle() => return Ok(()),
                None => {
                    if let Some(poisoned) = runtime.poisoned() {
                        return Err(poisoned);
                    }
                    runtime.queues.wait().await;
                }
            }
        }
    }

    fn now(
        &self,
    ) -> impl Future<Output = PortResult<Ieee80211Instant, ClockError, LowerMacFault>> + '_ {
        ready(self.runtime.now())
    }

    fn resources<'id>(
        &mut self,
        mut mint: Mint<'id>,
    ) -> (Esp32s31Held<'id, B, N>, Esp32s31Resources<'id>) {
        let (queue_line, buffer_line, [station_tx, access_point_tx]) = TxSeats::mint(&mut mint);
        let buffers = FifoPermitPool::new(mint.permits(), buffer_line);
        let held = Esp32s31Held {
            tx: TxPermits::new(&mut mint, queue_line, buffers),
            in_flight: RefCell::new([None; TX_QUEUES]),
            agreements: RefCell::new([const { None }; RX_BLOCK_ACK_SLOTS]),
        };
        let resources = Esp32s31Resources {
            station_tx,
            access_point_tx,
            station_pairwise_keys: mint.slots(),
            station_group_keys: mint.slots(),
            access_point_pairwise_keys: mint.slots(),
            access_point_group_keys: mint.slots(),
            block_acks: mint.slots(),
        };
        (held, resources)
    }

    fn setup<'a, 'id>(&'a mut self) -> Lent<'a, Self::Setup<'a, 'id>> {
        Lent::new(Esp32s31Setup {
            runtime: self.runtime,
        })
    }

    fn split<'a, 'id>(
        &'a mut self,
        held: &'a Esp32s31Held<'id, B, N>,
    ) -> PhaseParts<'a, Self::Events<'a, 'id>, Self::Control<'a, 'id>, Self::Clients<'a, 'id>>
    where
        'id: 'a,
    {
        let runtime = self.runtime;
        PhaseParts {
            events: Lent::new(Esp32s31Events { runtime, held }),
            control: Lent::new(Esp32s31Control { runtime }),
            clients: Lent::new(Esp32s31Clients {
                station: Esp32s31Client {
                    runtime,
                    held,
                    role: PhantomData,
                },
                access_point: Esp32s31Client {
                    runtime,
                    held,
                    role: PhantomData,
                },
            }),
        }
    }

    fn drive<'p, 'id>(
        &mut self,
        held: &'p Self::Held<'id>,
        retire: Retire<'p>,
    ) -> impl Future<Output = ()> + use<'p, 'id, 'r, 'slot, M, P, E, T, H, R, B, N, EV, U, O> {
        let runtime = self.runtime;
        async move {
            // The deadline serviced last, which the core may keep until an
            // interrupt edge moves it.
            let mut serviced = None;
            loop {
                if retire.is_requested() {
                    return;
                }
                held.complete(runtime.take_ended());
                if runtime.poisoned().is_some() {
                    match select(runtime.wake.wait(), retire.requested()).await {
                        embassy_futures::select::Either::First(()) => continue,
                        embassy_futures::select::Either::Second(()) => return,
                    }
                }
                runtime.service_deferred();
                let deadline = runtime
                    .installed
                    .lock(|installed| {
                        installed
                            .borrow()
                            .as_ref()
                            .map(|installed| installed.core.next_deadline())
                    })
                    .flatten()
                    .filter(|deadline| Some(*deadline) != serviced);
                let watchdog = async {
                    match deadline {
                        Some(deadline) => runtime.timer.wait_until(deadline).await,
                        None => core::future::pending().await,
                    }
                };
                match select3(watchdog, runtime.wake.wait(), retire.requested()).await {
                    Either3::First(()) => {
                        serviced = deadline;
                        let _ = runtime.with_installed(|core, hardware, _, sink| {
                            core.service(hardware, WifiTxWake::Deadline, sink)
                        });
                    }
                    // A deadline, an install, a completion or a fault; rearm.
                    Either3::Second(()) => serviced = None,
                    Either3::Third(()) => return,
                }
            }
        }
    }

    async fn retire_bound(&self) {
        let _ = self.runtime.timer.wait_for(RETIRE_BOUND).await;
    }

    fn detach<'id>(&mut self, _: &Esp32s31Held<'id, B, N>) {
        // The interrupt entries reach the runtime, which keeps no core once
        // it is uninstalled: they post nothing to this install again.
    }

    fn uninstall<'id>(
        self,
        _: Esp32s31Held<'id, B, N>,
        (): (),
        (): (),
    ) -> (LowerMacCore<'slot, P, E, T, B, N>, H) {
        self.runtime
            .uninstall()
            .expect("a disabled port's core is idle with its registers home")
    }

    fn abandon(&mut self) {
        // Close transmission at once; the poisoned runtime serves nothing
        // more until the radio is reset.
        self.runtime.set_gate(false);
        self.runtime.fault.lock(|fault| {
            if fault.get().is_none() {
                fault.set(Some(LowerMacFault::Abandoned));
            }
        });
        self.runtime.queues.changed.signal(());
        self.runtime.wake.signal(());
    }
}

impl<
    'r,
    'slot,
    M: RawMutex,
    P,
    E,
    T,
    H,
    R,
    const B: usize,
    const N: usize,
    const EV: usize,
    U: LowerMacRxUnit,
    O: TxBody,
> LowerMacBackend for Esp32s31Backend<'r, 'slot, M, P, E, T, H, R, B, N, EV, U, O>
where
    P: WifiTxPowerProfile,
    E: WifiTxEntropy,
    T: oer_time::Timer + ReceptionClock,
    H: LowerMacHardware,
    R: LowerMacRetune<H>,
{
    type Caps = Esp32s31Caps<B>;
}

impl<
    'slot,
    M: RawMutex,
    P,
    E,
    T,
    H,
    R,
    const B: usize,
    const N: usize,
    const EV: usize,
    U: LowerMacRxUnit,
    O: TxBody,
> PortRxSink<U> for Runtime<'slot, M, P, E, T, H, R, B, N, EV, U, O>
where
    P: WifiTxPowerProfile,
    E: WifiTxEntropy,
    T: oer_time::Timer + ReceptionClock,
    H: LowerMacHardware,
    R: LowerMacRetune<H>,
{
    fn received_room(&self) -> usize {
        Esp32s31LowerMac::received_room(self)
    }

    fn try_on_received(&self, unit: U) -> Result<Result<(), U>, RxError> {
        Esp32s31LowerMac::try_on_received(self, unit)
    }
}

mod rx_publisher;

#[cfg(target_arch = "riscv32")]
mod install;

#[cfg(target_arch = "riscv32")]
pub use install::{
    Esp32s31PortRetune, Esp32s31WifiPortHeld, Esp32s31WifiPortInstallError,
    Esp32s31WifiPortInstallFailure, Esp32s31WifiPortInstalled, Esp32s31WifiPortUninstallFailure,
    Esp32s31WifiPortUninstalled, install_esp32s31_wifi_port, uninstall_esp32s31_wifi_port,
};

pub use rx_publisher::{Esp32s31PortRxPublisher, PortRxSink};
