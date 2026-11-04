//! The S31 lower-MAC port: [`Esp32s31Port`], the radio as the station's
//! datapath actor owns it, with the device, the driver's state and the
//! transmit pool's places it runs on.
//!
//! P3 owns the interrupt epoch and posts static typed signals through
//! [`crate::irq`]; it never accesses this state. All DMA allocations and
//! the device remain in static storage through an abnormal exit. Neither a
//! dropped future nor a forgotten port frees them.

use crate::mac::{
    rx::{NormalizedRxFrame, RxError},
    tx::{LegacyTxQueue, TxBacking, TxSlot, TxSlotAccess},
};
use crate::{
    lower_mac::{self as core_mac, *},
    ordinary_tx::{OrdinaryTxIdleOwner, WifiTxEntropy, WifiTxPowerProfile},
    tx::WifiTxWake,
};
use core::{
    cell::Cell,
    convert::Infallible,
    future::Future,
    marker::PhantomData,
    pin::Pin,
    sync::atomic::{AtomicBool, Ordering},
    task::{Context, Poll},
};
use oer_portable_time::{Duration, Instant, Timer};
use oer_protocols_ieee80211_contracts_port::association;
use oer_protocols_ieee80211_contracts_port::*;
use oer_protocols_ieee80211_contracts_security::Ccmp128Key;
use oer_protocols_ieee80211_wire::{address::IndividualAddress, ccmp::GroupKeyId};
use oer_protocols_radio_port::{self as radio, Child, Root, Slot};
use oer_protocols_radio_port::{BodyLoan, ClockError, LoanedContext, SlotIdentity};

mod atomic_words;
use atomic_words::KeyActivity;
mod buffer;
#[cfg(target_arch = "riscv32")]
pub mod device;
mod driver;
mod engines;
pub use engines::{
    Esp32s31Arena, Esp32s31Commands, Esp32s31Port, GroupInstall, InstallRefused, PairwiseInstall,
    PortDriver, PortInstallation, ReinstallRefused,
};
mod keys;
mod lent;
pub use lent::{LentNotifies, LentReceive, NetworkPeers, SameHart};
mod installation;
pub use installation::{
    BlockAckSlots, Esp32s31Backend, InstallationPools, Resources, StationTokens,
};
#[cfg(any(target_arch = "riscv32", test))]
mod receive;
#[cfg(all(oer_trace, oer_debug_control))]
pub mod trace;
pub use buffer::*;
pub use keys::{GroupKey, KeyIdentity, PairwiseKey, PortKeys};
mod brand;
pub use crate::lower_mac::arena::BlockAckAnswer;
pub use crate::lower_mac::{ContentionDraw, Observations};

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

/// The receive units the port's caller holds at once besides those it lent
/// the network: its received frames and the MPDUs its receive Block Ack
/// agreements keep,
/// one 16-MPDU reorder window, the archive's choice: a 32-MPDU window
/// left too few descriptors armed under sustained HT40 traffic. The network leaves them in the receive pool
/// ([`lent_rx`]).
pub const RX_HELD: usize = 16;

