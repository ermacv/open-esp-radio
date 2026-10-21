//! The S31 lower-MAC port. Only the driver loan executes hardware operations.
//!
//! The image polls `run_port` on P2. P3 owns the interrupt epoch and posts
//! static typed signals through [`crate::irq`]; it never accesses this state.
//! All DMA allocations and the device remain in static storage through an
//! abnormal exit. Neither a dropped future nor a forgotten port frees them.

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
use oer_esp32s31_ieee80211_mac::{
    rx::{NormalizedRxFrame, RxError},
    tx::{LegacyTxQueue, TxSlot, TxSlotAccess, TxSlotFree},
};
use oer_ieee80211_port::association;
use oer_ieee80211_port::*;
use oer_ieee80211_wire::{
    address::IndividualAddress,
    ccmp::{Ccmp128Key, GroupKeyId},
};
use oer_radio_port::{self as radio, Child, Root, Slot};
use oer_radio_port::{BodyLoan, ClockError, LoanedContext, SlotBodyOwner, SlotIdentity, SlotOwner};
use oer_time::{Clock, Duration, Instant, Timer};

mod admission;
use admission::BufferAdmission;
mod atomic_words;
use atomic_words::KeyActivity;
mod buffer;
#[cfg(target_arch = "riscv32")]
pub mod device;
mod driver;
mod keys;
mod lifecycle;
mod phase;
mod placement;
use placement::{Prepared, PreparedCell};
#[cfg(all(oer_trace, oer_debug_control))]
pub mod trace;
pub use buffer::*;
pub use keys::{GroupKey, KeyIdentity, PairwiseKey, PortKeys};
mod loan;
mod owned;
pub use phase::{Esp32s31Interface, Esp32s31Phase, Esp32s31Setup};

/// Shared admission capacity across the four access categories.
pub const TX_TICKETS: usize = 8;
const INTERFACE_COMMANDS: usize = InterfaceCommandPool::<Station>::COMMAND_CAPACITY
    + InterfaceCommandPool::<AccessPoint>::COMMAND_CAPACITY;
const INTERFACE_TERMINALS: usize = InterfaceCommandPool::<Station>::TERMINAL_CAPACITY
    + InterfaceCommandPool::<AccessPoint>::TERMINAL_CAPACITY;
const TERMINALS: usize = TX_TICKETS + 16 + 2 + 1 + INTERFACE_TERMINALS;
// Each PTK may be removed before P2 runs its installation command.
const COMMANDS: usize = TX_TICKETS + 2 * 16 + 2 + 2 + INTERFACE_COMMANDS;
pub const PUBLICATION_BOUND: Duration = Duration::from_millis(250);
pub const COMMAND_BOUND: Duration = Duration::from_millis(400);
/// Response/retirement containment budget. It covers one P2 cleanup step and
/// ready P2 work within the existing command bound; a later answer fails closed.
pub const SHUTDOWN_BOUND: Duration = COMMAND_BOUND;
type Part<Id, const I: u8> = Child<Root<Id>, I>;
type CorePath<Id: SlotIdentity> = Part<Id, 0>;
type Core<'id, Id: SlotIdentity, D, const B: usize> = LowerMacCore<
    'id,
    'static,
    <D as Device<B>>::Profile,
    <D as Device<B>>::Power,
    <D as Device<B>>::Entropy,
    <D as Device<B>>::Timer,
    B,
    CorePath<Id>,
    TX_TICKETS,
>;

/// An owned RX allocation from the cooperative receive owner.
pub trait ReceiveUnit: 'static {
    fn normalized(&self) -> Result<NormalizedRxFrame<'_>, RxError>;
    fn bytes(&self) -> &[u8];
}

