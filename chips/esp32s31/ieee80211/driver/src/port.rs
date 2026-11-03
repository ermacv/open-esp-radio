//! The S31 lower-MAC port. The driver loan executes hardware operations; the
//! body's receive pull services the RX ring and returns its units.
//!
//! The image polls the port's body in the station's task and its driver
//! registration in the driver's task, beside it on the station's executor.
//! Each borrows the device and the driver's state only within one
//! synchronous step.
//! P3 owns the interrupt epoch and posts
//! static typed signals through [`crate::irq`]; it never accesses this state.
//! All DMA allocations and the device remain in static storage through an
//! abnormal exit. Neither a dropped future nor a forgotten port frees them.

use crate::mac::{
    rx::{NormalizedRxFrame, RxError},
    tx::{LegacyTxQueue, TxBacking, TxSlot, TxSlotAccess},
};
use crate::{
    lower_mac::{self as core_mac, *},
    ordinary_tx::{OrdinaryTxIdleOwner, WifiTxEntropy, WifiTxPowerProfile},
    tx::WifiTxWake,
};
use atomic_waker::AtomicWaker;
use core::{
    cell::{Cell, RefCell},
    convert::Infallible,
    future::{Future, poll_fn},
    marker::PhantomData,
    pin::Pin,
    task::{Context, Poll},
};
use oer_portable_time::{Clock, Duration, Instant, Timer};
use oer_protocols_ieee80211_contracts_port::association;
use oer_protocols_ieee80211_contracts_port::*;
use oer_protocols_ieee80211_contracts_security::Ccmp128Key;
use oer_protocols_ieee80211_wire::{address::IndividualAddress, ccmp::GroupKeyId};
use oer_protocols_radio_port::{self as radio, Child, Root, Slot};
use oer_protocols_radio_port::{
    BodyLoan, ClockError, LoanedContext, SlotBodyOwner, SlotIdentity, SlotOwner,
};

mod atomic_words;
use atomic_words::KeyActivity;
mod buffer;
#[cfg(target_arch = "riscv32")]
pub mod device;
mod driver;
mod engines;
pub use engines::{
    Esp32s31Arena, Esp32s31Commands, Esp32s31Port, GroupInstall, InstallRefused, PairwiseInstall,
    PortInstallation, StoppedDataPort,
};
mod keys;
mod lent;
pub use lent::{LentNotifies, LentReceive, NetworkPeers, SameHart};
mod lifecycle;
pub use lifecycle::{InstallationPools, Resources};
mod registration;
pub use registration::{Registration, RegistrationError};
mod phase;
#[cfg(any(target_arch = "riscv32", test))]
mod receive;
mod settings;
use oer_portable_sync::PriorityWaker;
#[cfg(all(oer_trace, oer_debug_control))]
pub mod trace;
pub use buffer::*;
pub use keys::{GroupKey, KeyIdentity, PairwiseKey, PortKeys};
mod loan;
mod owned;
pub use crate::lower_mac::arena::BlockAckAnswer;
pub use crate::lower_mac::{ContentionDraw, Observations};
pub use driver::StationArena;
pub use phase::{Esp32s31Interface, Esp32s31Phase, Esp32s31Setup};
pub use settings::SettingFault;

/// Shared admission capacity across the four access categories. Each MPDU
/// takes one ticket; the station's two data arenas hold one each.
pub const TX_TICKETS: usize = 8;

/// Frames of one data arena: the DMA chain's slots, at most the BlockAck
/// window and report.
pub const TX_AMPDU_SUBFRAMES: usize = crate::mac::tx::ampdu::TX_AMPDU_SLOT_CAPACITY;

/// The transmit pool's buffers: three complete aggregates, one in the air,
/// the next one growing and the network's queue behind them, and three for
/// MPDUs beside them.
pub const TX_BUFFERS: usize = 3 * TX_AMPDU_SUBFRAMES + 3;

/// The data arenas' descriptor chains: the arena on the air holds one while
/// the station fills the other.
pub const AMPDU_CHAINS: usize = 2;

/// The ordinary descriptors: one per EDCA queue, which holds one published
/// MPDU at a time.
pub const TX_DESCRIPTORS: usize = 4;

const _: () = {
    // A buffer place is named by a `u8` pool index.
    if TX_BUFFERS > u8::MAX as usize + 1 {
        panic!("the transmit pool's places fit their u8 index");
    }
    if AMPDU_CHAINS == 0 || AMPDU_CHAINS * TX_AMPDU_SUBFRAMES > TX_BUFFERS {
        panic!("every chain can hold a complete aggregate from the pool");
    }
    if TX_DESCRIPTORS != crate::lower_mac::LOWER_MAC_TX_QUEUES {
        panic!("one ordinary descriptor per EDCA queue");
    }
    if TX_AMPDU_SUBFRAMES > crate::mac::tx::ampdu::TX_BLOCK_ACK_MAX_WINDOW as usize
        || TX_AMPDU_SUBFRAMES > BlockAckReport::BITMAP_BITS as usize
    {
        panic!("an arena's frames fit its BlockAck window and report");
    }
};