/// The receive units of a pool of `units` (the units that may be outside
/// the ring at once) the network may hold. It leaves the radio
/// [`RX_HELD`] units and the admission's critical reserve: when the caller holds a
/// full reorder window and the network all it may, the pool still stages
/// the MPDU that closes the window's hole, which the hardware already
/// acknowledged, as ordinary bulk input.
pub const fn lent_rx(units: usize) -> usize {
    units.saturating_sub(RX_HELD + crate::rx::transaction::DEFAULT_CRITICAL_RESERVE)
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

/// One bounded hardware step of the data port. Implementations retain all
/// progress in themselves across `Pending`; no ISR may call any of these
/// operations. The device owns the cooperative registers, RX ring and PHY
/// membership.
pub trait Device<const B: usize>: Sized + 'static {
    /// Image-selected concrete coexistence policy, statically dispatched in
    /// the datapath actor's task.
    type Coex: RadioCoexPriorities + 'static;
    type Power: WifiTxPowerProfile;
    type Entropy: WifiTxEntropy;
    type Timer: Timer + Clone + 'static;
    type Hardware: LowerMacHardware;
    type Receive: ReceiveUnit + Send;
    /// How a lent receive unit's return tells the task waiting for
    /// returns: [`SameHart`], or the signal of a network on another hart
    /// ([`NetworkPeers`]).
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
    /// The device's next deadline for the port's turn. A due receive
    /// continuation is not one: the port's receive pull takes it
    /// ([`Self::pull_unit`]).
    fn next_deadline(&self) -> Option<Instant>;
    /// Whether a transmit or other event waits for the next poll.
    fn event_waiting(&self) -> bool {
        false
    }
    fn take_power_profile(&mut self) -> Option<Self::Power>;
    fn radio_now(&mut self) -> Result<Ieee80211Instant, ClockError>;
    fn poll_start(
        &mut self,
        cx: &mut Context<'_>,
        channel: oer_protocols_ieee80211_wire::channel::WifiChannel,
    ) -> Poll<Result<(), Fault>>;
    fn poll_stop(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Fault>>;
    /// The port's facts: take the interrupt facts but the receive ones
    /// ([`Self::poll_rx_work`]), and return a transmit or TBTT event. This
    /// never services the ring.
    fn poll_event(&mut self, cx: &mut Context<'_>) -> Poll<Result<DeviceEvent, Fault>>;
    /// A finite RX cut: mask/drain preceding DMA and ISR facts before a GTK
    /// mutation. Returned RX must be delivered before its command terminal.
    fn poll_rx_cut(
        &mut self,
        receive_room: usize,
        cx: &mut Context<'_>,
    ) -> Poll<Result<Option<Self::Receive>, Fault>>;
    fn finish_rx_cut(&mut self) -> Result<(), Fault>;
    /// The data port's receive facts: take them, registering `cx` alone
    /// on their signal, and report whether the ring holds work: a staged
    /// unit, or ring work that waits for no continuation (the pull that
    /// answered [`DevicePull::Later`] left that work to the caller's
    /// alarm). It reads no clock; while a cut or a stop holds the ring it
    /// is `false`.
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
    /// Mask the ring's interrupts and erase the keys of a retired
    /// installation.
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
    /// A received buffer was dropped without its recycle: its allocation
    /// stays out of the ring.
    ReceiveLost,
    /// The data arenas' step completed an MPDU or removed a key.
    ArenaCoreContract,
    /// The installation's descriptors, chains, tickets or buffers were not
    /// all available to it.
    InstallationResources,
    /// The core refused the EDCA table.
    EdcaUnsupported,
    /// The regulatory domain did not permit the channel being started.
    ChannelNotPermitted,
    /// The channel being started has no 2.4 GHz definition, the only band
    /// the front end tunes.
    ChannelUnsupported,
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

/// Caller-placed state the port shares with the owners it lends: the
/// transmit pool's return places, the receive units lent to the network and
/// each installed key's activity. Place once before the first installation;
/// a quarantine keeps its DMA return places and key activity unavailable
/// for reuse.
pub struct PortSharedStorage<const B: usize, P: LentNotifies = SameHart> {
    /// Set when a received buffer was dropped without its recycle.
    lost: AtomicBool,
    key_live: [KeyActivity; 20],
    /// Each pool buffer's place, holding it while the CPU owns it.
    returned: oer_portable_memory::ReturnBank<TxBacking<B>, TX_BUFFERS>,
    /// The receive units lent to the network.
    network: lent::NetworkCredit<P>,
    /// Slot and permission of a GTK command the installation quarantined.
    #[cfg(test)]
    retained_group: oer_portable_memory::OnceValue<(u8, u8)>,
    /// Previous and attempted KeyID of a quarantined GTK replacement, and
    /// whether its rollback restored the previous entry.
    #[cfg(test)]
    rolled_back: oer_portable_memory::OnceValue<(u8, u8, bool)>,
}
impl<const B: usize> PortSharedStorage<B, SameHart> {
    /// Vacant shared state, placed separately from the device.
    pub const fn new() -> Self {
        Self::with_network(lent::SameHart::new())
    }
}
impl<const B: usize, P: LentNotifies> PortSharedStorage<B, P> {
    /// The backing of a port whose lent units' returns tell through
    /// `notifies`: for a network on another hart, its signal.
    pub const fn with_network(notifies: P) -> Self {
        let network = lent::NetworkCredit::with(notifies);
        Self {
            lost: AtomicBool::new(false),
            key_live: [const { KeyActivity::new() }; 20],
            returned: oer_portable_memory::ReturnBank::new(),
            network,
            #[cfg(test)]
            retained_group: oer_portable_memory::OnceValue::new(),
            #[cfg(test)]
            rolled_back: oer_portable_memory::OnceValue::new(),
        }
    }
    /// Whether a received buffer was dropped without its recycle.
    fn lost(&self) -> bool {
        self.lost.load(Ordering::Acquire)
    }
}

impl<const B: usize> Default for PortSharedStorage<B, SameHart> {
    fn default() -> Self {
        Self::new()
    }
}

/// The complete construction resources; no DMA storage lives inline here.
pub struct PortParts<D: Device<B>, const B: usize, O: Observations> {
    /// The device, which the port owns from its installation on.
    pub device: &'static mut D,
    /// The state the port shares with the owners it lends.
    pub shared: &'static mut PortSharedStorage<B, D::Lent>,
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
/// Buffer `place` of `pool`, as the radio holds it.
fn claim<const B: usize>(pool: &'static TxPool<B>, place: u8) -> Option<TxBacking<B>> {
    pool.claim_network(place)
        .ok()?
        .publish(oer_portable_memory::FrameLen::constant::<0>(), |_| {});
    pool.claim_radio(place).ok()
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
type Terminal<'id, Id: SlotIdentity, D, const B: usize, O: Observations> =
    LowerMacTerminal<'id, Types<Id, D, B, O>>;

pub struct ReceiveBuffer<'id, Id: SlotIdentity, R> {
    brand: PhantomData<fn(&'id (), Id) -> (&'id (), Id)>,
    /// The unit, its view narrowed to the MPDU. A lost buffer never drops
    /// it: its original DMA lease stays quarantined.
    unit: core::mem::ManuallyDrop<R>,
    loss: LossGuard,
}
/// Records the loss of its receive buffer: dropping a buffer does not
/// authorize rearming its allocation, and the port's next call answers
/// with [`Fault::ReceiveLost`].
pub(crate) struct LossGuard {
    lost: &'static AtomicBool,
    armed: bool,
}
impl LossGuard {
    pub(crate) fn new(lost: &'static AtomicBool) -> Self {
        Self { lost, armed: true }
    }
    /// The buffer's unit leaves by an explicit path; nothing is lost.
    pub(crate) fn disarm(mut self) {
        self.armed = false;
    }
}
impl Drop for LossGuard {
    fn drop(&mut self) {
        if self.armed {
            self.lost.store(true, Ordering::Release);
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
    /// The unit, by an explicit return path.
    pub(crate) fn into_unit(self) -> R {
        let Self { unit, loss, .. } = self;
        loss.disarm();
        core::mem::ManuallyDrop::into_inner(unit)
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
