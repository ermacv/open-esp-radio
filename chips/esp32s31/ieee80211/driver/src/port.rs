//! The S31 lower-MAC port. Only the driver loan executes hardware operations.
//!
//! The image polls `run_port` on P1 and retains its driver registration on
//! P2. P3 owns the interrupt epoch and posts
//! static typed signals through [`crate::irq`]; it never accesses this state.
//! All DMA allocations and the device remain in static storage through an
//! abnormal exit. Neither a dropped future nor a forgotten port frees them.

use crate::mac::{
    rx::{NormalizedRxFrame, RxError},
    tx::{LegacyTxQueue, TxSlot, TxSlotAccess, TxSlotFree},
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
use oer_ieee80211_contracts_port::association;
use oer_ieee80211_contracts_port::*;
use oer_ieee80211_contracts_security::Ccmp128Key;
use oer_ieee80211_wire::{address::IndividualAddress, ccmp::GroupKeyId};
use oer_radio_port::{self as radio, Child, Root, Slot};
use oer_radio_port::{BodyLoan, ClockError, LoanedContext, SlotBodyOwner, SlotIdentity, SlotOwner};
use oer_time::{Clock, Duration, Instant, Timer};

mod atomic_words;
use atomic_words::KeyActivity;
mod buffer;
#[cfg(target_arch = "riscv32")]
pub mod device;
mod driver;
mod keys;
mod lent;
pub use lent::LentReceive;
mod lifecycle;
mod registration;
pub use registration::{Registration, RegistrationError};
mod phase;
#[cfg(any(target_arch = "riscv32", test))]
mod receive;
mod settings;
use oer_sync::PriorityWaker;
#[cfg(all(oer_trace, oer_debug_control))]
pub mod trace;
pub use buffer::*;
pub use keys::{GroupKey, KeyIdentity, PairwiseKey, PortKeys};
mod loan;
mod owned;
pub use crate::lower_mac::{ContentionDraw, Observations};
pub use phase::{Esp32s31Interface, Esp32s31Phase, Esp32s31Setup};
pub use settings::SettingFault;

/// Shared admission capacity across the four access categories.
pub const TX_TICKETS: usize = 8;

// A blocked RX handoff needs P1 to return a lease. Its retry timer must not
// keep P2 polling an already reached deadline while that consumer cannot run.
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
/// Response/retirement containment budget. It covers one P2 cleanup step and
/// ready P2 work within the existing command bound; a later answer fails closed.
pub const SHUTDOWN_BOUND: Duration = COMMAND_BOUND;
/// Fresh-clock response budget, independent of the command FIFO. It covers
/// one P2 reading and its ready peer tasks; expiry is transient Unavailable.
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
    fn bytes(&self) -> &[u8];
    /// [`Self::bytes`], then the rest of the unit's allocation; every byte
    /// is initialized.
    fn storage(&self) -> &[u8];
    /// [`Self::storage`], mutably, for a network owner that rewrites it.
    fn storage_mut(&mut self) -> &mut [u8];
}

/// One bounded P2 hardware step. Implementations retain all progress in
/// themselves across `Pending`; no ISR may call any of these operations.
/// The device owns the cooperative registers, RX ring and PHY membership.
pub trait Device<const B: usize>: Sized + 'static {
    /// Image-selected concrete coexistence policy, statically dispatched on P2.
    type Coex: RadioCoexPriorities + 'static;
    type Power: WifiTxPowerProfile;
    type Entropy: WifiTxEntropy;
    type Timer: Timer + Clone + 'static;
    type Hardware: LowerMacHardware;
    type Receive: ReceiveUnit + Send;
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
    /// The network dropped a lent unit: rearm what the ring got back.
    fn returned(&mut self);
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
    /// The radio front end did not retune the stopped port for this start.
    Retune(WifiRetuneError),
    /// The device lost its registers to an earlier failure.
    Abandoned,
    /// The lower MAC completed an aggregate the ordinary port never
    /// publishes; its queue and ticket stay quarantined.
    UnexpectedAggregate,
    /// The core asked for a retune right after recording the channel it was
    /// tuned to.
    TuneNotRecorded,
    /// A P1 setting could not be published or read back.
    Setting(settings::SettingFault),
    /// The device's registers were lent to an actor transaction or retired.
    HardwareUnavailable,
}

/// Why the radio front end did not retune a stopped Wi-Fi client.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WifiRetuneError {
    /// The MAC or its RX walker was still active; nothing was posted.
    NotStopped(oer_esp32s31_hal::owner::maintenance::Error),
    /// The radio system answered the retune with this failure.
    Radio(oer_esp32s31_radio_client::Failure),
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

/// P2-only backing borrowing the retained device and cross-context storage.
/// Only the P2 registration can reach its device `RefCell`.
pub struct PortStorage<D: 'static, const B: usize> {
    device: RefCell<&'static mut D>,
    shared: &'static mut PortSharedStorage<B>,
    identity: Cell<u64>,
    expired: Cell<u32>,
    /// The station and AP tables the driver last installed in its policy.
    #[cfg(test)]
    installed_edca: [Cell<Option<EdcaParameterSet>>; 2],
}