/// The transmit pool every MPDU and subframe is written into.
pub type TxPool<const B: usize> =
    oer_chips_esp32s31_hal::memory::dma_static::PlacedTxPool<B, 0, 0, TX_BUFFERS>;

/// One aggregate descriptor chain over the pool's buffers.
pub type AmpduChain<const B: usize> =
    crate::lower_mac::Esp32s31AmpduOwner<'static, TxBacking<B>, TX_AMPDU_SUBFRAMES>;

/// The descriptors the pool's buffers are published through: one ordinary
/// descriptor per EDCA queue and the aggregate chains. A stopped port gets
/// them back only when every one is idle.
pub struct TxDescriptors<const B: usize> {
    pub single: [Pin<&'static mut TxSlot<B>>; TX_DESCRIPTORS],
    pub chains: [AmpduChain<B>; AMPDU_CHAINS],
}

impl<const B: usize> TxDescriptors<B> {
    /// Whether no descriptor holds a buffer or a publication.
    fn idle(&self) -> bool {
        self.single
            .iter()
            .all(|slot| slot.state() == crate::mac::tx::TxDmaState::Free && !slot.attached())
            && self.chains.iter().all(|chain| chain.is_fully_free())
    }
}

/// The receive units the body holds at once besides those it lent the network:
/// its receive events and the MPDUs its receive Block Ack agreements keep,
/// one 16-MPDU reorder window, the archive's choice: a 32-MPDU window
/// left too few descriptors armed under sustained HT40 traffic. The network leaves them in the receive pool
/// ([`lent_rx`]).
pub const RX_HELD: usize = 16;

/// The receive units of a pool of `units` (the units that may be outside
/// the ring at once) the network may hold. It leaves the radio
/// [`RX_HELD`] units and the admission's critical reserve: when the body holds a
/// full reorder window and the network all it may, the pool still stages
/// the MPDU that closes the window's hole, which the hardware already
/// acknowledged, as ordinary bulk input.
pub const fn lent_rx(units: usize) -> usize {
    units.saturating_sub(RX_HELD + crate::rx::transaction::DEFAULT_CRITICAL_RESERVE)
}

/// The device events one driver turn serves before it polls again: a ring's
/// worth of received units, so a burst costs one turn, not one per unit.
const EVENTS_PER_POLL: usize = 32;

// A blocked RX handoff needs the body to return a lease. Its retry timer must
// not keep the driver polling an already reached deadline while that consumer
// cannot run.
pub(super) fn suspend_rx_continuation(receive_room: usize, continuation: &mut Option<Instant>) {
    if receive_room == 0 {
        *continuation = None;
    }
}

const INTERFACE_COMMANDS: usize = InterfaceCommandPool::<Station>::COMMAND_CAPACITY
    + InterfaceCommandPool::<AccessPoint>::COMMAND_CAPACITY;
const INTERFACE_TERMINALS: usize = InterfaceCommandPool::<Station>::TERMINAL_CAPACITY
    + InterfaceCommandPool::<AccessPoint>::TERMINAL_CAPACITY;
// A PTK slot has one installation or removal terminal outstanding: its
// removal needs the key that only its installation terminal returns.
pub const TERMINAL_CAPACITY: usize = TX_TICKETS + 16 + 2 + 1 + INTERFACE_TERMINALS;
// For the same reason, one installation or removal command per PTK slot.
const COMMANDS: usize = TX_TICKETS + 16 + 2 + 2 + INTERFACE_COMMANDS;
pub const PUBLICATION_BOUND: Duration = Duration::from_millis(250);
pub const COMMAND_BOUND: Duration = Duration::from_millis(400);
/// Response/retirement containment budget. It covers one driver cleanup step
/// and ready driver work within the existing command bound; a later answer
/// fails closed.
pub const SHUTDOWN_BOUND: Duration = COMMAND_BOUND;
/// Fresh-clock response budget, independent of the command FIFO. It covers
/// one driver reading and its ready peer tasks; expiry is transient Unavailable.
const CLOCK_BOUND: Duration = COMMAND_BOUND;
type Part<Id, const I: u8> = Child<Root<Id>, I>;
type CorePath<Id: SlotIdentity> = Part<Id, 0>;
type Core<'id, Id: SlotIdentity, D, const B: usize> = LowerMacCore<
    'id,
    'static,
    <D as Device<B>>::Power,
    <D as Device<B>>::Entropy,
    <D as Device<B>>::Timer,
    <D as Device<B>>::Coex,
    B,
    CorePath<Id>,
    TX_TICKETS,
>;

/// An owned RX allocation from the cooperative receive owner.
pub trait ReceiveUnit: 'static {
    fn normalized(&self) -> Result<NormalizedRxFrame<'_>, RxError>;
    /// The unit's view: the received bytes, or the MPDU once narrowed.
    fn bytes(&self) -> &[u8];
    /// Narrow the view to `length` bytes from `offset` in it; a range
    /// outside it is refused and changes nothing. Normalization reads the
    /// whole received unit, so it runs before.
    fn narrow(
        &mut self,
        offset: usize,
        length: usize,
    ) -> Result<(), oer_portable_memory::FrameRangeError>;
    /// [`Self::bytes`], then the rest of the unit's allocation; every byte
    /// is initialized.
    fn storage(&self) -> &[u8];
    /// [`Self::storage`], mutably, for a network owner that rewrites it.
    fn storage_mut(&mut self) -> &mut [u8];
}