/// One bounded P2 hardware step. Implementations retain all progress in
/// themselves across `Pending`; no ISR may call any of these operations.
/// The device owns the cooperative registers, RX ring and PHY membership.
pub trait Device<const B: usize>: Sized + 'static {
    type Profile: backend::BackendTxProfile;
    type Power: WifiTxPowerProfile;
    type Entropy: WifiTxEntropy;
    type Timer: Timer + Clone + 'static;
    type Hardware: LowerMacHardware;
    type Receive: ReceiveUnit;
    type Body: TxBody + 'static;
    type OwnedContext<Id: SlotIdentity>;
    type Context<'p, Id: SlotIdentity + 'p>: LoanedContext<'p, Id, Owned = Self::OwnedContext<Id>>
        + 'p;
    type OwnedConfigureContext<Id: SlotIdentity>;
    type ConfigureContext<'p, Id: SlotIdentity + 'p>: LoanedContext<'p, Id, Owned = Self::OwnedConfigureContext<Id>>
        + 'p;
    fn hardware(&mut self) -> &mut Self::Hardware;
    fn timer(&self) -> &Self::Timer;
    fn next_deadline(&self) -> Option<Instant>;
    fn take_power_profile(&mut self) -> Option<Self::Power>;
    fn radio_now(&mut self) -> Result<Ieee80211Instant, ClockError>;
    fn poll_start(
        &mut self,
        cx: &mut Context<'_>,
        channel: oer_ieee80211_wire::channel::WifiChannel,
    ) -> Poll<Result<(), Fault>>;
    fn poll_stop(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Fault>>;
    fn poll_receive(
        &mut self,
        receive_room: usize,
        cx: &mut Context<'_>,
    ) -> Poll<Result<DeviceEvent<Self::Receive>, Fault>>;
    /// A finite RX cut: mask/drain preceding DMA and ISR facts before a GTK
    /// mutation. Returned RX must be delivered before its command terminal.
    fn poll_rx_cut(
        &mut self,
        receive_room: usize,
        cx: &mut Context<'_>,
    ) -> Poll<Result<Option<Self::Receive>, Fault>>;
    fn finish_rx_cut(&mut self) -> Result<(), Fault>;
    fn recycle(&mut self, frame: Self::Receive);
    /// Mask requests and erase keys synchronously. A successful stop must
    /// include both DMA and entered IRQ operations; otherwise return false.
    fn abandon(&mut self) -> bool;
    fn detach(&mut self);
}

pub enum DeviceEvent<R> {
    Receive(R),
    Tx(WifiTxWake),
    Tbtt,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Fault {
    Core(LowerMacFault),
    Receive(crate::rx::control::WifiRxCause),
    Deadline,
    Capacity,
    Clock,
    Abandoned,
}
impl From<LowerMacFault> for Fault {
    fn from(value: LowerMacFault) -> Self {
        Self::Core(value)
    }
}

pub struct Esp32s31Caps<const B: usize>;
impl<const B: usize> Ieee80211Capabilities for Esp32s31Caps<B> {
    type SppScope = oer_ieee80211_wire::security::spp::Interface;
    const CAPS: LowerMacCapabilities = esp32s31_lower_mac_capabilities(B);
}

/// Static task-only backing borrowing a separately retained device.
/// Its `RefCell` is never shared with an ISR.
pub struct PortStorage<D: 'static, const B: usize> {
    device: RefCell<&'static mut D>,
    poisoned: oer_memory::OnceValue<Fault>,
    driver: AtomicWaker,
    body: AtomicWaker,
    key_live: [KeyActivity; 20],
    identity: Cell<u64>,
    returned: oer_memory::ReturnBank<TxSlotFree<'static, B>, TX_TICKETS>,
    expired: Cell<u32>,
}
impl<D: Device<B>, const B: usize> PortStorage<D, B> {
    // Keep storage construction outside the install future's poll frame.
    #[inline(never)]
    pub const fn new(device: &'static mut D) -> Self {
        Self {
            device: RefCell::new(device),
            poisoned: oer_memory::OnceValue::new(),
            driver: AtomicWaker::new(),
            body: AtomicWaker::new(),
            key_live: [const { KeyActivity::new() }; 20],
            identity: Cell::new(1),
            returned: oer_memory::ReturnBank::new(),
            expired: Cell::new(0),
        }
    }
    fn poison(&self, cause: Fault) {
        let _ = self.poisoned.set(cause);
        self.driver.wake();
        self.body.wake();
    }
    fn check(&self) -> Result<(), radio::Poisoned<Fault>> {
        self.poisoned
            .get()
            .map_or(Ok(()), |cause| Err(radio::Poisoned { cause }))
    }
}