/// Caller-placed cross-context state, with no device, Cell, RefCell or shared
/// producer. Place once before constructing `PortStorage`; quarantine keeps
/// its DMA return places and key activity unavailable for reuse.
pub struct PortSharedStorage<const B: usize> {
    poisoned: oer_memory::OnceValue<Fault>,
    // P2 can preempt a P1 notification. The P1 body consumer cannot
    // preempt its P2 producer, so its AtomicWaker needs no second slot.
    driver: PriorityWaker,
    body: AtomicWaker,
    key_live: [KeyActivity; 20],
    returned: oer_memory::ReturnBank<TxSlotFree<'static, B>, TX_TICKETS>,
    /// The receive units lent to the network.
    network: lent::NetworkCredit,
    settings: settings::Settings,
    /// Slot and permission of a GTK command the installation quarantined.
    #[cfg(test)]
    retained_group: oer_memory::OnceValue<(u8, u8)>,
    /// Previous and attempted KeyID of a quarantined GTK replacement, and
    /// whether its rollback restored the previous entry.
    #[cfg(test)]
    rolled_back: oer_memory::OnceValue<(u8, u8, bool)>,
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
    pub const fn new(device: &'static mut D, shared: &'static mut PortSharedStorage<B>) -> Self {
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
impl<const B: usize> PortSharedStorage<B> {
    // Keep the waker's typed scope at the indirect call, independently of
    // the large terminal dispatchers that notify the P1 body.
    #[inline(never)]
    fn wake_body(&self) {
        self.body.wake();
    }
    /// Vacant cross-context backing, placed separately from the P2-only device.
    pub const fn new() -> Self {
        Self {
            poisoned: oer_memory::OnceValue::new(),
            driver: PriorityWaker::new(),
            body: AtomicWaker::new(),
            key_live: [const { KeyActivity::new() }; 20],
            returned: oer_memory::ReturnBank::new(),
            network: lent::NetworkCredit::new(),
            settings: settings::Settings::new(),
            #[cfg(test)]
            retained_group: oer_memory::OnceValue::new(),
            #[cfg(test)]
            rolled_back: oer_memory::OnceValue::new(),
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

#[cfg(test)]
std::thread_local! {
    /// The cause the test thread's port published first: a frontend that
    /// sees its quarantine first cancels the body that would read it.
    pub(crate) static FIRST_POISON: core::cell::Cell<Option<Fault>> =
        const { core::cell::Cell::new(None) };
}

impl<const B: usize> Default for PortSharedStorage<B> {
    fn default() -> Self {
        Self::new()
    }
}

/// The complete construction resources; no DMA storage lives inline here.
pub struct PortParts<D: Device<B>, const B: usize, O: Observations> {
    pub storage: &'static mut PortStorage<D, B>,
    pub transmitter: OrdinaryTxIdleOwner<'static, D::Power, D::Entropy, D::Timer, B>,
    pub buffers: [Pin<&'static mut TxSlot<B>>; TX_TICKETS],
    pub config: LowerMacConfig,
    pub access_point_address: IndividualAddress,
    pub coex: &'static D::Coex,
    /// The P2 driver's receiver of its own publication observations; `()`
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
    observations: O,
    config: LowerMacConfig,
    coex: &'static D::Coex,
}
/// What a stopped port reinstalls with: the disabled core's transmitter and
/// the driver's observation receiver.
type Reusable<D, const B: usize, O: Observations> = (
    OrdinaryTxIdleOwner<
        'static,
        <D as Device<B>>::Power,
        <D as Device<B>>::Entropy,
        <D as Device<B>>::Timer,
        B,
    >,
    O,
);
enum BackendState<D: Device<B>, const B: usize, O: Observations> {
    Initial(Option<Initial<D, B, O>>),
    Running,
}
impl<Id: SlotIdentity, D: Device<B>, const B: usize, O: Observations> Esp32s31Backend<Id, D, B, O> {
    /// Optional aggregate admission of this port implementation. Its
    /// `NoAggregate` owner is uninhabited and it implements no `AmpduPhase`.
    pub const AMPDU: Option<AmpduCapabilities> = None;

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
            || parts
                .buffers
                .iter()
                .any(|slot| slot.state() != crate::mac::tx::TxDmaState::Free)
            || parts.storage.shared.poisoned.get().is_some()
        {
            return Err(parts);
        }
        let timer = parts.storage.device.get_mut().timer().clone();
        let PortParts {
            storage,
            transmitter,
            buffers,
            config,
            access_point_address,
            coex,
            observations,
        } = parts;
        // Every slot was checked free above. A slot that is not, or a place
        // the bank already holds, poisons this installation: the allocation
        // stays out of the bank and enable refuses with the fault.
        for (index, slot) in buffers.into_iter().enumerate() {
            let seeded = match slot.access() {
                TxSlotAccess::Free(memory) => storage.shared.returned.seed(index, memory).is_ok(),
                _ => false,
            };
            if !seeded {
                storage.shared.poison(Fault::Capacity);
            }
        }
        let channel = Channel::from_wifi_channel(config.channel);
        Ok(Self {
            storage,
            state: BackendState::Initial(Some(Initial {
                transmitter,
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
            || !self.storage.shared.returned.all_returned()
            || tsf_epoch == self.config.tsf_epoch
        {
            return Err(self);
        }
        let Some((transmitter, observations)) = self.reusable.take() else {
            return Err(self);
        };
        self.config.tsf_epoch = tsf_epoch;
        let channel = Channel::from_wifi_channel(self.config.channel);
        let timer = self.timer.clone();
        Ok(Esp32s31Backend {
            storage: self.storage,
            state: BackendState::Initial(Some(Initial {
                transmitter,
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
    unit: Option<R>,
    range: core::ops::Range<usize>,
    poison: &'static oer_memory::OnceValue<Fault>,
    driver: &'static PriorityWaker,
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
