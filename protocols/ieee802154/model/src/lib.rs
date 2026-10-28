#![no_std]
#![forbid(unsafe_code)]
#![deny(missing_docs)]
#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::todo,
    clippy::unimplemented,
    clippy::unreachable,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    clippy::disallowed_macros,
    clippy::disallowed_methods
)]
#![cfg_attr(
    test,
    allow(
        clippy::disallowed_macros,
        clippy::disallowed_methods,
        clippy::arithmetic_side_effects,
        clippy::unreachable
    )
)]

//! An in-memory IEEE 802.15.4 backend on the owner shape ([`oer_ieee802154_port::owner`]),
//! for deterministic host tests and simulation.
//!
//! [`OwnedModel::new`] returns the backend, which a test hands to
//! [`run_port`](oer_radio_port::run_port), and a [`ModelHandle`] that plays
//! the air and the world: it ends operations and receive windows, delivers
//! received frames, refuses the next enable or poisons the backend. Its held
//! part of the install keeps the operation permit in a [`ModelPermitPool`],
//! a first-in, first-out pool, and the body's resources give each
//! interface its seat in the pool's line.
//!
//! Its tokens have no value outside it: no caller mints a configured
//! interface or a pending entry (their fields are private).

extern crate alloc;
#[cfg(test)]
extern crate std;

use alloc::{
    collections::{BTreeMap, VecDeque},
    rc::Rc,
    vec::Vec,
};
use core::{
    future::{Future, poll_fn, ready},
    marker::PhantomData,
    task::{Poll, Waker},
};

use oer_radio_port::{
    BodyLoan, Call, CancelError, ClockError, EnableResult, InstallParts, ModelSlotOwner,
    PhaseParts, PortBackend, PortPhase, RadioPortClient, RadioWork, SlotBodyOwner, SlotIdentity,
    SlotOwner, SlotRetire, SlotStopped, SlotWakes, Work,
};
use oer_radio_port::{Child, EventsLost, Mint, Poisoned, PortResult, Root, Seat, Slot};

use oer_ieee802154_wire::{Channel, Frame, FrameError};

mod capacity;
mod storage;
pub use capacity::ModelPermitPool;
use storage::Local;

use oer_ieee802154_port::{
    CcaMode, CcaThresholdCapability, CslCapability, CslReceiver, EnhancedAckGeneration, FcsStatus,
    FrameCounterUpdate, FramePending, Ieee802154Capabilities, Ieee802154Instant, Ieee802154Radio,
    Interface, ProbingInitiator, RadioCapabilities, ReceivedFrame, RequestId, RxMetadata,
    SecurityStatus, SentAcknowledgement, TransmitPowerCapability, TransmitPowerLevels, TxStatus,
    owner::{
        AutomaticAcknowledgement, ClearChannelAssessment, ConfiguredInterface, ExtendedPending,
        Has, Ieee802154Backend, Ieee802154Caps, Ieee802154Client, Ieee802154Clients,
        Ieee802154Control, Ieee802154Events, Ieee802154Interfaces, Ieee802154Setup, InterfaceKind,
        InterfaceResources, Operation, OperationPermit, OperationPermits, PhaseEvent, Promiscuous,
        RadioSettings, RecentRssi, ShortPending, SourceMatch, SupportedCca, SupportedCcaThreshold,
        SupportedEnergyScan, SupportedHeaderIes, SupportedPower, SupportedScheduledReceive,
        SupportedTransmit,
    },
    pending::AutoPendingMode,
};

/// Interfaces of the model: two PANs.
pub const MODEL_INTERFACES: usize = 2;
/// Pending-table slots of each address size and interface.
pub const MODEL_PENDING_SLOTS: usize = 2;
/// Received frames the model queues before it reports a loss.
pub const MODEL_RECEIVED_FRAMES: usize = 4;

const MODEL_LEVELS: TransmitPowerLevels = match TransmitPowerLevels::new(&[-24, -12, 0, 12, 20]) {
    Ok(levels) => levels,
    Err(_) => panic!("ascending levels"),
};

/// The model's capabilities marker: every optional operation, two
/// interfaces.
pub struct ModelCaps;

impl Ieee802154Caps for ModelCaps {
    const CAPS: Ieee802154Capabilities = Ieee802154Capabilities {
        operations: RadioCapabilities::CLEAR_CHANNEL_ASSESSMENT
            .union(RadioCapabilities::CSMA_CA)
            .union(RadioCapabilities::ENERGY_SCAN)
            .union(RadioCapabilities::HARDWARE_ACKNOWLEDGEMENT)
            .union(RadioCapabilities::SCHEDULED_TRANSMIT)
            .union(RadioCapabilities::PROMISCUOUS)
            .union(RadioCapabilities::RECEIVE_TIMESTAMP)
            .union(RadioCapabilities::AUTOMATIC_ACKNOWLEDGEMENT)
            .union(RadioCapabilities::SECURITY_OFFLOAD)
            .union(RadioCapabilities::SOURCE_MATCH)
            .union(RadioCapabilities::TRANSMIT_RETRIES)
            .union(RadioCapabilities::SCHEDULED_RECEIVE)
            .union(RadioCapabilities::MULTI_PAN)
            .union(RadioCapabilities::TIME_SYNC),
        interfaces: MODEL_INTERFACES as u8,
        transmit_power: Some(TransmitPowerCapability {
            levels: MODEL_LEVELS,
            reset_dbm: 12,
        }),
        cca_threshold: Some(CcaThresholdCapability {
            lowest_dbm: -100,
            highest_dbm: -40,
            reset_dbm: -75,
        }),
        receive_sensitivity_dbm: -100,
        csl: Some(CslCapability {
            accuracy_ppm: 20,
            uncertainty_10us: 7,
        }),
        enhanced_ack_header_ies: 16,
    };
}

/// Why the model's radio refused an `Enable`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ModelRefusal;

/// Why the model is poisoned.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ModelFault {
    /// A fault injected by the external world or abandonment cleanup.
    Injected,
    /// No fresh request identity remains; the last one is retained.
    RequestIdExhausted {
        /// Last identity issued.
        last: u32,
    },
    /// A terminal could not release its admitted capacity.
    Completion(oer_ieee802154_port::owner::CompletionError),
    /// Accounting refused admission; the request was not published.
    Admission {
        /// Refused interface.
        interface: Interface,
        /// Refused identity.
        id: RequestId,
    },
    /// The disable sink returned this terminal; it remains queued.
    RejectedTerminal {
        /// Terminal's interface.
        interface: Interface,
        /// Terminal's identity.
        id: RequestId,
    },
}

/// External model input cannot be represented without changing its meaning.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ModelInputError {
    /// This radio has no such interface.
    Interface(Interface),
    /// Invalid MAC byte count; no frame or RSSI was published.
    Frame(FrameError),
}