/// One bounded hardware step of the driver's task. Implementations retain all progress in
/// themselves across `Pending`; no ISR may call any of these operations.
/// The device owns the cooperative registers, RX ring and PHY membership.
pub trait Device<const B: usize>: Sized + 'static {
    /// Image-selected concrete coexistence policy, statically dispatched in
    /// the driver's task.
    type Coex: RadioCoexPriorities + 'static;
    type Power: WifiTxPowerProfile;
    type Entropy: WifiTxEntropy;
    type Timer: Timer + Clone + 'static;
    type Hardware: LowerMacHardware;
    type Receive: ReceiveUnit + Send;
    /// How a lent receive unit's return tells the driver and the task
    /// waiting for every return: [`SameHart`], or the peers' signals of a
    /// network on another hart ([`NetworkPeers`]).
    type Lent: LentNotifies;
    type Body: TxBody + 'static;
    /// The receive units the network may hold at once. The device's other
    /// units stay with the radio, for its receive events and their
    /// reordering.
    const LENT_RX: usize;
    type OwnedContext<Id: SlotIdentity>;
    type Context<'p, Id: SlotIdentity + 'p>: LoanedContext<'p, Id, Owned = Self::OwnedContext<Id>>
        + 'p;
    type OwnedConfigureContext<Id: SlotIdentity>;
    type ConfigureContext<'p, Id: SlotIdentity + 'p>: LoanedContext<'p, Id, Owned = Self::OwnedConfigureContext<Id>>
        + 'p;
    /// The device's registers, while no actor transaction or retirement
    /// holds them; otherwise `Fault::HardwareUnavailable`.
    fn hardware(&mut self) -> Result<&mut Self::Hardware, Fault>;
    fn timer(&self) -> &Self::Timer;
    /// The device's next deadline for the driver's turn. A due receive
    /// continuation is not one: the body's pull takes it, and the driver's
    /// turn only wakes the body ([`Self::receive_due`]).
    fn next_deadline(&self) -> Option<Instant>;
    fn take_power_profile(&mut self) -> Option<Self::Power>;
    fn radio_now(&mut self) -> Result<Ieee80211Instant, ClockError>;
    fn poll_start(
        &mut self,
        cx: &mut Context<'_>,
        channel: oer_protocols_ieee80211_wire::channel::WifiChannel,
    ) -> Poll<Result<(), Fault>>;
    fn poll_stop(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Fault>>;
    /// The driver's turn: take the interrupt facts but the receive ones,
    /// which wake the body ([`Self::poll_received_facts`]), and return a
    /// transmit or TBTT event. This never services the ring.
    fn poll_event(&mut self, cx: &mut Context<'_>) -> Poll<Result<DeviceEvent, Fault>>;
    /// Whether the body's pull has a staged unit to take or ring work to
    /// service now. The driver wakes the body when it has.
    fn receive_due(&mut self) -> bool;
    /// The body's turn: take the receive interrupt facts, registering the
    /// body alone on their signal, and report [`Self::receive_due`]. While a
    /// cut or a stop holds the ring the driver takes them; this returns
    /// `false` without polling.
    fn poll_received_facts(&mut self, cx: &mut Context<'_>) -> Result<bool, Fault>;
    /// The body's pull, in its own turn: a staged unit, or the ring
    /// serviced once when its interrupt left work and the body has
    /// `receive_room`. It never polls the interrupt facts, which stay the
    /// driver's; `cx` is the body's, woken when the ring has more at once.
    /// `None` while a cut or a stop holds the ring.
    fn pull_received(
        &mut self,
        receive_room: usize,
        cx: &mut Context<'_>,
    ) -> Result<Option<Self::Receive>, Fault>;
    /// A finite RX cut: mask/drain preceding DMA and ISR facts before a GTK
    /// mutation. Returned RX must be delivered before its command terminal.
    fn poll_rx_cut(
        &mut self,
        receive_room: usize,
        cx: &mut Context<'_>,
    ) -> Poll<Result<Option<Self::Receive>, Fault>>;
    fn finish_rx_cut(&mut self) -> Result<(), Fault>;
    /// The data port's receive facts: take them, registering `cx` alone
    /// on their signal, and report whether the ring holds work (a staged
    /// unit, or ring work, however long its continuation waits). It
    /// reads no clock; while a cut or a stop holds the ring it is `false`.
    fn poll_rx_work(&mut self, cx: &mut Context<'_>) -> Result<bool, Fault>;
    /// The data port's receive pull at the turn's `now`: a staged unit, or
    /// the ring serviced once, while the caller has `receive_room`. It
    /// never wakes: ring work left now is [`DevicePull::Again`], work
    /// waiting for its continuation [`DevicePull::Later`].
    fn pull_unit(
        &mut self,
        receive_room: usize,
        now: Instant,
    ) -> Result<DevicePull<Self::Receive>, Fault>;
    fn recycle(&mut self, frame: Self::Receive);
    /// The network dropped a lent unit: rearm what the ring got back.
    fn returned(&mut self);
    /// Mask requests and erase keys synchronously. A successful stop must
    /// include both DMA and entered IRQ operations; otherwise return false.
    fn abandon(&mut self) -> bool;
    fn detach(&mut self);
}

pub enum DeviceEvent {
    Tx(WifiTxWake),
    Tbtt,
}

/// What the data port's receive pull found ([`Device::pull_unit`]).
pub enum DevicePull<R> {
    /// One received unit.
    Unit(R),
    /// Nothing now: the ring's next unit raises the receive signal.
    Empty,
    /// The ring has work left now; pull again within the turn's budget.
    Again,
    /// The ring's work resumes at this instant.
    Later(Instant),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Fault {
    Core(LowerMacFault),
    Receive(crate::rx::control::WifiRxCause),
    Deadline,
    Capacity,
    Clock,
    /// The radio front end did not retune the stopped port for this start.
    Retune(WifiRetuneError),
    /// The device lost its registers to an earlier failure.
    Abandoned,
    /// The data arenas quarantined themselves; every chain, ticket, key
    /// right and queue they hold stays with them.
    Arena(crate::lower_mac::arena::ArenaFault),
    /// The core asked for a retune right after recording the channel it was
    /// tuned to.
    TuneNotRecorded,
    /// A body setting could not be published or read back.
    Setting(settings::SettingFault),
    /// The device's registers were lent to an actor transaction or retired.
    HardwareUnavailable,
    /// The lower-MAC core is not in the view this step needs.
    CoreViewUnavailable(CoreOp),
    /// A hardware queue completed twice in one service.
    DuplicateCompletion {
        /// The hardware queue.
        queue: u8,
    },
    /// The core removed more keys than were installed.
    RemovalOverflow,
    /// A queue completed with no MPDU published on it.
    CompletionWithoutPending {
        /// The hardware queue.
        queue: u8,
    },
    /// A completed descriptor held no memory, or its place was gone.
    DescriptorMissing {
        /// The hardware queue.
        queue: u8,
    },
    /// A completion returned memory its buffer had not published.
    ReturnRefused {
        /// The hardware queue.
        queue: u8,
    },
    /// A delivered BlockAckReq came without its BlockAck report.
    BlockAckReportMissing {
        /// The hardware queue.
        queue: u8,
    },
    /// An ended MPDU recorded no transmission.
    AirMissing {
        /// The hardware queue.
        queue: u8,
    },
    /// An attempt's transmission or retry count overflowed.
    AttemptCounter {
        /// The hardware queue.
        queue: u8,
    },
    /// An attempt's retry plan has no stage at its position.
    StageMissing {
        /// The hardware queue.
        queue: u8,
    },
    /// A queue held more attempts than there are tickets.
    QueueFull {
        /// The hardware queue.
        queue: u8,
    },
    /// The caller would hold more received units than [`RX_HELD`].
    ReceiveOverflow,
    /// A received unit's MPDU lies outside the unit.
    ReceiveRange,
    /// A cut unit found the slot-loan path's event queue full.
    EventQueueFull,
    /// The data arenas' step completed an MPDU or removed a key.
    ArenaCoreContract,
    /// The installation's descriptors, chains, tickets or buffers were not
    /// all available to it.
    InstallationResources,
    /// The core refused the EDCA table.
    EdcaUnsupported,
    /// The regulatory domain did not permit the channel being started.
    ChannelNotPermitted,
    /// A key a command names is not installed, or no key place is free.
    KeySlotMissing(KeyOp),
    /// The caller returned more received units than it held, or the
    /// network more lends than the caller made.
    ReceiveAccounting,
    /// A command the data port held at its barrier passed its bound; the
    /// port keeps the command with its quarantine.
    CommandExpired(oer_protocols_ieee80211_contracts_port::CommandKind),
}

/// The lower-MAC operation a missing core view stopped.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CoreOp {
    /// An idle-core command: a key, a setting or a Block Ack agreement.
    Command,
    /// A TBTT of the station's schedule.
    Tbtt,
    /// The data arenas' core.
    Arena,
    /// A single MPDU's publication.
    Publish,
}

/// The key operation that found no key.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum KeyOp {
    /// An installation found no free key place.
    Install,
    /// A removal found no installed key of its identity.
    Remove,
    /// A replacement found no installed group key of its identity.
    Replace,
    /// A selection found no installed group key of its identity.
    Select,
    /// An attempt named a key that is not installed for transmission.
    Transmit,
}

/// Why the radio front end did not retune a stopped Wi-Fi client.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WifiRetuneError {
    /// The MAC or its RX walker was still active; nothing was posted.
    NotStopped(oer_chips_esp32s31_hal::owner::maintenance::Error),
    /// The radio system answered the retune with this failure.
    Radio(oer_chips_esp32s31_radio_client::Failure),
}
impl From<LowerMacFault> for Fault {
    fn from(value: LowerMacFault) -> Self {
        Self::Core(value)
    }
}

pub struct Esp32s31Caps<const B: usize>;
impl<const B: usize> Ieee80211Capabilities for Esp32s31Caps<B> {
    type SppScope = oer_protocols_ieee80211_wire::security::spp::Interface;
    const CAPS: LowerMacCapabilities = esp32s31_lower_mac_capabilities(B);
}

/// The driver's backing, borrowing the retained device and cross-context
/// storage. The driver's registration and the body's receive pull reach its
/// device `RefCell`, both from tasks of the station's executor.
pub struct PortStorage<D: Device<B>, const B: usize> {
    device: RefCell<&'static mut D>,
    shared: &'static mut PortSharedStorage<B, D::Lent>,
    identity: Cell<u64>,
    expired: Cell<u32>,
    /// The station and AP tables the driver last installed in its policy.
    #[cfg(test)]
    installed_edca: [Cell<Option<EdcaParameterSet>>; 2],
}

/// Caller-placed cross-context state, with no device, Cell, RefCell or shared
/// producer. Place once before constructing `PortStorage`; quarantine keeps
/// its DMA return places and key activity unavailable for reuse.
pub struct PortSharedStorage<const B: usize, P: LentNotifies = SameHart> {
    poisoned: oer_portable_memory::OnceValue<Fault>,
    // The driver's waker keeps a slot for a notifier the driver preempts;
    // beside the station on its executor none does. The body's consumer
    // never preempts its producer, so its AtomicWaker needs no second slot.
    driver: PriorityWaker,
    body: AtomicWaker,
    key_live: [KeyActivity; 20],
    /// Each pool buffer's place, holding it while the CPU owns it.
    returned: oer_portable_memory::ReturnBank<TxBacking<B>, TX_BUFFERS>,
    /// The receive units lent to the network.
    network: lent::NetworkCredit<P>,
    settings: settings::Settings,
    /// Slot and permission of a GTK command the installation quarantined.
    #[cfg(test)]
    retained_group: oer_portable_memory::OnceValue<(u8, u8)>,
    /// Previous and attempted KeyID of a quarantined GTK replacement, and
    /// whether its rollback restored the previous entry.
    #[cfg(test)]
    rolled_back: oer_portable_memory::OnceValue<(u8, u8, bool)>,
}
#[cfg(test)]
impl<Id: SlotIdentity, D: Device<B>, const B: usize, O: Observations> Esp32s31Backend<Id, D, B, O> {
    /// The installation's storage, readable after the backend is registered.
    pub(crate) fn storage_for_test(&self) -> &'static PortStorage<D, B> {
        self.storage
    }
    /// Keep one DMA allocation outside the bank, as a lost return would.
    pub(crate) fn withhold_dma(&self, index: usize) -> bool {
        match self.storage.shared.returned.take(index) {
            Some(taken) => {
                let _withheld = core::mem::ManuallyDrop::new(taken);
                true
            }
            None => false,
        }
    }
}
#[cfg(test)]
impl<D: Device<B>, const B: usize> PortStorage<D, B> {
    /// Slot and permission of a GTK command the installation quarantined.
    pub(crate) fn retained_group(&self) -> Option<(u8, u8)> {
        self.shared.retained_group.get()
    }
    /// The quarantined GTK replacement's KeyIDs and rollback result.
    pub(crate) fn rolled_back(&self) -> Option<(u8, u8, bool)> {
        self.shared.rolled_back.get()
    }
    /// The station and AP tables the driver last installed.
    pub(crate) fn installed_edca(&self) -> [Option<EdcaParameterSet>; 2] {
        [self.installed_edca[0].get(), self.installed_edca[1].get()]
    }
}
impl<D: Device<B>, const B: usize> PortStorage<D, B> {
    // Keep storage construction outside the install future's poll frame.
    #[inline(never)]
    pub const fn new(
        device: &'static mut D,
        shared: &'static mut PortSharedStorage<B, D::Lent>,
    ) -> Self {
        Self {
            device: RefCell::new(device),
            shared,
            identity: Cell::new(1),
            expired: Cell::new(0),
            #[cfg(test)]
            installed_edca: [const { Cell::new(None) }; 2],
        }
    }
    fn poison(&self, cause: Fault) {
        self.shared.poison(cause);
    }
    /// The device, unless another borrow holds it.
    fn device(&self) -> Result<core::cell::RefMut<'_, &'static mut D>, Fault> {
        self.device
            .try_borrow_mut()
            .map_err(|_| Fault::HardwareUnavailable)
    }
    fn check(&self) -> Result<(), radio::Poisoned<Fault>> {
        self.shared.check()
    }
}
impl<const B: usize> PortSharedStorage<B, SameHart> {
    /// Vacant cross-context backing, placed separately from the driver's device.
    pub const fn new() -> Self {
        Self::with_network(lent::SameHart::new())
    }
}
impl<const B: usize, P: LentNotifies> PortSharedStorage<B, P> {
    // Keep the waker's typed scope at the indirect call, independently of
    // the large terminal dispatchers that notify the body.
    #[inline(never)]
    fn wake_body(&self) {
        self.body.wake();
    }
    /// The backing of a port whose lent units' returns tell through
    /// `notifies`: for a network on another hart, its peers' signals.
    pub const fn with_network(notifies: P) -> Self {
        let network = lent::NetworkCredit::with(notifies);
        Self {
            poisoned: oer_portable_memory::OnceValue::new(),
            driver: PriorityWaker::new(),
            body: AtomicWaker::new(),
            key_live: [const { KeyActivity::new() }; 20],
            returned: oer_portable_memory::ReturnBank::new(),
            network,
            settings: settings::Settings::new(),
            #[cfg(test)]
            retained_group: oer_portable_memory::OnceValue::new(),
            #[cfg(test)]
            rolled_back: oer_portable_memory::OnceValue::new(),
        }
    }
    fn poison(&self, cause: Fault) {
        let _first = self.poisoned.set(cause).is_ok();
        #[cfg(test)]
        if _first {
            FIRST_POISON.with(|first| first.set(Some(cause)));
        }
        self.driver.wake();
        self.wake_body();
    }
    fn check(&self) -> Result<(), radio::Poisoned<Fault>> {
        self.poisoned
            .get()
            .map_or(Ok(()), |cause| Err(radio::Poisoned { cause }))
    }
}