/// The complete construction resources; no DMA storage lives inline here.
pub struct PortParts<D: Device<B>, const B: usize> {
    pub storage: &'static mut PortStorage<D, B>,
    pub transmitter: OrdinaryTxIdleOwner<'static, D::Profile, D::Power, D::Entropy, D::Timer, B>,
    pub buffers: [Pin<&'static mut TxSlot<B>>; TX_TICKETS],
    pub config: LowerMacConfig,
    pub access_point_address: IndividualAddress,
    pub coex: &'static dyn RadioCoexPriorities,
}

pub struct Esp32s31Backend<Id: SlotIdentity, D: Device<B>, const B: usize> {
    storage: &'static PortStorage<D, B>,
    state: BackendState<D, B>,
    channel: Channel,
    addresses: [IndividualAddress; 2],
    timer: D::Timer,
    config: LowerMacConfig,
    coex: &'static dyn RadioCoexPriorities,
    identity: PhantomData<fn(Id) -> Id>,
}
struct Initial<D: Device<B>, const B: usize> {
    transmitter: OrdinaryTxIdleOwner<'static, D::Profile, D::Power, D::Entropy, D::Timer, B>,
    config: LowerMacConfig,
    coex: &'static dyn RadioCoexPriorities,
}
enum BackendState<D: Device<B>, const B: usize> {
    Initial(Option<Initial<D, B>>),
    Running,
}
impl<Id: SlotIdentity, D: Device<B>, const B: usize> Esp32s31Backend<Id, D, B> {
    /// Optional aggregate admission of this port implementation. Its
    /// `NoAggregate` owner is uninhabited and it implements no `AmpduPhase`.
    pub const AMPDU: Option<AmpduCapabilities> = None;