// Runtime inputs are checked; generic port calls select their interface at
// monomorphization. No runtime array offset or default interface is used.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ModelInterface {
    Zero,
    One,
}
impl ModelInterface {
    fn checked(interface: Interface) -> Result<Self, ModelInputError> {
        match interface.index() {
            0 => Ok(Self::Zero),
            1 => Ok(Self::One),
            _ => Err(ModelInputError::Interface(interface)),
        }
    }
    const fn of<const I: u8>() -> Self {
        const {
            match I {
                0 => Self::Zero,
                1 => Self::One,
                _ => panic!("the model has only interfaces 0 and 1"),
            }
        }
    }
    fn get(self, interfaces: &[InterfaceState; MODEL_INTERFACES]) -> &InterfaceState {
        let [zero, one] = interfaces;
        match self {
            Self::Zero => zero,
            Self::One => one,
        }
    }
    fn get_mut(self, interfaces: &mut [InterfaceState; MODEL_INTERFACES]) -> &mut InterfaceState {
        let [zero, one] = interfaces;
        match self {
            Self::Zero => zero,
            Self::One => one,
        }
    }
}

/// What a terminal of the model ends.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Kind {
    Transmit,
    EnergyScan,
    Cca,
}

/// An owned event of the model.
#[derive(Debug)]
pub enum ModelEvent {
    /// A received frame.
    Received(Frame, RxMetadata),
    /// A transmission ended.
    TransmitDone(Interface, RequestId, TxStatus),
    /// An energy scan ended.
    EnergyScanDone(Interface, RequestId, Option<i8>),
    /// A clear-channel assessment ended.
    CcaDone(Interface, RequestId, Option<bool>),
}

impl ModelEvent {
    fn ends(&self) -> Option<(Interface, RequestId)> {
        match *self {
            Self::Received(..) => None,
            Self::TransmitDone(interface, id, _)
            | Self::EnergyScanDone(interface, id, _)
            | Self::CcaDone(interface, id, _) => Some((interface, id)),
        }
    }
}

/// What one interface asks of the radio's receive state.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Wish {
    /// Sleep.
    #[default]
    Sleep,
    /// Receive on a channel.
    Receive(Channel),
    /// A scheduled receive window on a channel.
    Window(Channel),
    /// Waiting to receive on a channel another interface does not receive
    /// on; meanwhile the interface does not receive.
    Waiting(Channel),
}

/// One interface's state.
#[derive(Clone, Debug, Default)]
pub struct InterfaceState {
    /// Configured, by the setup or the control.
    pub configured: bool,
    /// Its PAN identifier.
    pub pan_id: Option<u16>,
    /// Its short address.
    pub short_address: Option<u16>,
    /// Its extended address.
    pub extended_address: Option<[u8; 8]>,
    /// Its MAC key index, when it has keys.
    pub key_id: Option<u8>,
    /// Its next frame counter, when it has keys.
    pub frame_counter: Option<u32>,
    /// How its acknowledgements set frame pending.
    pub pending_mode: Option<AutoPendingMode>,
    /// The short addresses with pending data, by slot.
    pub pending_short: BTreeMap<u8, [u8; 2]>,
    /// The extended addresses with pending data, by slot.
    pub pending_extended: BTreeMap<u8, [u8; 8]>,
    /// What it asks of the receive state.
    pub wish: Wish,
}

/// The radio-wide settings the model holds.
#[derive(Clone, Debug, Default)]
pub struct RadioWide {
    /// Every channel's transmit power in dBm.
    pub transmit_power_dbm: Option<i8>,
    /// Per-channel transmit power overrides in dBm.
    pub channel_power_dbm: Vec<(Channel, i8)>,
    /// The CCA threshold in dBm.
    pub cca_threshold_dbm: Option<i8>,
    /// The CCA mode.
    pub cca_mode: Option<CcaMode>,
    /// Promiscuous reception.
    pub promiscuous: bool,
    /// Automatic acknowledgement.
    pub automatic_acknowledgement: bool,
    /// PAN coordinator.
    pub pan_coordinator: bool,
    /// The CSL receiver.
    pub csl: CslReceiver,
    /// The enhanced-ACK generator.
    pub enhanced_ack: Option<EnhancedAckGeneration>,
    /// The enhanced-ACK header IEs.
    pub header_ies: Vec<u8>,
    /// The probing initiators.
    pub probing: Vec<ProbingInitiator>,
}

#[derive(Default)]
struct State {
    enabled: bool,
    refuse_enable: bool,
    poisoned: Option<ModelFault>,
    abandoned: bool,
    interfaces: [InterfaceState; MODEL_INTERFACES],
    radio: RadioWide,
    operation: Option<(Interface, RequestId, Kind)>,
    tx_security: Option<oer_ieee802154_port::TxSecurity>,
    events: VecDeque<Result<ModelEvent, EventsLost>>,
    /// A loss marker is queued and not yet taken.
    lost: bool,
    next_id: u32,
    now_micros: u64,
    rssi_dbm: i8,
    /// The interface whose scheduled window is open now.
    open_window: Option<ModelInterface>,
    consumer: Option<Rc<Waker>>,
    receivers: Vec<Rc<Waker>>,
    wakes: Vec<Rc<Waker>>,
    retired: Vec<Rc<Waker>>,
}

impl State {
    fn wake_consumer(&mut self) {
        if let Some(waker) = self.consumer.take() {
            self.wakes.push(waker);
        }
    }

    fn wake_receivers(&mut self) {
        self.wakes.append(&mut self.receivers);
    }

    fn clear_wakers(&mut self) {
        self.retired.extend(self.consumer.take());
        self.retired.append(&mut self.receivers);
    }

    fn register_consumer(&mut self, waker: Rc<Waker>) {
        self.retired.extend(self.consumer.replace(waker));
    }

    fn push_terminal(&mut self, event: ModelEvent) {
        self.events.push_back(Ok(event));
        self.wake_consumer();
    }

    /// End interface `index`'s window, if it has one: it sleeps, and every
    /// waiter is told.
    fn end_window(&mut self, index: ModelInterface) {
        if let Wish::Window(_) = index.get(&self.interfaces).wish {
            if self.open_window == Some(index) {
                self.open_window = None;
            }
            index.get_mut(&mut self.interfaces).wish = Wish::Sleep;
            self.wake_receivers();
        }
    }

    /// End the operation in flight, if any, with its failed terminal.
    fn abort_operation(&mut self) {
        if let Some((interface, id, kind)) = self.operation.take() {
            self.push_terminal(match kind {
                Kind::Transmit => ModelEvent::TransmitDone(interface, id, TxStatus::Aborted),
                Kind::EnergyScan => ModelEvent::EnergyScanDone(interface, id, None),
                Kind::Cca => ModelEvent::CcaDone(interface, id, None),
            });
        }
    }

    /// The channel the radio receives on: an open window's, which
    /// preempts, or else the one every receiving interface shares.
    fn receiving(&self) -> Option<Channel> {
        if let Some(index) = self.open_window
            && let Wish::Window(channel) = index.get(&self.interfaces).wish
        {
            return Some(channel);
        }
        self.receive_channel(None)
    }

    /// The channel the interfaces other than `except` ask to receive on.
    fn receive_channel(&self, except: Option<ModelInterface>) -> Option<Channel> {
        [ModelInterface::Zero, ModelInterface::One]
            .into_iter()
            .filter(|index| Some(*index) != except)
            .find_map(|index| match index.get(&self.interfaces).wish {
                Wish::Receive(channel) => Some(channel),
                Wish::Window(..) | Wish::Waiting(_) | Wish::Sleep => None,
            })
    }