/// The first fault an installation's port recorded: what poisoned it before
/// its driver or body ended. A copied observation that grants nothing; the
/// quarantine keeps every owner.
#[derive(Clone, Copy)]
pub struct FirstFault(&'static oer_portable_memory::OnceValue<Fault>);

impl FirstFault {
    /// The recorded fault, `None` while the port recorded none.
    pub fn get(&self) -> Option<Fault> {
        self.0.get()
    }
}

/// The bytes of one event-queue message, and of a received frame's own
/// fields, which every received frame moves through that queue.
#[cfg(test)]
pub(crate) const fn event_sizes<Id: SlotIdentity, D: Device<B>, const B: usize>() -> (usize, usize)
{
    (
        size_of::<owned::OwnedEvent<'static, Id, D, B>>(),
        size_of::<ReceiveBuffer<'static, Id, D::Receive>>() + size_of::<Esp32s31RxMeta>(),
    )
}

#[cfg(test)]
std::thread_local! {
    /// The cause the test thread's port published first: a frontend that
    /// sees its quarantine first cancels the body that would read it.
    pub(crate) static FIRST_POISON: core::cell::Cell<Option<Fault>> =
        const { core::cell::Cell::new(None) };
}

impl<const B: usize> Default for PortSharedStorage<B, SameHart> {
    fn default() -> Self {
        Self::new()
    }
}

/// The complete construction resources; no DMA storage lives inline here.
pub struct PortParts<D: Device<B>, const B: usize, O: Observations> {
    pub storage: &'static mut PortStorage<D, B>,
    pub transmitter: OrdinaryTxIdleOwner<'static, D::Power, D::Entropy, D::Timer, B>,
    /// The transmit pool. The port holds its first `bank` buffers from here
    /// on, one at each of the first `bank` buffer places; every other place
    /// holds no memory until a network lends it a buffer of this pool for
    /// one MSDU ([`TxBufferPool::install`]).
    pub pool: &'static TxPool<B>,
    /// The pool buffers the port holds itself, at most [`TX_BUFFERS`].
    pub bank: u8,
    /// The idle descriptors its buffers are published through.
    pub descriptors: TxDescriptors<B>,
    pub config: LowerMacConfig,
    pub access_point_address: IndividualAddress,
    pub coex: &'static D::Coex,
    /// The driver's receiver of its own publication observations; `()`
    /// keeps nothing and builds nothing.
    pub observations: O,
}

pub struct Esp32s31Backend<Id: SlotIdentity, D: Device<B>, const B: usize, O: Observations> {
    storage: &'static PortStorage<D, B>,
    state: BackendState<D, B, O>,
    channel: Channel,
    addresses: [IndividualAddress; 2],
    timer: D::Timer,
    config: LowerMacConfig,
    coex: &'static D::Coex,
    identity: PhantomData<fn(Id) -> (Id, O)>,
}
struct Initial<D: Device<B>, const B: usize, O: Observations> {
    transmitter: OrdinaryTxIdleOwner<'static, D::Power, D::Entropy, D::Timer, B>,
    descriptors: TxDescriptors<B>,
    observations: O,
    config: LowerMacConfig,
    coex: &'static D::Coex,
}
/// What a stopped port reinstalls with: the disabled core's transmitter,
/// the idle descriptors and the driver's observation receiver.
type Reusable<D, const B: usize, O: Observations> = (
    OrdinaryTxIdleOwner<
        'static,
        <D as Device<B>>::Power,
        <D as Device<B>>::Entropy,
        <D as Device<B>>::Timer,
        B,
    >,
    TxDescriptors<B>,
    O,
);
enum BackendState<D: Device<B>, const B: usize, O: Observations> {
    Initial(Option<Initial<D, B, O>>),
    Running,
}
impl<Id: SlotIdentity, D: Device<B>, const B: usize, O: Observations> Esp32s31Backend<Id, D, B, O> {
    /// The A-MPDUs this port's data arenas send: up to
    /// [`TX_AMPDU_SUBFRAMES`] subframes.
    pub const AMPDU: Option<AmpduCapabilities> =
        Some(esp32s31_ampdu_capabilities(TX_AMPDU_SUBFRAMES));

    #[inline(never)]
    pub fn new(parts: PortParts<D, B, O>) -> Result<Self, PortParts<D, B, O>> {
        let Ok(station) = IndividualAddress::for_interface(parts.config.station_address) else {
            return Err(parts);
        };
        if parts.config.publication_timeout > PUBLICATION_BOUND
            || parts.config.publication_timeout == Duration::ZERO
            || IndividualAddress::for_interface(parts.access_point_address.get()).is_err()
            || !Esp32s31Caps::<B>::CAPS
                .supports_channel(Channel::from_wifi_channel(parts.config.channel))
            || B < crate::ordinary_tx::TX_METADATA_SIZE
                + 24
                + crate::ordinary_tx::TX_CCMP_MIC_SIZE
                + crate::ordinary_tx::TX_FCS_SIZE
            || usize::from(parts.bank) > TX_BUFFERS
            || !parts.descriptors.idle()
            || parts.storage.shared.poisoned.get().is_some()
        {
            return Err(parts);
        }
        let timer = parts.storage.device.get_mut().timer().clone();
        let PortParts {
            storage,
            transmitter,
            pool,
            bank,
            descriptors,
            config,
            access_point_address,
            coex,
            observations,
        } = parts;
        // The port takes each of its pool buffers at its place. A buffer
        // the pool does not give, or a place already held, poisons this
        // installation: the allocation stays out of the bank and enable
        // refuses with the fault.
        for index in 0..TX_BUFFERS {
            let seeded = u8::try_from(index).ok().is_some_and(|place| {
                place >= bank
                    || claim(pool, place)
                        .is_some_and(|memory| storage.shared.returned.seed(index, memory).is_ok())
            });
            if !seeded {
                storage.shared.poison(Fault::InstallationResources);
            }
        }
        let channel = Channel::from_wifi_channel(config.channel);
        Ok(Self {
            storage,
            state: BackendState::Initial(Some(Initial {
                transmitter,
                descriptors,
                observations,
                config,
                coex,
            })),
            channel,
            addresses: [station, access_point_address],
            timer,
            config,
            coex,
            identity: PhantomData,
        })
    }
}

/// Buffer `place` of `pool`, as the radio holds it.
fn claim<const B: usize>(pool: &'static TxPool<B>, place: u8) -> Option<TxBacking<B>> {
    pool.claim_network(place)
        .ok()?
        .publish(oer_portable_memory::FrameLen::constant::<0>(), |_| {});
    pool.claim_radio(place).ok()
}

/// Hardware and reusable DMA owners returned after confirmed normal stopping.
pub struct StoppedPort<Id: SlotIdentity, D: Device<B>, const B: usize, O: Observations> {
    storage: &'static PortStorage<D, B>,
    timer: D::Timer,
    reusable: Option<Reusable<D, B, O>>,
    config: LowerMacConfig,
    addresses: [IndividualAddress; 2],
    coex: &'static D::Coex,
    identity: PhantomData<fn(Id) -> Id>,
}
impl<Id: SlotIdentity, D: Device<B>, const B: usize, O: Observations> StoppedPort<Id, D, B, O> {
    pub fn expired_msdus(&self) -> u32 {
        self.storage.expired.get()
    }
    /// The returned device may recover its stopped PHY/IRQ ownership here;
    /// `HardwareUnavailable` while another borrow holds it.
    pub fn with_device<R>(&mut self, use_device: impl FnOnce(&mut D) -> R) -> Result<R, Fault> {
        Ok(use_device(&mut **self.storage.device()?))
    }

    /// Begin a fresh installation on the same static hardware/RX backing.
    /// The image supplies a distinct TSF epoch. Forgotten TX owners retain
    /// their allocations, so the previous stopped owner returns on refusal.
    #[inline(never)]
    pub fn reinstall(mut self, tsf_epoch: u32) -> Result<Esp32s31Backend<Id, D, B, O>, Self> {
        if self.storage.shared.poisoned.get().is_some()
            || !self.storage.shared.returned.none_out()
            || tsf_epoch == self.config.tsf_epoch
        {
            return Err(self);
        }
        let Some((transmitter, descriptors, observations)) = self.reusable.take() else {
            return Err(self);
        };
        self.config.tsf_epoch = tsf_epoch;
        let channel = Channel::from_wifi_channel(self.config.channel);
        let timer = self.timer.clone();
        Ok(Esp32s31Backend {
            storage: self.storage,
            state: BackendState::Initial(Some(Initial {
                transmitter,
                descriptors,
                observations,
                config: self.config,
                coex: self.coex,
            })),
            channel,
            addresses: self.addresses,
            timer,
            config: self.config,
            coex: self.coex,
            identity: PhantomData,
        })
    }
}

pub struct Tickets;
pub struct Buffers;
pub struct BlockAcks;
pub struct GroupPermission<R>(PhantomData<fn() -> R>);
pub struct Types<Id: SlotIdentity, D, const B: usize, O: Observations>(
    PhantomData<fn(Id) -> (Id, D, O)>,
);

type Single<'id, Id: SlotIdentity, D, const B: usize, O: Observations> = InFlight<
    <D as Device<B>>::OwnedContext<Id>,
    Slot<'id, Tickets, Part<Id, 2>>,
    Mpdu<'id, Id, D, B, O>,
>;
type Event<'id, Id, D, const B: usize, O: Observations> = LowerMacEvent<'id, Types<Id, D, B, O>>;
type Terminal<'id, Id: SlotIdentity, D, const B: usize, O: Observations> =
    LowerMacTerminal<'id, Types<Id, D, B, O>>;

mod handoff;
use handoff::{Attached, FrontHeld, Held, HeldBacking, Incoming, Outgoing};

pub struct ReceiveBuffer<'id, Id: SlotIdentity, R> {
    brand: PhantomData<fn(&'id (), Id) -> (&'id (), Id)>,
    /// The unit, its view narrowed to the MPDU. A lost buffer never drops
    /// it: its original DMA lease stays quarantined.
    unit: core::mem::ManuallyDrop<R>,
    loss: LossGuard,
}
/// Poisons the installation when its receive buffer is lost: dropping a
/// buffer does not authorize rearming its allocation.
pub(crate) struct LossGuard {
    poison: &'static oer_portable_memory::OnceValue<Fault>,
    driver: &'static PriorityWaker,
    armed: bool,
}
impl LossGuard {
    pub(crate) fn new(
        poison: &'static oer_portable_memory::OnceValue<Fault>,
        driver: &'static PriorityWaker,
    ) -> Self {
        Self {
            poison,
            driver,
            armed: true,
        }
    }
    /// The buffer's unit leaves by an explicit path; nothing is lost.
    pub(crate) fn disarm(mut self) -> &'static PriorityWaker {
        self.armed = false;
        self.driver
    }
}
impl Drop for LossGuard {
    fn drop(&mut self) {
        if self.armed {
            let _ = self.poison.set(Fault::Abandoned);
            self.driver.wake();
        }
    }
}
impl<'id, Id: SlotIdentity, R> ReceiveBuffer<'id, Id, R> {
    pub(crate) fn new(unit: R, loss: LossGuard) -> Self {
        Self {
            brand: PhantomData,
            unit: core::mem::ManuallyDrop::new(unit),
            loss,
        }
    }
    /// The unit, by an explicit return path, and the driver to wake.
    pub(crate) fn into_unit(self) -> (R, &'static PriorityWaker) {
        let Self { unit, loss, .. } = self;
        (core::mem::ManuallyDrop::into_inner(unit), loss.disarm())
    }
    /// The same buffer under another brand.
    pub(crate) fn rebrand<'p>(self) -> ReceiveBuffer<'p, Id, R> {
        let Self { unit, loss, .. } = self;
        ReceiveBuffer {
            brand: PhantomData,
            unit,
            loss,
        }
    }
}
impl<Id: SlotIdentity, R: ReceiveUnit> RxBuffer for ReceiveBuffer<'_, Id, R> {
    fn bytes(&self) -> &[u8] {
        self.unit.bytes()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Ack {
    pub snr_db: i8,
}

impl Ack {
    fn from_sample(sample: Option<i8>) -> Result<Self, Fault> {
        sample.map(|snr_db| Self { snr_db }).ok_or(Fault::Capacity)
    }
}

// Successful ordinary completions always supply the signed vendor sample;
// group/no-ACK transmissions carry SingleResponse::Unsolicited instead.
impl AckSnr for Ack {
    fn snr_db(&self) -> i8 {
        self.snr_db
    }
}

impl<Id: SlotIdentity, D: Device<B>, const B: usize, O: Observations> LowerMacBackend<Id>
    for Esp32s31Backend<Id, D, B, O>
{
    type Caps = Esp32s31Caps<B>;
    const TX_QUEUE_DEPTH: core::num::NonZeroU8 =
        core::num::NonZeroU8::new(8).expect("eight admission tickets");
    const PUBLICATION_BOUND: Duration = PUBLICATION_BOUND;
    const GROUP_COMMAND_BOUND: Duration = COMMAND_BOUND;
}

#[cfg(test)]
mod ack_tests {
    use super::*;
    #[test]
    fn rate_control_receives_the_real_ack_sample_and_missing_evidence_is_refused() {
        for sample in [i8::MIN, -7, 0, i8::MAX] {
            let ack = Ack::from_sample(Some(sample)).expect("measured ACK");
            assert_eq!(ack.snr_db(), sample);
        }
        assert_eq!(Ack::from_sample(None), Err(Fault::Capacity));
    }
}