    #[inline(never)]
    pub fn new(parts: PortParts<D, B>) -> Result<Self, PortParts<D, B>> {
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
            || parts
                .buffers
                .iter()
                .any(|slot| slot.state() != oer_esp32s31_ieee80211_mac::tx::TxDmaState::Free)
            || parts.storage.poisoned.get().is_some()
        {
            return Err(parts);
        }
        let timer = parts.storage.device.borrow().timer().clone();
        let PortParts {
            storage,
            transmitter,
            buffers,
            config,
            access_point_address,
            coex,
        } = parts;
        for (index, slot) in buffers.into_iter().enumerate() {
            let TxSlotAccess::Free(memory) = slot.access() else {
                panic!("installation takes only idle TX allocations")
            };
            if storage.returned.seed(index, memory).is_err() {
                panic!("initial installation seeds each return place once");
            }
        }
        let channel = Channel::from_wifi_channel(config.channel);
        Ok(Self {
            storage,
            state: BackendState::Initial(Some(Initial {
                transmitter,
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

/// Hardware and reusable DMA owners returned after confirmed normal stopping.
pub struct StoppedPort<Id: SlotIdentity, D: Device<B>, const B: usize> {
    storage: &'static PortStorage<D, B>,
    transmitter:
        Option<OrdinaryTxIdleOwner<'static, D::Profile, D::Power, D::Entropy, D::Timer, B>>,
    config: LowerMacConfig,
    addresses: [IndividualAddress; 2],
    coex: &'static dyn RadioCoexPriorities,
    identity: PhantomData<fn(Id) -> Id>,
}
impl<Id: SlotIdentity, D: Device<B>, const B: usize> StoppedPort<Id, D, B> {
    pub fn expired_msdus(&self) -> u32 {
        self.storage.expired.get()
    }
    /// The returned device may recover its stopped PHY/IRQ ownership here.
    pub fn with_device<R>(&mut self, use_device: impl FnOnce(&mut D) -> R) -> R {
        use_device(&mut *self.storage.device.borrow_mut())
    }

    /// Begin a fresh installation on the same static hardware/RX backing.
    /// The image supplies a distinct TSF epoch. Forgotten TX owners retain
    /// their allocations, so the previous stopped owner returns on refusal.
    #[inline(never)]
    pub fn reinstall(mut self, tsf_epoch: u32) -> Result<Esp32s31Backend<Id, D, B>, Self> {
        if self.storage.poisoned.get().is_some()
            || !self.storage.returned.all_returned()
            || self.transmitter.is_none()
            || tsf_epoch == self.config.tsf_epoch
        {
            return Err(self);
        }
        let transmitter = self
            .transmitter
            .take()
            .expect("stopped core returned its transmitter");
        self.config.tsf_epoch = tsf_epoch;
        let channel = Channel::from_wifi_channel(self.config.channel);
        let timer = self.storage.device.borrow().timer().clone();
        Ok(Esp32s31Backend {
            storage: self.storage,
            state: BackendState::Initial(Some(Initial {
                transmitter,
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
pub struct Types<Id: SlotIdentity, D, const B: usize>(PhantomData<fn(Id) -> (Id, D)>);

type Single<'id, Id: SlotIdentity, D, const B: usize> = InFlight<
    <D as Device<B>>::OwnedContext<Id>,
    Slot<'id, Tickets, Part<Id, 2>>,
    Mpdu<'id, Id, D, B>,
>;
type Event<'id, Id, D, const B: usize> = LowerMacEvent<'id, Types<Id, D, B>>;
type Terminal<'id, Id: SlotIdentity, D, const B: usize> = LowerMacTerminal<'id, Types<Id, D, B>>;

struct Queue<T, const N: usize>(RefCell<heapless::Deque<T, N>>);
impl<T, const N: usize> Queue<T, N> {
    const fn new() -> Self {
        Self(RefCell::new(heapless::Deque::new()))
    }
    fn push(&self, value: T) -> Result<(), T> {
        self.0.borrow_mut().push_back(value)
    }
    fn pop(&self) -> Option<T> {
        self.0.borrow_mut().pop_front()
    }
    fn empty(&self) -> bool {
        self.0.borrow().is_empty()
    }
}

pub struct Held<'id, Id: SlotIdentity, D: Device<B>, const B: usize> {
    storage: Prepared<&'static PortStorage<D, B>>,
    commands: Queue<driver::Command<'id, Id, D, B>, COMMANDS>,
    events: Queue<owned::OwnedEvent<'id, Id, D, B>, { TERMINALS + TX_TICKETS }>,
    rx_out: Cell<usize>,
    recycled: Queue<ReceiveBuffer<'id, Id, D::Receive>, TX_TICKETS>,
    failed_command: RefCell<Option<driver::Command<'id, Id, D, B>>>,
    failed_terminal: RefCell<Option<owned::OwnedTerminal<'id, Id, D, B>>>,
    gate: RefCell<Prepared<GateIssuer<'id, Part<Id, 1>>>>,
    enabled: Cell<bool>,
    want_enabled: Cell<bool>,
    channel: PreparedCell<Channel>,
    edca: PreparedCell<[EdcaParameterSet; 2]>,
    dirty_edca: Cell<bool>,
    block_acks: RefCell<[Option<core_mac::RxBlockAckValues>; 8]>,
    dirty_ba: Cell<bool>,
    color: Cell<Option<BssColor>>,
    loss: Cell<bool>,
    #[cfg(oer_debug_control)]
    publication_hold: Cell<u8>,
    #[cfg(oer_debug_control)]
    completion_hold: Cell<bool>,
}
impl<'id, Id: SlotIdentity, D: Device<B>, const B: usize> Held<'id, Id, D, B> {
    const fn vacant() -> Self {
        Self {
            storage: Prepared::vacant(),
            commands: Queue::new(),
            events: Queue::new(),
            recycled: Queue::new(),
            rx_out: Cell::new(0),
            failed_command: RefCell::new(None),
            failed_terminal: RefCell::new(None),
            gate: RefCell::new(Prepared::vacant()),
            enabled: Cell::new(false),
            want_enabled: Cell::new(false),
            channel: PreparedCell::vacant(),
            edca: PreparedCell::vacant(),
            dirty_edca: Cell::new(false),
            block_acks: RefCell::new([None; 8]),
            dirty_ba: Cell::new(false),
            color: Cell::new(None),
            loss: Cell::new(false),
            #[cfg(oer_debug_control)]
            publication_hold: Cell::new(0),
            #[cfg(oer_debug_control)]
            completion_hold: Cell::new(false),
        }
    }
    fn prepare(
        &mut self,
        storage: &'static PortStorage<D, B>,
        issuer: GateIssuer<'id, Part<Id, 1>>,
        channel: Channel,
    ) {
        assert!(
            self.commands.empty() && self.events.empty() && self.recycled.empty(),
            "joined installation drained its rings"
        );
        assert!(
            self.failed_command.get_mut().is_none() && self.failed_terminal.get_mut().is_none(),
            "joined installation returned failed owners"
        );
        self.storage.prepare(storage);
        self.gate.get_mut().prepare(issuer);
        self.enabled.set(false);
        self.want_enabled.set(false);
        self.rx_out.set(0);
        self.channel.set(channel);
        self.edca.set([EdcaParameterSet::STANDARD_DEFAULT; 2]);
        self.dirty_edca.set(true);
        *self.block_acks.get_mut() = [None; 8];
        self.dirty_ba.set(false);
        self.color.set(None);
        self.loss.set(false);
        #[cfg(oer_debug_control)]
        {
            self.publication_hold.set(0);
            self.completion_hold.set(false);
        }
    }
    fn command(&self, command: driver::Command<'id, Id, D, B>) {
        if let Err(command) = self.commands.push(command) {
            *self.failed_command.borrow_mut() = Some(command);
            self.storage.poison(Fault::Capacity);
        }
        self.storage.driver.wake();
    }
    fn terminal(&self, terminal: owned::OwnedTerminal<'id, Id, D, B>) {
        if let Err(event) = self.events.push(owned::OwnedEvent::Terminal(terminal)) {
            if let owned::OwnedEvent::Terminal(terminal) = event {
                *self.failed_terminal.borrow_mut() = Some(terminal);
            }
            self.storage.poison(Fault::Capacity);
        } else {
            #[cfg(all(oer_trace, oer_debug_control))]
            oer_trace::emit(&trace::TerminalPosted);
        }
        self.storage.body.wake();
    }
}

pub struct ReceiveBuffer<'id, Id: SlotIdentity, R> {
    brand: PhantomData<fn(&'id (), Id) -> (&'id (), Id)>,
    unit: Option<R>,
    range: core::ops::Range<usize>,
    poison: &'static oer_memory::OnceValue<Fault>,
    driver: &'static AtomicWaker,
}
impl<Id: SlotIdentity, R: ReceiveUnit> RxBuffer for ReceiveBuffer<'_, Id, R> {
    fn bytes(&self) -> &[u8] {
        &self
            .unit
            .as_ref()
            .expect("received owner holds its allocation")
            .bytes()[self.range.clone()]
    }
}
impl<Id: SlotIdentity, R> Drop for ReceiveBuffer<'_, Id, R> {
    fn drop(&mut self) {
        if let Some(unit) = self.unit.take() {
            // Losing an affine receive owner quarantines its original DMA
            // lease; Drop cannot authorize rearming that allocation.
            core::mem::forget(unit);
            let _ = self.poison.set(Fault::Abandoned);
            self.driver.wake();
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Ack {
    pub snr_db: Option<i8>,
}

impl<Id: SlotIdentity, D: Device<B>, const B: usize> LowerMacBackend<Id>
    for Esp32s31Backend<Id, D, B>
{
    type Caps = Esp32s31Caps<B>;
    const TX_QUEUE_DEPTH: core::num::NonZeroU8 =
        core::num::NonZeroU8::new(8).expect("eight admission tickets");
    const PUBLICATION_BOUND: Duration = PUBLICATION_BOUND;
    const GROUP_COMMAND_BOUND: Duration = COMMAND_BOUND;
}