    fn mint_id(&mut self) -> Result<RequestId, ModelFault> {
        let next = self
            .next_id
            .checked_add(1)
            .ok_or(ModelFault::RequestIdExhausted { last: self.next_id })?;
        self.next_id = next;
        Ok(RequestId::new(next))
    }
}

#[derive(Default)]
struct ModelState(Local<State>);
impl ModelState {
    // All callers are internal operations: only plain values and Rc owners
    // move while State is out. User Waker code runs after publishing State.
    fn with<R>(&self, operation: impl FnOnce(&mut State) -> R) -> R {
        let (result, wakes, retired) = self.0.with(|state| {
            let result = operation(state);
            (
                result,
                core::mem::take(&mut state.wakes),
                core::mem::take(&mut state.retired),
            )
        });
        drop(retired);
        for waker in wakes {
            waker.wake_by_ref();
        }
        result
    }
}
type Shared = Rc<ModelState>;

/// The world's side of an [`OwnedModel`].
#[derive(Clone)]
pub struct ModelHandle(Shared);

impl ModelHandle {
    /// End the transmission `id`; `false` when it is not in flight.
    pub fn complete_transmit(&self, id: RequestId, status: TxStatus) -> bool {
        self.complete(id, Kind::Transmit, |interface| {
            ModelEvent::TransmitDone(interface, id, status)
        })
    }
    /// End the energy scan, with or without a measurement.
    pub fn complete_energy_scan(&self, id: RequestId, energy_dbm: Option<i8>) -> bool {
        self.complete(id, Kind::EnergyScan, |interface| {
            ModelEvent::EnergyScanDone(interface, id, energy_dbm)
        })
    }
    /// End the assessment, with or without an idle result.
    pub fn complete_cca(&self, id: RequestId, idle: Option<bool>) -> bool {
        self.complete(id, Kind::Cca, |interface| {
            ModelEvent::CcaDone(interface, id, idle)
        })
    }
    fn complete(
        &self,
        id: RequestId,
        kind: Kind,
        terminal: impl FnOnce(Interface) -> ModelEvent,
    ) -> bool {
        self.0.with(|state| match state.operation {
            Some((interface, running, running_kind)) if running == id && running_kind == kind => {
                state.operation = None;
                state.push_terminal(terminal(interface));
                true
            }
            _ => false,
        })
    }
    /// Open the interface's scheduled window; `false` if it has none.
    /// An interface outside this model is refused without changing state.
    pub fn open_window(&self, interface: Interface) -> Result<bool, ModelInputError> {
        let index = ModelInterface::checked(interface)?;
        Ok(self.0.with(|state| {
            let scheduled = matches!(index.get(&state.interfaces).wish, Wish::Window(..));
            if scheduled {
                state.open_window = Some(index);
            }
            scheduled
        }))
    }
    /// End the interface's scheduled window; `false` if it has none.
    /// An interface outside this model is refused without changing state.
    pub fn end_window(&self, interface: Interface) -> Result<bool, ModelInputError> {
        let index = ModelInterface::checked(interface)?;
        Ok(self.0.with(|state| {
            let open = matches!(index.get(&state.interfaces).wish, Wish::Window(..));
            state.end_window(index);
            open
        }))
    }
    /// Inject a received MAC frame. Returns `false` if the radio is disabled,
    /// poisoned or receiving elsewhere. A full RX queue emits one loss marker.
    /// Invalid interfaces and MAC lengths return a typed error before mutation,
    /// including when the radio is disabled or its RX queue is full.
    pub fn receive(
        &self,
        channel: Channel,
        interface: Interface,
        bytes: &[u8],
        rssi_dbm: i8,
    ) -> Result<bool, ModelInputError> {
        ModelInterface::checked(interface)?;
        let frame = Frame::try_from_bytes(bytes).map_err(ModelInputError::Frame)?;
        Ok(self.0.with(|state| {
            if !state.enabled || state.poisoned.is_some() || state.receiving() != Some(channel) {
                return false;
            }
            state.rssi_dbm = rssi_dbm;
            let received = state
                .events
                .iter()
                .filter(|event| matches!(event, Ok(ModelEvent::Received(..))))
                .count();
            if received == MODEL_RECEIVED_FRAMES {
                if !state.lost {
                    state.lost = true;
                    state.events.push_back(Err(EventsLost));
                    state.wake_consumer();
                }
                return true;
            }
            let metadata = RxMetadata {
                channel,
                rssi_dbm,
                link_quality: 255,
                timestamp: Some(Ieee802154Instant::from_micros(state.now_micros)),
                fcs: FcsStatus::Valid,
                security: SecurityStatus::Unprocessed,
                frame_pending: FramePending::Clear,
                sent_acknowledgement: SentAcknowledgement::NONE,
                interface: Some(interface),
            };
            state
                .events
                .push_back(Ok(ModelEvent::Received(frame, metadata)));
            state.wake_consumer();
            true
        }))
    }
    /// Refuse enable as an unavailable radio until cleared by the world.
    pub fn refuse_enable(&self, refuse: bool) {
        self.0.with(|state| state.refuse_enable = refuse);
    }
    /// End in-flight work and windows, then inject a backend fault.
    pub fn poison(&self) {
        self.0.with(|state| {
            state.abort_operation();
            for index in [ModelInterface::Zero, ModelInterface::One] {
                state.end_window(index);
            }
            state.poisoned.get_or_insert(ModelFault::Injected);
            state.wake_consumer();
            state.wake_receivers();
        });
    }
    /// Set the externally supplied radio clock observation.
    pub fn set_now(&self, micros: u64) {
        self.0.with(|state| state.now_micros = micros);
    }
    /// The interface and identity of in-flight work.
    pub fn operation(&self) -> Option<(Interface, RequestId)> {
        self.0
            .with(|state| state.operation.map(|(interface, id, _)| (interface, id)))
    }
    /// The channel the radio receives on.
    pub fn receiving(&self) -> Option<Channel> {
        self.0.with(|state| state.receiving())
    }
    /// A snapshot of a valid interface; an unknown interface is an input error.
    pub fn interface(&self, interface: Interface) -> Result<InterfaceState, ModelInputError> {
        let index = ModelInterface::checked(interface)?;
        Ok(self.0.with(|state| index.get(&state.interfaces).clone()))
    }
    /// A snapshot of radio-wide settings.
    pub fn radio(&self) -> RadioWide {
        self.0.with(|state| state.radio.clone())
    }
    /// Whether the radio is enabled.
    pub fn enabled(&self) -> bool {
        self.0.with(|state| state.enabled)
    }
    /// Whether the owner abandoned the backend.
    pub fn abandoned(&self) -> bool {
        self.0.with(|state| state.abandoned)
    }
}

/// The model's IEEE 802.15.4 backend.
pub struct OwnedModel<Id: SlotIdentity> {
    shared: Shared,
    identity: PhantomData<fn(Id) -> Id>,
}

impl<Id: SlotIdentity> OwnedModel<Id> {
    /// A disabled, unconfigured P2 owner and its external model world.
    pub fn new() -> (Self, ModelHandle) {
        let shared = Shared::default();
        (
            Self {
                shared: shared.clone(),
                identity: PhantomData,
            },
            ModelHandle(shared),
        )
    }
    /// Resume stopped model hardware after its slot's ordinary join.
    pub fn resume(hardware: ModelVacant) -> Self {
        Self {
            shared: hardware.shared,
            identity: PhantomData,
        }
    }
}

/// The model's held part of the install: the operation permit.
pub struct ModelHeld<Id: SlotIdentity> {
    permits: OperationPermits<
        'static,
        ModelPermitPool<'static, Operation, Child<Root<Id>, 2>>,
        Child<Root<Id>, 2>,
    >,
}

type LentPermits<'p, Id> = oer_ieee802154_port::owner::LentWorkPermits<
    'p,
    Operation,
    ModelPermitPool<'static, Operation, Child<Root<Id>, 2>>,
    Child<Root<Id>, 2>,
    OwnedModel<Id>,
>;

/// The model's port-lifetime resources: each interface's slots, its seat
/// in the operation's line and the recent-RSSI reader.
pub struct ModelResources<'id, Id: SlotIdentity> {
    /// Each interface's seat in the operation's line, by interface.
    pub operation: [Seat<'id, Operation, Child<Root<Id>, 2>>; MODEL_INTERFACES],
    /// Interface 0's slots.
    pub interface0: InterfaceResources<'id, 0, MODEL_PENDING_SLOTS, Child<Root<Id>, 0>>,
    /// Interface 1's slots.
    pub interface1: InterfaceResources<'id, 1, MODEL_PENDING_SLOTS, Child<Root<Id>, 1>>,
    /// The recent-RSSI reader.
    pub rssi: ModelRssi,
}

/// The model's recent-RSSI reader.
pub struct ModelRssi(Shared);

impl RecentRssi for ModelRssi {
    fn recent_rssi(&self) -> i8 {
        self.0.with(|state| state.rssi_dbm)
    }
}

/// A configured interface `I` of the model; it holds the interface's slot.
pub struct ModelConfigured<'id, Id: SlotIdentity, const I: u8> {
    slot: Slot<'id, InterfaceKind<I>, Child<Child<Root<Id>, I>, 0>>,
}

impl<'id, Id: SlotIdentity, const I: u8> ConfiguredInterface<'id, I>
    for ModelConfigured<'id, Id, I>
{
}

/// The model's disabled owner's settings.
pub struct ModelSetup<'a, 'id, Id: SlotIdentity> {
    shared: &'a Shared,
    brand: PhantomData<fn(&'id (), Id) -> (&'id (), Id)>,
}

/// The model's phase control.
pub struct ModelControl<'a, 'id, Id: SlotIdentity> {
    shared: &'a Shared,
    brand: PhantomData<fn(&'id (), Id) -> (&'id (), Id)>,
}

fn settings(shared: &Shared, change: impl FnOnce(&mut RadioWide)) {
    shared.with(|state| change(&mut state.radio));
}

macro_rules! radio_settings {
    ($type:ty) => {
        impl<Id: SlotIdentity> RadioSettings for $type {
            type Caps = ModelCaps;

            fn set_transmit_power(&mut self, power: SupportedPower<ModelCaps>) {
                settings(self.shared, |radio| {
                    radio.transmit_power_dbm = Some(power.dbm())
                });
            }

            fn set_channel_power(&mut self, channel: Channel, power: SupportedPower<ModelCaps>) {
                settings(self.shared, |radio| {
                    radio.channel_power_dbm.retain(|(set, _)| *set != channel);
                    radio.channel_power_dbm.push((channel, power.dbm()));
                });
            }

            fn set_cca_threshold(&mut self, threshold: SupportedCcaThreshold<ModelCaps>) {
                settings(self.shared, |radio| {
                    radio.cca_threshold_dbm = Some(threshold.dbm())
                });
            }

            fn set_cca_mode(&mut self, _: Has<ModelCaps, ClearChannelAssessment>, mode: CcaMode) {
                settings(self.shared, |radio| radio.cca_mode = Some(mode));
            }

            fn set_promiscuous(&mut self, _: Has<ModelCaps, Promiscuous>, promiscuous: bool) {
                settings(self.shared, |radio| radio.promiscuous = promiscuous);
            }

            fn set_automatic_acknowledgement(
                &mut self,
                _: Has<ModelCaps, AutomaticAcknowledgement>,
                acknowledge: bool,
            ) {
                settings(self.shared, |radio| {
                    radio.automatic_acknowledgement = acknowledge
                });
            }

            fn set_pan_coordinator(&mut self, coordinator: bool) {
                settings(self.shared, |radio| radio.pan_coordinator = coordinator);
            }

            fn set_csl(&mut self, csl: CslReceiver) {
                settings(self.shared, |radio| radio.csl = csl);
            }

            fn set_enhanced_ack(&mut self, generation: Option<EnhancedAckGeneration>) {
                settings(self.shared, |radio| radio.enhanced_ack = generation);
            }

            fn set_enhanced_ack_header_ies(&mut self, ies: SupportedHeaderIes<'_, ModelCaps>) {
                settings(self.shared, |radio| radio.header_ies = ies.get().to_vec());
            }

            fn set_enhanced_ack_probing(&mut self, initiators: &[ProbingInitiator]) {
                settings(self.shared, |radio| radio.probing = initiators.to_vec());
            }
        }
    };
}

radio_settings!(ModelSetup<'_, '_, Id>);
radio_settings!(ModelControl<'_, '_, Id>);

fn change_interface<const I: u8>(shared: &Shared, change: impl FnOnce(&mut InterfaceState)) {
    shared.with(|state| change(ModelInterface::of::<I>().get_mut(&mut state.interfaces)));
}

macro_rules! interfaces {
    ($type:ty) => {
        impl<'id, Id: SlotIdentity> Ieee802154Interfaces<'id> for $type {
            type InterfacePath<const I: u8> = Child<Root<Id>, I>;
            type Configured<const I: u8> = ModelConfigured<'id, Id, I>;

            fn configure<const I: u8>(
                &mut self,
                slot: Slot<'id, InterfaceKind<I>, Child<Child<Root<Id>, I>, 0>>,
            ) -> ModelConfigured<'id, Id, I> {
                change_interface::<I>(self.shared, |interface| interface.configured = true);
                ModelConfigured { slot }
            }

            fn remove<const I: u8>(
                &mut self,
                interface: ModelConfigured<'id, Id, I>,
            ) -> Slot<'id, InterfaceKind<I>, Child<Child<Root<Id>, I>, 0>> {
                self.shared.with(|state| {
                    let index = ModelInterface::of::<I>();
                    state.end_window(index);
                    let removed = index.get_mut(&mut state.interfaces);
                    // The pending entries remain installed under their affine tokens.
                    let pending_short = core::mem::take(&mut removed.pending_short);
                    let pending_extended = core::mem::take(&mut removed.pending_extended);
                    *removed = InterfaceState {
                        pending_short,
                        pending_extended,
                        ..InterfaceState::default()
                    };
                    state.wake_receivers();
                });
                interface.slot
            }
        }
    };
}

interfaces!(ModelSetup<'_, 'id, Id>);
interfaces!(ModelControl<'_, 'id, Id>);

impl<'id, Id: SlotIdentity> Ieee802154Setup<'id> for ModelSetup<'_, 'id, Id> {}

impl<'id, Id: SlotIdentity> Ieee802154Control<'id> for ModelControl<'_, 'id, Id> {}

/// The model's one events consumer.
pub struct ModelEvents<'a, 'id, Id: SlotIdentity> {
    shared: &'a Shared,
    held: &'a LentPermits<'id, Id>,
    end: &'a mut Option<oer_radio_port::EventEnd<ModelFault>>,
}

impl<Id: SlotIdentity> PortPhase for ModelEvents<'_, '_, Id> {
    type Event = ModelEvent;
    type Fault = ModelFault;

    fn next_event(
        &mut self,
    ) -> impl Future<
        Output = Result<Result<ModelEvent, EventsLost>, oer_radio_port::EventEnd<ModelFault>>,
    > + '_ {
        poll_fn(|context| {
            if let Some(end) = *self.end {
                return Poll::Ready(Err(end));
            }
            // Clone user wakers before taking State out. Rc moves/clones inside
            // the transaction never invoke the RawWaker callbacks.
            let waker = Rc::new(context.waker().clone());
            let next = self.shared.with(|state| {
                if let Some(event) = state.events.pop_front() {
                    if event.is_err() {
                        state.lost = false;
                    }
                    return Poll::Ready(Ok(event));
                }
                if let Some(cause) = state.poisoned {
                    return Poll::Ready(Err(oer_radio_port::EventEnd::Drained { cause }));
                }
                state.register_consumer(waker.clone());
                Poll::Pending
            });
            match next {
                Poll::Ready(Ok(Ok(event))) => {
                    if let Some((_, id)) = event.ends()
                        && let Err(error) = self.held.complete(id)
                    {
                        let cause = ModelFault::Completion(error);
                        self.shared.with(|state| {
                            state.events.push_front(Ok(event));
                            state.poisoned = Some(cause);
                        });
                        let end = oer_radio_port::EventEnd::Quarantined { cause };
                        *self.end = Some(end);
                        return Poll::Ready(Err(end));
                    }
                    Poll::Ready(Ok(Ok(event)))
                }
                Poll::Ready(Err(end)) => {
                    *self.end = Some(end);
                    Poll::Ready(Err(end))
                }
                other => other,
            }
        })
    }
}

impl<Id: SlotIdentity> Ieee802154Events for ModelEvents<'_, '_, Id> {
    fn view(event: &ModelEvent) -> PhaseEvent<'_> {
        match event {
            ModelEvent::Received(frame, metadata) => PhaseEvent::Received(ReceivedFrame {
                frame: frame.view(),
                metadata: *metadata,
            }),
            ModelEvent::TransmitDone(interface, id, status) => PhaseEvent::TransmitDone {
                interface: *interface,
                id: *id,
                status: *status,
                acknowledgement: None,
                security: None,
            },
            ModelEvent::EnergyScanDone(interface, id, Some(energy_dbm)) => {
                PhaseEvent::EnergyScanDone {
                    interface: *interface,
                    id: *id,
                    energy_dbm: *energy_dbm,
                }
            }
            ModelEvent::EnergyScanDone(interface, id, None) => PhaseEvent::EnergyScanFailed {
                interface: *interface,
                id: *id,
            },
            ModelEvent::CcaDone(interface, id, Some(idle)) => {
                PhaseEvent::ClearChannelAssessmentDone {
                    interface: *interface,
                    id: *id,
                    idle: *idle,
                }
            }
            ModelEvent::CcaDone(interface, id, None) => PhaseEvent::ClearChannelAssessmentFailed {
                interface: *interface,
                id: *id,
            },
        }
    }
}

/// The model's clients: one per configured interface.
pub struct ModelClients<'a, 'id, Id: SlotIdentity> {
    shared: &'a Shared,
    held: &'a LentPermits<'id, Id>,
    pool: &'a ModelPermitPool<'static, Operation, Child<Root<Id>, 2>>,
    loan: BodyLoan<'id, Id, OwnedModel<Id>>,
}

impl<'a, 'id, Id: SlotIdentity> Ieee802154Clients<'id> for ModelClients<'a, 'id, Id> {
    type Configured<const I: u8> = ModelConfigured<'id, Id, I>;
    type Client<'c, const I: u8>
        = ModelClient<'c, 'id, Id, I>
    where
        Self: 'c;

    fn client<'c, const I: u8>(
        &'c self,
        _: &'c mut ModelConfigured<'id, Id, I>,
    ) -> ModelClient<'c, 'id, Id, I> {
        ModelClient {
            shared: self.shared,
            held: self.held,
            pool: self.pool,
            loan: self.loan,
        }
    }
}

/// The work token of the model: the interface and identity of admitted
/// work.
#[derive(Debug)]
pub struct ModelToken {
    interface: Interface,
    id: RequestId,
}

impl RadioWork for ModelToken {
    type Id = RequestId;

    fn id(&self) -> RequestId {
        self.id
    }
}

/// An installed short-address pending entry of interface `I`.
pub struct ModelShortEntry<'id, Id: SlotIdentity, const I: u8> {
    slot: Slot<'id, ShortPending<I>, Child<Child<Root<Id>, I>, 1>>,
}

/// An installed extended-address pending entry of interface `I`.
pub struct ModelExtendedEntry<'id, Id: SlotIdentity, const I: u8> {
    slot: Slot<'id, ExtendedPending<I>, Child<Child<Root<Id>, I>, 2>>,
}

/// The client of the model's interface `I` in a running phase.
pub struct ModelClient<'c, 'id, Id: SlotIdentity, const I: u8> {
    shared: &'c Shared,
    held: &'c LentPermits<'id, Id>,
    pool: &'c ModelPermitPool<'static, Operation, Child<Root<Id>, 2>>,
    loan: BodyLoan<'id, Id, OwnedModel<Id>>,
}

impl<'c, 'id, Id: SlotIdentity, const I: u8> ModelClient<'c, 'id, Id, I> {
    /// Set this interface's wish: end its window, set the wish and wake
    /// every waiting receive, whose condition may now hold.
    fn set_wish(&self, state: &mut State, wish: Wish) {
        let index = ModelInterface::of::<I>();
        state.end_window(index);
        index.get_mut(&mut state.interfaces).wish = wish;
        state.wake_receivers();
    }

    fn poisoned(&self) -> Result<(), Poisoned<ModelFault>> {
        self.shared.with(|state| match state.poisoned {
            Some(cause) => Err(Poisoned { cause }),
            None => Ok(()),
        })
    }

    fn admit<'o>(
        &'o self,
        permit: OperationPermit<'o, 'id, Child<Root<Id>, 2>>,
        kind: Kind,
    ) -> Result<ModelToken, Poisoned<ModelFault>> {
        self.poisoned()?;
        let interface = Interface::new(I);
        let id = self
            .shared
            .with(|state| {
                state.mint_id().inspect_err(|cause| {
                    state.poisoned = Some(*cause);
                    state.wake_consumer();
                    state.wake_receivers();
                })
            })
            .map_err(|cause| Poisoned { cause })?;
        if let Err(refused) = self.held.admit(interface, id, permit) {
            let cause = ModelFault::Admission {
                interface: refused.interface,
                id: refused.id,
            };
            self.shared.with(|state| state.poisoned = Some(cause));
            return Err(Poisoned { cause });
        }
        self.shared
            .with(|state| state.operation = Some((interface, id, kind)));
        Ok(ModelToken { interface, id })
    }
}

impl<Id: SlotIdentity, const I: u8> RadioPortClient for ModelClient<'_, '_, Id, I> {
    type Token = ModelToken;
    type Domain = Ieee802154Radio;
    type Fault = ModelFault;

    fn now(
        &self,
    ) -> impl Future<Output = PortResult<Ieee802154Instant, ClockError, ModelFault>> + '_ {
        ready(self.shared.with(|state| match state.poisoned {
            Some(cause) => Err(Poisoned { cause }),
            None => Ok(Ok(Ieee802154Instant::from_micros(state.now_micros))),
        }))
    }

    fn end(
        &self,
        token: ModelToken,
    ) -> impl Future<Output = PortResult<(), CancelError, ModelFault>> + '_ {
        ready(self.shared.with(|state| {
            if let Some(cause) = state.poisoned {
                return Err(Poisoned { cause });
            }
            Ok(match state.operation {
                Some((interface, id, _)) if interface == token.interface && id == token.id => {
                    state.abort_operation();
                    Ok(())
                }
                _ => Err(CancelError::NotRunning),
            })
        }))
    }
}

impl<'c, 'id, Id: SlotIdentity, const I: u8> Ieee802154Client<'id, I>
    for ModelClient<'c, 'id, Id, I>
{
    type OperationPath = Child<Root<Id>, 2>;
    type Caps = ModelCaps;

    type InterfacePath = Child<Root<Id>, I>;
    type ShortEntry = ModelShortEntry<'id, Id, I>;
    type ExtendedEntry = ModelExtendedEntry<'id, Id, I>;

    fn set_pan_id(&self, pan_id: u16) {
        change_interface::<I>(self.shared, |interface| interface.pan_id = Some(pan_id));
    }

    fn set_short_address(&self, address: u16) {
        change_interface::<I>(self.shared, |interface| {
            interface.short_address = Some(address)
        });
    }

    fn set_extended_address(&self, address: [u8; 8]) {
        change_interface::<I>(self.shared, |interface| {
            interface.extended_address = Some(address)
        });
    }

    fn set_mac_keys(
        &self,
        key_id: u8,
        _: [u8; 16],
        _: [u8; 16],
        _: [u8; 16],
        frame_counter: Option<u32>,
    ) {
        change_interface::<I>(self.shared, |interface| {
            interface.key_id = Some(key_id);
            let counter = frame_counter.unwrap_or(interface.frame_counter.unwrap_or(0));
            interface.frame_counter = Some(counter);
        });
    }

    fn remove_mac_keys(&self) {
        change_interface::<I>(self.shared, |interface| {
            interface.key_id = None;
            interface.frame_counter = None;
        });
    }

    fn update_frame_counter(&self, update: FrameCounterUpdate) {
        change_interface::<I>(self.shared, |interface| {
            let counter = interface.frame_counter.get_or_insert(0);
            let FrameCounterUpdate::SetIfLarger(value) = update;
            *counter = (*counter).max(value);
        });
    }

    fn frame_counter(&self) -> Option<u32> {
        self.shared.with(|state| {
            let interface = ModelInterface::of::<I>().get(&state.interfaces);
            interface.key_id.and(interface.frame_counter)
        })
    }

    fn set_pending_mode(&self, _: Has<ModelCaps, SourceMatch>, mode: AutoPendingMode) {
        change_interface::<I>(self.shared, |interface| interface.pending_mode = Some(mode));
    }

    fn add_pending_short(
        &self,
        slot: Slot<'id, ShortPending<I>, Child<Child<Root<Id>, I>, 1>>,
        address: [u8; 2],
    ) -> ModelShortEntry<'id, Id, I> {
        change_interface::<I>(self.shared, |interface| {
            interface.pending_short.insert(slot.index(), address);
        });
        ModelShortEntry { slot }
    }

    fn add_pending_extended(
        &self,
        slot: Slot<'id, ExtendedPending<I>, Child<Child<Root<Id>, I>, 2>>,
        address: [u8; 8],
    ) -> ModelExtendedEntry<'id, Id, I> {
        change_interface::<I>(self.shared, |interface| {
            interface.pending_extended.insert(slot.index(), address);
        });
        ModelExtendedEntry { slot }
    }

    fn remove_pending_short(
        &self,
        entry: ModelShortEntry<'id, Id, I>,
    ) -> Slot<'id, ShortPending<I>, Child<Child<Root<Id>, I>, 1>> {
        change_interface::<I>(self.shared, |interface| {
            interface.pending_short.remove(&entry.slot.index());
        });
        entry.slot
    }

    fn remove_pending_extended(
        &self,
        entry: ModelExtendedEntry<'id, Id, I>,
    ) -> Slot<'id, ExtendedPending<I>, Child<Child<Root<Id>, I>, 2>> {
        change_interface::<I>(self.shared, |interface| {
            interface.pending_extended.remove(&entry.slot.index());
        });
        entry.slot
    }

    fn receive(&self, channel: Channel) -> impl Future<Output = Result<(), Poisoned<ModelFault>>> {
        poll_fn(move |context| {
            let waker = Rc::new(context.waker().clone());
            self.shared.with(|state| {
                if let Some(cause) = state.poisoned {
                    return Poll::Ready(Err(Poisoned { cause }));
                }
                let index = ModelInterface::of::<I>();
                if state
                    .receive_channel(Some(index))
                    .is_some_and(|on| on != channel)
                {
                    if index.get(&state.interfaces).wish != Wish::Waiting(channel) {
                        self.set_wish(state, Wish::Waiting(channel));
                    }
                    state.receivers.push(waker.clone());
                    return Poll::Pending;
                }
                self.set_wish(state, Wish::Receive(channel));
                Poll::Ready(Ok(()))
            })
        })
    }

    fn sleep(&self) {
        self.shared.with(|state| {
            let index = ModelInterface::of::<I>();
            state.end_window(index);
            index.get_mut(&mut state.interfaces).wish = Wish::Sleep;
            state.wake_receivers();
        });
    }

    fn scheduled_receive(&self, window: SupportedScheduledReceive<ModelCaps>) {
        self.shared
            .with(|state| self.set_wish(state, Wish::Window(window.get().0)));
    }

    fn window_ended(&self) -> impl Future<Output = Result<(), Poisoned<ModelFault>>> {
        poll_fn(move |context| {
            let waker = Rc::new(context.waker().clone());
            self.shared.with(|state| {
                if let Some(cause) = state.poisoned {
                    return Poll::Ready(Err(Poisoned { cause }));
                }
                if matches!(
                    ModelInterface::of::<I>().get(&state.interfaces).wish,
                    Wish::Window(_)
                ) {
                    state.receivers.push(waker.clone());
                    return Poll::Pending;
                }
                Poll::Ready(Ok(()))
            })
        })
    }

    fn operation<'o, 's>(
        &'o self,
        seat: &'s mut Seat<'id, Operation, Child<Root<Id>, 2>>,
    ) -> impl Future<Output = OperationPermit<'o, 'id, Child<Root<Id>, 2>>> + 's
    where
        'id: 'o,
        'o: 's,
    {
        let permits = self.held;
        let mut taking = alloc::boxed::Box::pin(self.pool.take_loaned(seat, self.loan));
        poll_fn(move |context| {
            taking
                .as_mut()
                .poll(context)
                .map(|permit| permits.guard(permit))
        })
    }

    fn transmit<'o>(
        &'o self,
        permit: OperationPermit<'o, 'id, Child<Root<Id>, 2>>,
        transmission: SupportedTransmit<'_, ModelCaps>,
    ) -> Result<Work<'o, Self>, Poisoned<ModelFault>>
    where
        'id: 'o,
    {
        let token = self.admit(permit, Kind::Transmit)?;
        self.shared
            .with(|state| state.tx_security = Some(transmission.get().security));
        Ok(Work::new(self, token))
    }

    fn energy_scan<'o>(
        &'o self,
        permit: OperationPermit<'o, 'id, Child<Root<Id>, 2>>,
        _: SupportedEnergyScan<ModelCaps>,
    ) -> Result<Work<'o, Self>, Poisoned<ModelFault>>
    where
        'id: 'o,
    {
        let token = self.admit(permit, Kind::EnergyScan)?;
        Ok(Work::new(self, token))
    }

    fn clear_channel_assessment<'o>(
        &'o self,
        permit: OperationPermit<'o, 'id, Child<Root<Id>, 2>>,
        _: SupportedCca<ModelCaps>,
    ) -> Result<Work<'o, Self>, Poisoned<ModelFault>>
    where
        'id: 'o,
    {
        let token = self.admit(permit, Kind::Cca)?;
        Ok(Work::new(self, token))
    }
}

/// Stopped model hardware retained by the caller slot between installations.
/// The observer handle cannot construct or clone this authority.
pub struct ModelVacant {
    shared: Shared,
}

/// Fixed local endpoint backing, including owned capacity and admitted permits.
pub struct ModelQueues<Id: SlotIdentity> {
    shared: Shared,
    held: ModelHeld<Id>,
    resources: Option<ModelResources<'static, Id>>,
}

/// Local frontend endpoints; no reference to the P2 backend owner.
pub struct ModelFrontendRaw<'s, Id: SlotIdentity> {
    shared: Shared,
    held: &'s ModelHeld<Id>,
    resources: &'s mut Option<ModelResources<'static, Id>>,
}

/// Body-loaned local model accounting. The owned map remains in slot backing.
pub struct ModelFrontend<'p, Id: SlotIdentity> {
    shared: Shared,
    held: LentPermits<'p, Id>,
    pool: &'p ModelPermitPool<'static, Operation, Child<Root<Id>, 2>>,
    loan: BodyLoan<'p, Id, OwnedModel<Id>>,
    end: Option<oer_radio_port::EventEnd<ModelFault>>,
}

impl<Id: SlotIdentity> SlotOwner<Id> for OwnedModel<Id> {
    type Pools = (
        oer_ieee802154_port::owner::InterfacePool<ModelCaps, 0, MODEL_PENDING_SLOTS>,
        oer_ieee802154_port::owner::InterfacePool<ModelCaps, 1, MODEL_PENDING_SLOTS>,
        oer_radio_port::Permits<Operation, 1, MODEL_INTERFACES>,
    );
    type Queues = Option<ModelQueues<Id>>;
    const VACANT_QUEUES: Self::Queues = None;
    type Vacant = ModelVacant;
    type Frontend<'s>
        = ModelFrontendRaw<'s, Id>
    where
        Self: 's,
        Id: 's;
    type Invocation<'s>
        = &'s ModelHeld<Id>
    where
        Self: 's,
        Id: 's;

    fn prepare_queues(&mut self, backing: &mut Self::Queues, mint: Mint<'static, Self::Pools, Id>) {
        // Programmed settings belong to the ended installation. Physical
        // world facts (clock, RSSI, injected refusal/fault) remain unchanged.
        self.shared.with(|state| {
            state.interfaces = core::array::from_fn(|_| InterfaceState::default());
            state.radio = RadioWide::default();
            state.events.clear();
            state.lost = false;
        });
        let (interface0, interface1, work) = mint.into_pools();
        let (permits, operation) =
            OperationPermits::new(work, |supply, _| ModelPermitPool::new(supply));
        *backing = Some(ModelQueues {
            shared: self.shared.clone(),
            held: ModelHeld { permits },
            resources: Some(ModelResources {
                operation,
                interface0,
                interface1,
                rssi: ModelRssi(self.shared.clone()),
            }),
        });
    }

    fn split<'s>(
        queues: &'s mut Self::Queues,
        _: SlotWakes<'s, Id>,
    ) -> Result<(Self::Frontend<'s>, Self::Invocation<'s>), oer_radio_port::SlotInitError>
    where
        Self: 's,
        Id: 's,
    {
        let queues = queues
            .as_mut()
            .ok_or(oer_radio_port::SlotInitError::QueuesUnprepared)?;
        Ok((
            ModelFrontendRaw {
                shared: queues.shared.clone(),
                held: &queues.held,
                resources: &mut queues.resources,
            },
            &queues.held,
        ))
    }

    async fn run<'s>(
        &mut self,
        held: &mut Self::Invocation<'s>,
        retire: SlotRetire<'_, Id, Self>,
    ) -> SlotStopped<Id, Self>
    where
        Self: 's,
        Id: 's,
    {
        retire.wait().await;
        if !matches!(held.permits.in_flight(), Ok(None)) {
            // An undelivered admitted owner cannot authorize a fresh mint.
            retire.request_failure(oer_radio_port::LifecycleError::StopUnconfirmed);
            core::future::pending::<()>().await;
        }
        end_world(&self.shared);
        retire.requested().await
    }

    fn finish<'s>(&mut self, _: &mut Self::Invocation<'s>) -> ModelVacant
    where
        Self: 's,
        Id: 's,
    {
        ModelVacant {
            shared: self.shared.clone(),
        }
    }
}
#[cfg(any(test, feature = "test-support"))]
impl<Id: SlotIdentity> oer_radio_port::conformance::registration::MockCleanup<Id>
    for OwnedModel<Id>
{
    fn abandon<'s>(
        &mut self,
        _: Option<&mut Self::Invocation<'s>>,
        authority: oer_radio_port::Quarantine,
    ) -> oer_radio_port::Abandon
    where
        Self: 's,
        Id: 's,
    {
        self.shared.with(|state| {
            state.abandoned = true;
            state.enabled = false;
            state.poisoned.get_or_insert(ModelFault::Injected);
            state.abort_operation();
            state.wake_consumer();
            state.clear_wakers();
        });
        oer_radio_port::Abandon::Stopped(authority.stopped())
    }
}

impl<Id: SlotIdentity> ModelSlotOwner<Id> for OwnedModel<Id> {
    fn end_world(&mut self, _: Option<&mut Self::Queues>) {
        end_world(&self.shared);
    }
    fn end_vacant_world(hardware: &mut ModelVacant) {
        end_world(&hardware.shared);
    }
}

fn end_world(shared: &Shared) {
    shared.with(|state| {
        state.enabled = false;
        state.operation = None;
        state.clear_wakers();
    });
}

impl<Id: SlotIdentity> SlotBodyOwner<Id> for OwnedModel<Id> {
    type Body<'p>
        = InstallParts<'p, Id, Self>
    where
        Self: 'p,
        Id: 'p;
    fn lend<'p, 's>(
        raw: &'p mut Self::Frontend<'s>,
        loan: BodyLoan<'p, Id, Self>,
    ) -> Result<Self::Body<'p>, oer_radio_port::SlotInitError>
    where
        Self: 's,
        Id: 's,
        's: 'p,
    {
        let resources =
            raw.resources
                .take()
                .ok_or(oer_radio_port::SlotInitError::MissingResource(
                    "802.15.4 resources",
                ))?;
        let lend_interface =
            |resources: InterfaceResources<'static, 0, MODEL_PENDING_SLOTS, Child<Root<Id>, 0>>| {
                InterfaceResources {
                    interface: loan.lend_slot(resources.interface),
                    short: resources.short.map(|s| loan.lend_slot(s)),
                    extended: resources.extended.map(|s| loan.lend_slot(s)),
                }
            };
        Ok(InstallParts {
            view: ModelFrontend {
                shared: raw.shared.clone(),
                held: raw.held.permits.lend(loan),
                pool: raw.held.permits.pool(),
                loan,
                end: None,
            },
            resources: ModelResources {
                operation: resources.operation.map(|s| loan.lend_seat(s)),
                interface0: lend_interface(resources.interface0),
                interface1: InterfaceResources {
                    interface: loan.lend_slot(resources.interface1.interface),
                    short: resources.interface1.short.map(|s| loan.lend_slot(s)),
                    extended: resources.interface1.extended.map(|s| loan.lend_slot(s)),
                },
                rssi: resources.rssi,
            },
        })
    }
}

impl<Id: SlotIdentity> PortBackend<Id> for OwnedModel<Id> {
    type Event<'p>
        = ModelEvent
    where
        Self: 'p,
        Id: 'p;
    type Terminal<'p>
        = ModelEvent
    where
        Self: 'p,
        Id: 'p;
    type Domain = Ieee802154Radio;
    type Fault = ModelFault;
    type Refusal = ModelRefusal;
    type View<'p>
        = ModelFrontend<'p, Id>
    where
        Self: 'p,
        Id: 'p;
    type Resources<'p>
        = ModelResources<'p, Id>
    where
        Self: 'p,
        Id: 'p;
    type Setup<'a, 'p>
        = ModelSetup<'a, 'p, Id>
    where
        Self: 'p,
        Id: 'p,
        'p: 'a;
    type Phase<'a, 'p>
        = PhaseParts<ModelEvents<'a, 'p, Id>, ModelControl<'a, 'p, Id>, ModelClients<'a, 'p, Id>>
    where
        Self: 'p,
        Id: 'p,
        'p: 'a;
    type Uninstall = ();

    fn body_parts<'p>(body: Self::Body<'p>) -> InstallParts<'p, Id, Self>
    where
        Self: 'p,
        Id: 'p,
    {
        body
    }

    fn enable<'a, 'p>(
        _: Call,
        frontend: &'a mut Self::View<'p>,
    ) -> impl Future<Output = EnableResult<ModelRefusal, ModelFault>> + 'a
    where
        Self: 'p,
        Id: 'p,
        'p: 'a,
    {
        ready(frontend.shared.with(|state| match state.poisoned {
            Some(cause) => Err(Poisoned { cause }),
            None if state.refuse_enable => Ok(Err(ModelRefusal)),
            None => {
                state.enabled = true;
                Ok(Ok(()))
            }
        }))
    }

    fn disable<'a, 'p, S>(
        _: Call,
        frontend: &'a mut Self::View<'p>,
        sink: &'a mut S,
    ) -> impl Future<Output = Result<(), Poisoned<ModelFault>>> + 'a
    where
        Self: 'p,
        Id: 'p,
        'p: 'a,
        S: FnMut(ModelEvent) -> core::ops::ControlFlow<ModelEvent>,
    {
        frontend.shared.with(|state| {
            state.abort_operation();
            for index in [ModelInterface::Zero, ModelInterface::One] {
                state.end_window(index);
                index.get_mut(&mut state.interfaces).wish = Wish::Sleep;
            }
            state.enabled = false;
            state.lost = false;
        });
        // Only terminals go to the sink. RX and loss markers end with the
        // disabled phase. No state is held across accounting or sink callbacks.
        while let Some(event) = frontend.shared.with(|state| state.events.pop_front()) {
            if let Ok(event) = event
                && let Some((interface, id)) = event.ends()
            {
                let cause = match frontend.held.complete(id) {
                    Ok(_completed) => match sink(event) {
                        core::ops::ControlFlow::Continue(()) => continue,
                        core::ops::ControlFlow::Break(event) => {
                            frontend
                                .shared
                                .with(|state| state.events.push_front(Ok(event)));
                            ModelFault::RejectedTerminal { interface, id }
                        }
                    },
                    Err(error) => {
                        frontend
                            .shared
                            .with(|state| state.events.push_front(Ok(event)));
                        ModelFault::Completion(error)
                    }
                };
                frontend.shared.with(|state| state.poisoned = Some(cause));
                return ready(Err(Poisoned { cause }));
            }
        }
        ready(frontend.shared.with(|state| match state.poisoned {
            Some(cause) => Err(Poisoned { cause }),
            None => Ok(()),
        }))
    }

    fn now<'a, 'p>(
        _: Call,
        frontend: &'a Self::View<'p>,
    ) -> impl Future<Output = PortResult<Ieee802154Instant, ClockError, ModelFault>> + 'a
    where
        Self: 'p,
        Id: 'p,
        'p: 'a,
    {
        ready(frontend.shared.with(|state| match state.poisoned {
            Some(cause) => Err(Poisoned { cause }),
            None => Ok(Ok(Ieee802154Instant::from_micros(state.now_micros))),
        }))
    }

    fn setup<'a, 'p>(_: Call, frontend: &'a mut Self::View<'p>) -> Self::Setup<'a, 'p>
    where
        Self: 'p,
        Id: 'p,
        'p: 'a,
    {
        ModelSetup {
            shared: &frontend.shared,
            brand: PhantomData,
        }
    }

    fn phase<'a, 'p>(_: Call, frontend: &'a mut Self::View<'p>) -> Self::Phase<'a, 'p>
    where
        Self: 'p,
        Id: 'p,
        'p: 'a,
    {
        PhaseParts {
            events: ModelEvents {
                shared: &frontend.shared,
                held: &frontend.held,
                end: &mut frontend.end,
            },
            control: ModelControl {
                shared: &frontend.shared,
                brand: PhantomData,
            },
            clients: ModelClients {
                shared: &frontend.shared,
                held: &frontend.held,
                pool: frontend.pool,
                loan: frontend.loan,
            },
        }
    }

    fn request_uninstall<'s>(_: &mut Self::Frontend<'s>, (): ()) -> Result<(), Poisoned<ModelFault>>
    where
        Self: 's,
        Id: 's,
    {
        Ok(())
    }
    fn shutdown_bound<'s>(_: &Self::Frontend<'s>) -> impl Future<Output = ()> + use<'s, Id>
    where
        Self: 's,
        Id: 's,
    {
        core::future::pending()
    }
}

impl<Id: SlotIdentity> Ieee802154Backend<Id> for OwnedModel<Id> {
    type Caps = ModelCaps;
}

#[cfg(test)]
mod tests;
