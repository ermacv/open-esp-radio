//! The IEEE 802.15.4 radio port on the shared owner shape of
//! `oer-radio-port` ([`Owner`](oer_radio_port::Owner),
//! [`PortBody`](oer_radio_port::PortBody)).
//!
//! Every refusal of a command port (its `CommandError`) that a
//! caller's own state explains is a type here (`docs/architecture.md`,
//! Ownership); what the world controls (the air, time, the hardware) stays a
//! typed outcome. A backend implements [`Ieee802154Backend`]; no Espressif
//! backend in this repository implements it.
//!
//! | Refusal of a command port | Form here |
//! | --- | --- |
//! | lifecycle refusals, `CommandError::Disabled`, `AlreadyEnabled`, `Quiesced` | the owner's state; the shape has no quiesce |
//! | `CommandError::Busy` (the caller's own operation runs) | the operation permit ([`Ieee802154Client::operation`], awaited in arrival order across the interfaces): a submission moves it into the backend's [`OperationPermits`], which gives it back when it takes the operation's terminal |
//! | `CommandError::Busy` (receive or sleep while an operation runs) | a client's receive or sleep is its interface's wish, which the radio resumes when the operation ends |
//! | `CommandError::EventQueueFull` | the operation permit: the backend reserves its terminal's slot with it; a scheduled receive window is a wish and owes no terminal ([`Ieee802154Client::scheduled_receive`]) |
//! | `CommandError::UnknownInterface` | a client per configured interface ([`ConfiguredInterface`]), of an index the capabilities state |
//! | `CommandError::PendingTableFull` | a pending-table [`Slot`] of the interface's own kind, which the install's resources give once and the caller counts itself |
//! | `CommandError::Unsupported`, `CommandError::TransmitPower`, a refused power, CCA threshold or header IEs | the validated values [`SupportedTransmit`], [`SupportedEnergyScan`], [`SupportedCca`], [`SupportedScheduledReceive`], [`SupportedPower`], [`SupportedCcaThreshold`], [`SupportedHeaderIes`] and the feature witness [`Has`] |
//! | enhanced-ACK settings while the generator is off | header IEs and probing initiators are kept while the generator is off and go out once it is on |
//! | duplicate request identifiers | the backend mints the [`Work`] of each submission |
//!
//! These stay the world's outcomes: [`Poisoned`] (a radio fault poisons
//! the port; the work admitted before it ends first),
//! [`EventsLost`](oer_radio_port::EventsLost) (only received frames), a
//! clock that could not be read, an operation's status on the air, and a
//! cancel that finds the work already ended
//! ([`CancelError::NotRunning`](oer_radio_port::CancelError::NotRunning)).
//! The event stream alone ends with [`oer_radio_port::EventEnd`]: `Drained`
//! accounts every admitted terminal, while `Quarantined` leaves unread
//! ownership in `PortEnd` and requires fail-stop without further draining.
//! Its variant and cause remain final across phase loans.
//!
//! # Interfaces
//!
//! The radio has [`Ieee802154Capabilities::interfaces`] addressing
//! interfaces (several with [`RadioCapabilities::MULTI_PAN`]). Interface
//! `I` is a type parameter: its configuration slot, its pending-table slots
//! and its client are of its own kinds, so an entry of one PAN's table
//! never lands in another's. As in ESP-IDF, configuration is admitted in
//! any radio state: the disabled owner's setup and a running phase's control
//! both configure an interface with its slot
//! ([`Ieee802154Interfaces::configure`]) and remove it. The caller keeps
//! the [`ConfiguredInterface`] lifecycle token; each running
//! phase lends that interface's one client to whoever holds the token by
//! `&mut` ([`Ieee802154Clients::client`]). Settings are admitted in any radio
//! state, while the interface's client lives and its operation runs.
//! Each setting replaces its latest value without a queue or capacity failure;
//! the backend reads the newest value at its application point. Identity,
//! keys, source match and ACK timeout are in force when the call returns;
//! power and CCA at the next transmission; receive-side values within one
//! frame, at once while listening with no frame in progress.
//!
//! One interface has one client at a time:
//!
//! ```compile_fail,E0499
//! # use oer_ieee802154_mac::owner::Ieee802154Clients;
//! fn twice<'id, C: Ieee802154Clients<'id>>(clients: &C, zero: &mut C::Configured<0>) {
//!     let first = clients.client(zero);
//!     let second = clients.client(zero);
//!     drop((first, second));
//! }
//! ```
//!
//! Its compiling twin lends it again after the first client:
//!
//! ```
//! # use oer_ieee802154_mac::owner::Ieee802154Clients;
//! fn again<'id, C: Ieee802154Clients<'id>>(clients: &C, zero: &mut C::Configured<0>) {
//!     drop(clients.client(&mut *zero));
//!     drop(clients.client(zero));
//! }
//! ```
//!
//! A setting reaches the interface while its client lives:
//!
//! ```
//! # use oer_ieee802154_mac::owner::{Ieee802154Clients, Ieee802154Client};
//! fn set<'id, C: Ieee802154Clients<'id>>(clients: &C, zero: &mut C::Configured<0>) {
//!     let client = clients.client(zero);
//!     client.set_pan_id(1);
//!     drop(client);
//! }
//! ```
//!
//! Removal needs the lifecycle token back from the client:
//!
//! ```compile_fail,E0505
//! # use oer_ieee802154_mac::owner::{Ieee802154Clients, Ieee802154Interfaces};
//! fn remove<'id, I, C>(control: &mut I, clients: &C, zero: I::Configured<0>)
//! where
//!     I: Ieee802154Interfaces<'id>,
//!     C: Ieee802154Clients<'id, Configured<0> = I::Configured<0>>,
//! {
//!     let mut zero = zero;
//!     let client = clients.client(&mut zero);
//!     control.remove(zero);
//!     drop(client);
//! }
//! ```
//!
//! The receive state is the radio's, but each interface's stack asks for
//! it. The radio receives while any configured interface asks to receive;
//! the interfaces share one channel, so a client asking to receive on
//! another channel waits until the interfaces receiving on the current one
//! sleep. An operation (a transmission, an energy
//! scan, a clear-channel assessment) and an open scheduled receive window
//! take the radio for their duration and return it to that receive state:
//! a window keeps its time on its own channel.
//!
//! # Tokens
//!
//! Every token carries the install's brand `'id`. The operation permit
//! stays in the backend's held part of the install ([`OperationPermits`]):
//! a client awaits it with its interface's seat in the operation's line
//! ([`Permits`](oer_radio_port::Permits) issues one per interface client to [`WorkPermits::new`], which the body's
//! resources hand out and the caller keeps across takes) as a guard that
//! gives it back when dropped before a submission; a submission moves it in, and the backend gives it back when
//! it takes the operation's terminal, before it hands the event out. No
//! caller holds a submitted operation's permit, and a dropped handle or
//! event loses none.
//!
//! # Compile-fail tests
//!
//! No caller makes an operation permit:
//!
//! ```compile_fail,E0451
//! # use oer_ieee802154_mac::owner::{OperationPermit, WorkPermit};
//! fn forge<'q, 'id>() -> OperationPermit<'q, 'id> {
//!     WorkPermit { held: todo!() }
//! }
//! ```
//!
//! Its compiling twin reads the permit a client gave:
//!
//! ```
//! # use oer_ieee802154_mac::owner::OperationPermit;
//! fn held<'q, 'id>(permit: &'q OperationPermit<'q, 'id>) -> &'q OperationPermit<'q, 'id> {
//!     permit
//! }
//! ```
//!
//! No caller mints a validated value without its check:
//!
//! ```compile_fail,E0451
//! # use core::marker::PhantomData;
//! # use oer_ieee802154_mac::owner::SupportedPower;
//! fn forge<K>() -> SupportedPower<K> {
//!     SupportedPower { dbm: 20, brand: PhantomData }
//! }
//! ```
//!
//! ```
//! # use oer_ieee802154_mac::owner::{Ieee802154Caps, SupportedPower};
//! fn checked<K: Ieee802154Caps>() -> Option<SupportedPower<K>> {
//!     SupportedPower::new(20).ok()
//! }
//! ```
//!
//! An optional feature the capabilities do not state does not compile as a
//! constant:
//!
//! ```compile_fail,E0080
//! # use oer_ieee802154_mac::owner::{Has, Ieee802154Caps, Promiscuous};
//! # use oer_ieee802154_mac::{Ieee802154Capabilities, RadioCapabilities};
//! # struct Plain;
//! # impl Ieee802154Caps for Plain {
//! #     const CAPS: Ieee802154Capabilities = Ieee802154Capabilities {
//! #         operations: RadioCapabilities::NONE,
//! #         interfaces: 1,
//! #         transmit_power: None,
//! #         cca_threshold: None,
//! #         receive_sensitivity_dbm: -100,
//! #         csl: None,
//! #         enhanced_ack_header_ies: 0,
//! #     };
//! # }
//! const SNIFF: Has<Plain, Promiscuous> = Has::from_const();
//! # fn main() { let _ = SNIFF; }
//! ```
//!
//! Its compiling twin names a feature the capabilities state:
//!
//! ```
//! # use oer_ieee802154_mac::owner::{Has, Ieee802154Caps, Promiscuous};
//! # use oer_ieee802154_mac::{Ieee802154Capabilities, RadioCapabilities};
//! # struct Sniffer;
//! # impl Ieee802154Caps for Sniffer {
//! #     const CAPS: Ieee802154Capabilities = Ieee802154Capabilities {
//! #         operations: RadioCapabilities::PROMISCUOUS,
//! #         interfaces: 1,
//! #         transmit_power: None,
//! #         cca_threshold: None,
//! #         receive_sensitivity_dbm: -100,
//! #         csl: None,
//! #         enhanced_ack_header_ies: 0,
//! #     };
//! # }
//! const SNIFF: Has<Sniffer, Promiscuous> = Has::from_const();
//! # fn main() { let _ = SNIFF; }
//! ```
//!
//! An interface the capabilities do not state has no resources:
//!
//! ```compile_fail,E0080
//! # use oer_ieee802154_mac::owner::{Ieee802154Caps, InterfacePool, InterfaceResources};
//! # use oer_ieee802154_mac::{Ieee802154Capabilities, RadioCapabilities};
//! # use oer_radio_port::Mint;
//! # struct One;
//! # impl Ieee802154Caps for One {
//! #     const CAPS: Ieee802154Capabilities = Ieee802154Capabilities {
//! #         operations: RadioCapabilities::SOURCE_MATCH,
//! #         interfaces: 1,
//! #         transmit_power: None,
//! #         cca_threshold: None,
//! #         receive_sensitivity_dbm: -100,
//! #         csl: None,
//! #         enhanced_ack_header_ies: 0,
//! #     };
//! # }
//! fn second<'id>(mint: Mint<'id, InterfacePool<One, 1, 4>>) -> InterfaceResources<'id, 1, 4> {
//!     mint.into_pools()
//! }
//! # fn main() { let _: fn(Mint<'static, InterfacePool<One, 1, 4>>) -> InterfaceResources<'static, 1, 4> = second; }
//! ```
//!
//! Its compiling twin mints the resources of the interface it has:
//!
//! ```
//! # use oer_ieee802154_mac::owner::{Ieee802154Caps, InterfacePool, InterfaceResources};
//! # use oer_ieee802154_mac::{Ieee802154Capabilities, RadioCapabilities};
//! # use oer_radio_port::Mint;
//! # struct One;
//! # impl Ieee802154Caps for One {
//! #     const CAPS: Ieee802154Capabilities = Ieee802154Capabilities {
//! #         operations: RadioCapabilities::SOURCE_MATCH,
//! #         interfaces: 1,
//! #         transmit_power: None,
//! #         cca_threshold: None,
//! #         receive_sensitivity_dbm: -100,
//! #         csl: None,
//! #         enhanced_ack_header_ies: 0,
//! #     };
//! # }
//! fn first<'id>(mint: Mint<'id, InterfacePool<One, 0, 4>>) -> InterfaceResources<'id, 0, 4> {
//!     mint.into_pools()
//! }
//! # fn main() { let _: fn(Mint<'static, InterfacePool<One, 0, 4>>) -> InterfaceResources<'static, 0, 4> = first; }
//! ```
//!
//! A pending-table slot of one interface does not fit another's table:
//!
//! ```compile_fail,E0308
//! # use oer_ieee802154_mac::owner::{ShortPending};
//! # use oer_radio_port::Slot;
//! fn cross<'id>(slot: Slot<'id, ShortPending<0>>) -> Slot<'id, ShortPending<1>> {
//!     slot
//! }
//! ```
//!
//! ```
//! # use oer_ieee802154_mac::owner::{ShortPending};
//! # use oer_radio_port::Slot;
//! fn same<'id>(slot: Slot<'id, ShortPending<1>>) -> Slot<'id, ShortPending<1>> {
//!     slot
//! }
//! ```

use core::{cell::RefCell, future::Future, marker::PhantomData};

use oer_radio_port::{
    Child, Line, Permit, PermitPool, PermitSupply, Poisoned, PoolPath, Root, Seat, Slot,
};
use oer_radio_port::{PortBackend, PortPhase, RadioPortClient, RadioWork, Work};

use crate::{
    AppliedSecurity, CcaMode, Channel, CslReceiver, EnhancedAckGeneration, FrameCounterUpdate,
    FrameView, Ieee802154Capabilities, Ieee802154Instant, Ieee802154Radio, Interface,
    ProbingInitiator, RadioCapabilities, ReceivedFrame, RequestId, TimeSync, TxMode, TxSecurity,
    TxStatus,
    mac::pending::AutoPendingMode,
    radio::power::{CcaThresholdCapability, CcaThresholdError, TransmitPowerError},
};

/// What an IEEE 802.15.4 backend type supports, the same for every
/// instance: the brand of its validated values.
pub trait Ieee802154Caps {
    /// The backend's capabilities.
    const CAPS: Ieee802154Capabilities;
}

/// An optional feature of [`RadioCapabilities`], named by a type so that a
/// call needing it takes the witness [`Has`].
pub trait Feature {
    /// The capability bits the feature needs.
    const REQUIRED: RadioCapabilities;
}

/// Promiscuous reception ([`RadioCapabilities::PROMISCUOUS`]).
pub enum Promiscuous {}
/// The frame-pending table ([`RadioCapabilities::SOURCE_MATCH`]).
pub enum SourceMatch {}
/// Automatic acknowledgement ([`RadioCapabilities::AUTOMATIC_ACKNOWLEDGEMENT`]).
pub enum AutomaticAcknowledgement {}
/// Clear-channel assessment ([`RadioCapabilities::CLEAR_CHANNEL_ASSESSMENT`]).
pub enum ClearChannelAssessment {}

impl Feature for Promiscuous {
    const REQUIRED: RadioCapabilities = RadioCapabilities::PROMISCUOUS;
}

impl Feature for SourceMatch {
    const REQUIRED: RadioCapabilities = RadioCapabilities::SOURCE_MATCH;
}

impl Feature for AutomaticAcknowledgement {
    const REQUIRED: RadioCapabilities = RadioCapabilities::AUTOMATIC_ACKNOWLEDGEMENT;
}

impl Feature for ClearChannelAssessment {
    const REQUIRED: RadioCapabilities = RadioCapabilities::CLEAR_CHANNEL_ASSESSMENT;
}

/// The witness that the capabilities `K` state the feature `F`.
#[derive(Debug)]
pub struct Has<K, F> {
    brand: PhantomData<fn() -> (K, F)>,
}

impl<K: Ieee802154Caps, F: Feature> Has<K, F> {
    /// The witness, when `K` states `F`.
    pub const fn new() -> Option<Self> {
        if K::CAPS.operations.contains(F::REQUIRED) {
            Some(Self { brand: PhantomData })
        } else {
            None
        }
    }

    /// The witness, checked when the image is built.
    ///
    /// The inline constant rejects an unsupported feature during code
    /// generation, even when called outside a constant expression.
    pub const fn from_const() -> Self {
        const {
            assert!(
                K::CAPS.operations.contains(F::REQUIRED),
                "the backend does not state this feature"
            );
        }
        Self { brand: PhantomData }
    }
}

/// A transmit power `K` radiates: the level a request selects
/// ([`TransmitPowerLevels::select`](crate::TransmitPowerLevels::select)).
#[derive(Debug)]
pub struct SupportedPower<K> {
    dbm: i8,
    brand: PhantomData<fn() -> K>,
}

impl<K: Ieee802154Caps> SupportedPower<K> {
    /// The level `requested_dbm` selects among `K`'s levels.
    ///
    /// # Errors
    ///
    /// `K` publishes no transmit-power capability, or the request lies
    /// below its lowest level.
    pub const fn new(requested_dbm: i8) -> Result<Self, TransmitPowerError> {
        match crate::TransmitPowerCapability::check(K::CAPS.transmit_power, requested_dbm) {
            Ok(dbm) => Ok(Self {
                dbm,
                brand: PhantomData,
            }),
            Err(error) => Err(error),
        }
    }

    /// The level in dBm.
    pub const fn dbm(&self) -> i8 {
        self.dbm
    }
}

/// A clear-channel assessment energy threshold within `K`'s range.
#[derive(Debug)]
pub struct SupportedCcaThreshold<K> {
    dbm: i8,
    brand: PhantomData<fn() -> K>,
}

impl<K: Ieee802154Caps> SupportedCcaThreshold<K> {
    /// `dbm`, when `K` accepts it.
    ///
    /// # Errors
    ///
    /// `K` publishes no threshold capability, or `dbm` lies outside it.
    pub const fn new(dbm: i8) -> Result<Self, CcaThresholdError> {
        match CcaThresholdCapability::check(K::CAPS.cca_threshold, dbm) {
            Ok(()) => Ok(Self {
                dbm,
                brand: PhantomData,
            }),
            Err(error) => Err(error),
        }
    }

    /// The threshold in dBm.
    pub const fn dbm(&self) -> i8 {
        self.dbm
    }
}

/// The header IEs every enhanced ACK carries, no longer than `K` holds.
#[derive(Debug)]
pub struct SupportedHeaderIes<'a, K> {
    ies: &'a [u8],
    brand: PhantomData<fn() -> K>,
}

impl<'a, K: Ieee802154Caps> SupportedHeaderIes<'a, K> {
    /// `ies`, when `K` holds that many bytes.
    pub const fn new(ies: &'a [u8]) -> Option<Self> {
        if ies.len() <= K::CAPS.enhanced_ack_header_ies as usize {
            Some(Self {
                ies,
                brand: PhantomData,
            })
        } else {
            None
        }
    }

    /// The IE bytes.
    pub const fn get(&self) -> &'a [u8] {
        self.ies
    }
}

/// One transmission as a client describes it, before the capabilities
/// check it ([`SupportedTransmit::new`]).
#[derive(Debug)]
pub struct Transmission<'f, K> {
    /// The MAC frame without FCS.
    pub frame: FrameView<'f>,
    /// The channel it goes out on.
    pub channel: Channel,
    /// How it gains the channel.
    pub mode: TxMode,
    /// Its power; `None` sends at the channel's power.
    pub power: Option<SupportedPower<K>>,
    /// Retransmissions after an unacknowledged attempt.
    pub max_frame_retries: u8,
    /// How it is secured.
    pub security: TxSecurity,
    /// The Time IE the radio fills at the SFD.
    pub time_sync: Option<TimeSync>,
}

/// A transmission every part of which `K` performs.
#[derive(Debug)]
pub struct SupportedTransmit<'f, K> {
    transmission: Transmission<'f, K>,
}

impl<'f, K: Ieee802154Caps> SupportedTransmit<'f, K> {
    /// `transmission`, when `K` performs its mode, its acknowledgement
    /// request, its retries, its Time IE and its security.
    ///
    /// # Errors
    ///
    /// The transmission and the capability bits it misses.
    pub fn new(
        transmission: Transmission<'f, K>,
    ) -> Result<Self, (Transmission<'f, K>, RadioCapabilities)> {
        let operations = K::CAPS.operations;
        let mut missing = RadioCapabilities::NONE;
        let mut need = |used: bool, required: RadioCapabilities| {
            if used && !operations.contains(required) {
                missing |= required;
            }
        };
        match transmission.mode {
            TxMode::Direct => {}
            TxMode::ClearChannelAssessment => {
                need(true, RadioCapabilities::CLEAR_CHANNEL_ASSESSMENT);
            }
            TxMode::CsmaCa { .. } => need(true, RadioCapabilities::CSMA_CA),
            TxMode::Scheduled { cca, .. } => {
                need(true, RadioCapabilities::SCHEDULED_TRANSMIT);
                need(cca, RadioCapabilities::CLEAR_CHANNEL_ASSESSMENT);
            }
        }
        need(
            transmission.frame.acknowledgement_requested(),
            RadioCapabilities::HARDWARE_ACKNOWLEDGEMENT,
        );
        need(
            transmission.max_frame_retries > 0,
            RadioCapabilities::TRANSMIT_RETRIES,
        );
        need(
            transmission.time_sync.is_some(),
            RadioCapabilities::TIME_SYNC,
        );
        need(
            matches!(transmission.security, TxSecurity::Key { .. }),
            RadioCapabilities::SECURITY_OFFLOAD,
        );
        if missing == RadioCapabilities::NONE {
            Ok(Self { transmission })
        } else {
            Err((transmission, missing))
        }
    }

    /// The checked transmission.
    pub const fn get(&self) -> &Transmission<'f, K> {
        &self.transmission
    }
}

/// An energy scan `K` performs.
#[derive(Debug)]
pub struct SupportedEnergyScan<K> {
    channel: Channel,
    duration_us: u32,
    brand: PhantomData<fn() -> K>,
}

impl<K: Ieee802154Caps> SupportedEnergyScan<K> {
    /// A scan of `channel` for `duration_us`, when `K` scans.
    pub const fn new(channel: Channel, duration_us: u32) -> Option<Self> {
        if K::CAPS.operations.contains(RadioCapabilities::ENERGY_SCAN) {
            Some(Self {
                channel,
                duration_us,
                brand: PhantomData,
            })
        } else {
            None
        }
    }

    /// The channel and the duration in microseconds.
    pub const fn get(&self) -> (Channel, u32) {
        (self.channel, self.duration_us)
    }
}

/// A standalone clear-channel assessment `K` performs.
#[derive(Debug)]
pub struct SupportedCca<K> {
    channel: Channel,
    brand: PhantomData<fn() -> K>,
}

impl<K: Ieee802154Caps> SupportedCca<K> {
    /// An assessment of `channel`, when `K` assesses channels.
    pub const fn new(channel: Channel) -> Option<Self> {
        if K::CAPS
            .operations
            .contains(RadioCapabilities::CLEAR_CHANNEL_ASSESSMENT)
        {
            Some(Self {
                channel,
                brand: PhantomData,
            })
        } else {
            None
        }
    }

    /// The channel.
    pub const fn get(&self) -> Channel {
        self.channel
    }
}

/// A scheduled receive window `K` opens.
#[derive(Debug)]
pub struct SupportedScheduledReceive<K> {
    channel: Channel,
    start: Ieee802154Instant,
    duration_us: u32,
    brand: PhantomData<fn() -> K>,
}

impl<K: Ieee802154Caps> SupportedScheduledReceive<K> {
    /// A window on `channel` from `start` for `duration_us`, when `K`
    /// schedules receive windows.
    pub const fn new(channel: Channel, start: Ieee802154Instant, duration_us: u32) -> Option<Self> {
        if K::CAPS
            .operations
            .contains(RadioCapabilities::SCHEDULED_RECEIVE)
        {
            Some(Self {
                channel,
                start,
                duration_us,
                brand: PhantomData,
            })
        } else {
            None
        }
    }

    /// The channel, the start and the duration in microseconds.
    pub const fn get(&self) -> (Channel, Ieee802154Instant, u32) {
        (self.channel, self.start, self.duration_us)
    }
}

/// The kind of interface `I`'s configuration slot.
pub enum InterfaceKind<const I: u8> {}
/// The kind of interface `I`'s short-address pending-table slots.
pub enum ShortPending<const I: u8> {}
/// The kind of interface `I`'s extended-address pending-table slots.
pub enum ExtendedPending<const I: u8> {}

/// The port-lifetime resources of interface `I`: its configuration slot
/// and `P` pending-table slots of each address size. A radio without the
/// frame-pending table has none. `Path` identifies this interface component;
/// its three child paths distinguish configuration, short and extended slots.
pub struct InterfaceResources<'id, const I: u8, const P: usize, Path: PoolPath = Root> {
    /// The slot [`Ieee802154Interfaces::configure`] takes.
    pub interface: Slot<'id, InterfaceKind<I>, Child<Path, 0>>,
    /// The slots of short source addresses.
    pub short: [Slot<'id, ShortPending<I>, Child<Path, 1>>; P],
    /// The slots of extended source addresses.
    pub extended: [Slot<'id, ExtendedPending<I>, Child<Path, 2>>; P],
}

/// The complete pool layout of interface `I` for capabilities `K`.
/// Another interface than `K` advertises, or pending slots without source
/// matching, is rejected at compile time when the layout is issued.
pub struct InterfacePool<K, const I: u8, const P: usize>(PhantomData<fn() -> K>);

impl<K: Ieee802154Caps, const I: u8, const P: usize> oer_radio_port::PoolSet
    for InterfacePool<K, I, P>
{
    type Pools<'id, Path: PoolPath> = InterfaceResources<'id, I, P, Path>;

    fn build<'id, Path: PoolPath>(
        builder: oer_radio_port::PoolBuilder<'id, Path>,
    ) -> Self::Pools<'id, Path> {
        const {
            assert!(
                I < K::CAPS.interfaces,
                "the capabilities state no such interface"
            );
            assert!(
                P == 0 || K::CAPS.operations.contains(RadioCapabilities::SOURCE_MATCH),
                "pending slots need the frame-pending table"
            );
        };
        let ([interface], short, extended) = builder.into_pools::<(
            oer_radio_port::Slots<InterfaceKind<I>, 1>,
            oer_radio_port::Slots<ShortPending<I>, P>,
            oer_radio_port::Slots<ExtendedPending<I>, P>,
        )>();
        InterfaceResources {
            interface,
            short,
            extended,
        }
    }
}

/// The permit kind of the radio's one operation.
pub enum Operation {}

/// What gives a taken permit of kind `K` back: the [`WorkPermits`] it came
/// from.
trait Release<'id, K, Path: PoolPath> {
    fn release(&self, permit: Permit<'id, K, Path>);
}

/// A permit of kind `K` that admitted work holds until its terminal, which
/// only [`WorkPermits::guard`] makes. Dropped before a submission, it goes
/// back to its pool; a submission moves it into the backend.
#[must_use = "submit the permit, or drop it to return its capacity"]
#[allow(
    clippy::type_complexity,
    reason = "the affine permit and its return function are held by the same guard"
)]
pub struct WorkPermit<'q, 'id, K, Path: PoolPath = Root> {
    held: scopeguard::ScopeGuard<HeldPermit<'q, 'id, K, Path>, fn(HeldPermit<'q, 'id, K, Path>)>,
}

struct HeldPermit<'q, 'id, K, Path: PoolPath> {
    permit: Permit<'id, K, Path>,
    permits: &'q dyn Release<'id, K, Path>,
}

fn release_held<K, Path: PoolPath>(held: HeldPermit<'_, '_, K, Path>) {
    held.permits.release(held.permit);
}

/// The radio's one operation permit.
pub type OperationPermit<'q, 'id, Path = Root> = WorkPermit<'q, 'id, Operation, Path>;

/// The terminal whose matching permit has been returned to its pool.
#[must_use = "hand the completed operation's terminal to its consumer"]
pub struct Completed {
    interface: Interface,
    id: RequestId,
}

impl Completed {
    /// The interface whose operation ended.
    pub const fn interface(&self) -> Interface {
        self.interface
    }
    /// The completed operation's correlation identity.
    pub const fn id(&self) -> RequestId {
        self.id
    }
}

/// A terminal did not name the held operation. Its permit stays retained;
/// the backend must publish poisoning instead of fabricating a completion.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CompletionMismatch {
    /// The terminal received from the driver.
    pub received: RequestId,
    /// The operation still held, if any.
    pub held: Option<(Interface, RequestId)>,
}

/// The work in flight: whose it is and its permit.
struct InFlight<'id, K, Path: PoolPath> {
    interface: Interface,
    id: RequestId,
    permit: Permit<'id, K, Path>,
}

/// The one permit of kind `K` of an install, which a backend keeps in its
/// held part (`PortBackend::Held`): the permit in the pool `P`, which
/// serves its takers in its documented order, and the work in flight,
/// keyed by its interface and identity. A client awaits the permit with its
/// interface's seat in the pool's line, never refused; a submission moves
/// it in, and the backend gives it back when it takes the work's terminal,
/// before it hands the event out.
pub struct WorkPermits<'id, K, P, Path: PoolPath = Root> {
    pool: P,
    in_flight: RefCell<Option<InFlight<'id, K, Path>>>,
}

/// The operation permit of one install.
pub type OperationPermits<'id, P, Path = Root> = WorkPermits<'id, Operation, P, Path>;

impl<'id, K, Path: PoolPath, P: PermitPool<'id, K, Path>> WorkPermits<'id, K, P, Path> {
    /// Consume the installation's declared permit component: its one permit
    /// of kind `K`, handed to `pool` with the line of its `S` seats, and the
    /// seats, one per interface client, which the body's resources hand
    /// out.
    pub fn new<const S: usize>(
        tokens: oer_radio_port::PermitTokens<'id, K, 1, S, Path>,
        pool: impl FnOnce(PermitSupply<'id, K, 1, Path>, Line<'id, K, S, Path>) -> P,
    ) -> (Self, [Seat<'id, K, Path>; S]) {
        let oer_radio_port::PermitTokens {
            supply,
            line,
            seats,
        } = tokens;
        (
            Self {
                pool: pool(supply, line),
                in_flight: RefCell::new(None),
            },
            seats,
        )
    }

    /// The pool a client awaits the permit from.
    pub const fn pool(&self) -> &P {
        &self.pool
    }

    /// Guard `permit`, which [`Self::pool`] served: dropped before a
    /// submission, the guard gives it back. The pool holds the install's one
    /// permit of kind `K` at `Path`, so every permit of this path is that one.
    pub fn guard(&self, permit: Permit<'id, K, Path>) -> WorkPermit<'_, 'id, K, Path> {
        WorkPermit {
            held: scopeguard::guard(
                HeldPermit {
                    permit,
                    permits: self,
                },
                release_held,
            ),
        }
    }

    /// Keep the permit of the admitted work `id` of `interface` until its
    /// terminal; a backend's submission calls it.
    pub fn admit(&self, interface: Interface, id: RequestId, permit: WorkPermit<'_, 'id, K, Path>) {
        let HeldPermit { permit, .. } = scopeguard::ScopeGuard::into_inner(permit.held);
        // The pool holds one permit, so the entry is free while it was out.
        *self.in_flight.borrow_mut() = Some(InFlight {
            interface,
            id,
            permit,
        });
    }

    /// Return the permit of the terminal `id` and its affine completion.
    /// A mismatch leaves the admitted operation and its capacity intact.
    pub fn complete(&self, id: RequestId) -> Result<Completed, CompletionMismatch> {
        let mut in_flight = self.in_flight.borrow_mut();
        match in_flight.take_if(|held| held.id == id) {
            Some(held) => {
                self.pool.release(held.permit);
                Ok(Completed {
                    interface: held.interface,
                    id: held.id,
                })
            }
            None => Err(CompletionMismatch {
                received: id,
                held: in_flight.as_ref().map(|held| (held.interface, held.id)),
            }),
        }
    }

    /// The work in flight, by interface and identity.
    pub fn in_flight(&self) -> Option<(Interface, RequestId)> {
        self.in_flight
            .borrow()
            .as_ref()
            .map(|held| (held.interface, held.id))
    }
}

impl<'id, K, Path: PoolPath, P: PermitPool<'id, K, Path>> Release<'id, K, Path>
    for WorkPermits<'id, K, P, Path>
{
    fn release(&self, permit: Permit<'id, K, Path>) {
        self.pool.release(permit);
    }
}

/// An IEEE 802.15.4 backend on the owner shape. Its disabled owner's settings
/// are an [`Ieee802154Setup`], its phase control an [`Ieee802154Control`],
/// its clients [`Ieee802154Clients`] and its events an
/// [`Ieee802154Events`]; its held part of the install keeps the operation
/// permit ([`OperationPermits`]), and the body's resources give each
/// interface's configuration and pending-table slots once
/// ([`InterfaceResources`]).
pub trait Ieee802154Backend: PortBackend {
    /// The backend's capabilities marker.
    type Caps: Ieee802154Caps;
}

/// The radio-wide settings, which the disabled owner's setup and a running
/// phase's control both change. None refuses: every value is checked.
pub trait RadioSettings {
    /// The backend's capabilities marker.
    type Caps: Ieee802154Caps;

    /// The transmit power of every channel (`esp_ieee802154_set_txpower`).
    fn set_transmit_power(&mut self, power: SupportedPower<Self::Caps>);

    /// The transmit power of one channel
    /// (`esp_ieee802154_set_power_with_channel`).
    fn set_channel_power(&mut self, channel: Channel, power: SupportedPower<Self::Caps>);

    /// The energy threshold of clear-channel assessment.
    fn set_cca_threshold(&mut self, threshold: SupportedCcaThreshold<Self::Caps>);

    /// How clear-channel assessment decides that the channel is busy.
    fn set_cca_mode(&mut self, has: Has<Self::Caps, ClearChannelAssessment>, mode: CcaMode);

    /// Receive every frame, unfiltered.
    fn set_promiscuous(&mut self, has: Has<Self::Caps, Promiscuous>, promiscuous: bool);

    /// Acknowledge received frames that request it.
    fn set_automatic_acknowledgement(
        &mut self,
        has: Has<Self::Caps, AutomaticAcknowledgement>,
        acknowledge: bool,
    );

    /// Act as the PAN coordinator in address filtering.
    fn set_pan_coordinator(&mut self, coordinator: bool);

    /// The CSL receiver state.
    fn set_csl(&mut self, csl: CslReceiver);

    /// Generate enhanced ACKs, or refuse every enhanced ACK (`None`). The
    /// header IEs and probing initiators set meanwhile are kept.
    fn set_enhanced_ack(&mut self, generation: Option<EnhancedAckGeneration>);

    /// The header IEs every enhanced ACK carries.
    fn set_enhanced_ack_header_ies(&mut self, ies: SupportedHeaderIes<'_, Self::Caps>);

    /// The Link Metrics probing initiators, most recently added first.
    fn set_enhanced_ack_probing(&mut self, initiators: &[ProbingInitiator]);
}

/// The configuration of an IEEE 802.15.4 port's interfaces, admitted in
/// any radio state, as in ESP-IDF: the disabled owner's setup and a running
/// phase's control both configure and remove an interface. Its client
/// changes settings while it lives and its operation runs.
pub trait Ieee802154Interfaces<'id>: RadioSettings {
    /// Pool component containing interface I's resources.
    type InterfacePath<const I: u8>: PoolPath;
    /// A configured interface `I`; it holds the interface's slot.
    type Configured<const I: u8>: ConfiguredInterface<'id, I>;
    /// Configure interface `I`: its client exists in the running phase from
    /// now on, and in every later one. Nothing refuses it: the slot proves
    /// the interface exists and is not configured.
    fn configure<const I: u8>(
        &mut self,
        slot: Slot<'id, InterfaceKind<I>, Child<Self::InterfacePath<I>, 0>>,
    ) -> Self::Configured<I>;

    /// Remove interface `I`, giving its slot back. In a running phase its
    /// wish ends (a window included) and an operation it started ends with
    /// its terminal. Its pending entries stay installed with their tokens,
    /// and apply again when it is configured again.
    fn remove<const I: u8>(
        &mut self,
        interface: Self::Configured<I>,
    ) -> Slot<'id, InterfaceKind<I>, Child<Self::InterfacePath<I>, 0>>;
}

/// The disabled owner's settings of an IEEE 802.15.4 port: the radio-wide
/// settings and its interfaces.
pub trait Ieee802154Setup<'id>: Ieee802154Interfaces<'id> {}

/// A configured interface `I` of the install `'id`, which the disabled
/// owner keeps and lends into each phase.
pub trait ConfiguredInterface<'id, const I: u8> {
    /// The interface.
    fn interface(&self) -> Interface {
        Interface::new(I)
    }
}

/// The control of a running IEEE 802.15.4 phase, with one holder: the
/// radio-wide settings and the interfaces, as the disabled owner's setup.
pub trait Ieee802154Control<'id>: Ieee802154Interfaces<'id> {}

/// The clients of a running phase, one per configured interface.
pub trait Ieee802154Clients<'id> {
    /// A configured interface `I`, as the setup and the control name it.
    type Configured<const I: u8>: ConfiguredInterface<'id, I>;
    /// Interface `I`'s client.
    type Client<'c, const I: u8>: Ieee802154Client<'id, I>
    where
        Self: 'c;

    /// The client of the configured interface `I`, the only one while it
    /// lives: it borrows the lifecycle token, so removal waits for its return.
    fn client<'c, const I: u8>(
        &'c self,
        interface: &'c mut Self::Configured<I>,
    ) -> Self::Client<'c, I>;
}

/// What the client of interface `I` does in a running phase of the install
/// `'id`. Configuration is admitted in any radio state, while the interface's
/// client lives and its operation runs. Each call replaces the latest value
/// synchronously, without a queue, future or capacity failure. The backend
/// reads the newest value at its application point; pending entries keep
/// their owned slots and entry tokens.
pub trait Ieee802154Client<'id, const I: u8>:
    RadioPortClient<Token: RadioWork<Id = RequestId>, Domain = Ieee802154Radio> + Sized
{
    /// The backend's capabilities marker.
    type Caps: Ieee802154Caps;
    /// Pool component containing the radio's operation permit and waiter line.
    type OperationPath: PoolPath;
    /// Pool component containing this interface's pending-table slots.
    type InterfacePath: PoolPath;
    /// An installed short-address pending entry of interface `I`; removing
    /// it gives its slot back.
    type ShortEntry;
    /// An installed extended-address pending entry of interface `I`;
    /// removing it gives its slot back.
    type ExtendedEntry;

    /// The PAN identifier the interface filters on.
    fn set_pan_id(&self, pan_id: u16);

    /// The interface's short address.
    fn set_short_address(&self, address: u16);

    /// The interface's extended address in frame byte order.
    fn set_extended_address(&self, address: [u8; 8]);

    /// Replace the interface's MAC keys (`otPlatRadioSetMacKey`), keeping
    /// its frame counter unless `frame_counter` replaces it in the same
    /// request. Only a key replacement can lower the counter. The new keys
    /// affect subsequent frames; a submitted transmission keeps the security
    /// snapshot in its request ([`TxSecurity`]).
    fn set_mac_keys(
        &self,
        key_id: u8,
        previous: [u8; 16],
        current: [u8; 16],
        next: [u8; 16],
        frame_counter: Option<u32>,
    );

    /// Forget the interface's MAC keys: its secured frames go out as given.
    fn remove_mac_keys(&self);

    /// Raise the interface's frame counter, never lower it.
    fn update_frame_counter(&self, update: FrameCounterUpdate);

    /// The interface's next frame counter, a published snapshot; `None`
    /// without keys.
    fn frame_counter(&self) -> Option<u32>;

    /// How the interface's acknowledgements set frame pending.
    fn set_pending_mode(&self, has: Has<Self::Caps, SourceMatch>, mode: AutoPendingMode);

    /// Mark `address` as having pending data, in `slot`.
    fn add_pending_short(
        &self,
        slot: Slot<'id, ShortPending<I>, Child<Self::InterfacePath, 1>>,
        address: [u8; 2],
    ) -> Self::ShortEntry;

    /// Mark `address` as having pending data, in `slot`.
    fn add_pending_extended(
        &self,
        slot: Slot<'id, ExtendedPending<I>, Child<Self::InterfacePath, 2>>,
        address: [u8; 8],
    ) -> Self::ExtendedEntry;

    /// Clear `entry`, giving its slot back.
    fn remove_pending_short(
        &self,
        entry: Self::ShortEntry,
    ) -> Slot<'id, ShortPending<I>, Child<Self::InterfacePath, 1>>;

    /// Clear `entry`, giving its slot back.
    fn remove_pending_extended(
        &self,
        entry: Self::ExtendedEntry,
    ) -> Slot<'id, ExtendedPending<I>, Child<Self::InterfacePath, 2>>;

    /// Ask to receive on `channel`: the request replaces the interface's
    /// previous wish at once (a window included), so it no longer receives
    /// elsewhere, and the future ends once the radio receives there for
    /// this interface, which waits while another interface receives on
    /// another channel. Two interfaces moving to one channel admit each
    /// other.
    fn receive(&self, channel: Channel) -> impl Future<Output = Result<(), Poisoned<Self::Fault>>>;

    /// Ask to sleep, replacing a window too; the radio sleeps once no
    /// interface asks to receive.
    fn sleep(&self);

    /// Ask to sleep until a window opens, then receive in it, then sleep:
    /// a wish, as `receive` and `sleep` are, set at once and replaced by
    /// the next one. The window keeps its time:
    /// while it is open the radio receives on its channel, whatever another
    /// interface asks, and returns to the interfaces' receive after it.
    /// Frames received in it arrive as ordinary receptions; the window
    /// itself owes no terminal, and its end is published state
    /// ([`Self::window_ended`]).
    fn scheduled_receive(&self, window: SupportedScheduledReceive<Self::Caps>);

    /// Wait until the interface has no window: its last one ended, a new
    /// wish replaced it, or it never asked for one.
    fn window_ended(&self) -> impl Future<Output = Result<(), Poisoned<Self::Fault>>>;

    /// The radio's one operation permit, once no operation holds it: a
    /// running operation is waited for with the interface's `seat` in the
    /// operation's line, in arrival order across the interfaces, never
    /// refused. The seat is lent only while the take waits.
    fn operation<'c, 's>(
        &'c self,
        seat: &'s mut Seat<'id, Operation, Self::OperationPath>,
    ) -> impl Future<Output = OperationPermit<'c, 'id, Self::OperationPath>> + 's
    where
        'id: 'c,
        'c: 's;

    /// Transmit; the permit stays with the backend until it takes the
    /// transmission's terminal.
    ///
    /// # Errors
    ///
    /// A poisoned backend.
    fn transmit<'c>(
        &'c self,
        permit: OperationPermit<'c, 'id, Self::OperationPath>,
        transmission: SupportedTransmit<'_, Self::Caps>,
    ) -> Result<Work<'c, Self>, Poisoned<Self::Fault>>
    where
        'id: 'c;

    /// Measure a channel's energy; the permit stays with the backend until
    /// it takes the scan's terminal.
    ///
    /// # Errors
    ///
    /// A poisoned backend.
    fn energy_scan<'c>(
        &'c self,
        permit: OperationPermit<'c, 'id, Self::OperationPath>,
        scan: SupportedEnergyScan<Self::Caps>,
    ) -> Result<Work<'c, Self>, Poisoned<Self::Fault>>
    where
        'id: 'c;

    /// Assess a channel; the permit stays with the backend until it takes
    /// the assessment's terminal.
    ///
    /// # Errors
    ///
    /// A poisoned backend.
    fn clear_channel_assessment<'c>(
        &'c self,
        permit: OperationPermit<'c, 'id, Self::OperationPath>,
        cca: SupportedCca<Self::Caps>,
    ) -> Result<Work<'c, Self>, Poisoned<Self::Fault>>
    where
        'id: 'c;
}

/// The recent-RSSI reader an install hands out beside its resources: one
/// writer (the backend) publishes the RSSI of the most recent reception,
/// and the reader takes a snapshot without a lock, whatever the port's
/// state, as OpenThread's synchronous `otPlatRadioGetRssi` needs.
pub trait RecentRssi {
    /// The RSSI in dBm of the most recent reception.
    fn recent_rssi(&self) -> i8;
}

/// The portable view of one event of a running phase. Each terminal names
/// the interface and the identity of the work it ends.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhaseEvent<'f> {
    /// A received frame; its metadata names the interface it matched.
    Received(ReceivedFrame<'f>),
    /// A transmission ended.
    TransmitDone {
        /// Whose transmission.
        interface: Interface,
        /// Its work's identity.
        id: RequestId,
        /// How it ended on the air.
        status: TxStatus,
        /// The acknowledgement received for it.
        acknowledgement: Option<ReceivedFrame<'f>>,
        /// The auxiliary security fields the radio wrote.
        security: Option<AppliedSecurity>,
    },
    /// An energy scan ended with its measurement in dBm.
    EnergyScanDone {
        /// Whose scan.
        interface: Interface,
        /// Its work's identity.
        id: RequestId,
        /// The energy in dBm.
        energy_dbm: i8,
    },
    /// An energy scan ended without a measurement.
    EnergyScanFailed {
        /// Whose scan.
        interface: Interface,
        /// Its work's identity.
        id: RequestId,
    },
    /// A clear-channel assessment ended.
    ClearChannelAssessmentDone {
        /// Whose assessment.
        interface: Interface,
        /// Its work's identity.
        id: RequestId,
        /// Whether the channel was idle.
        idle: bool,
    },
    /// A clear-channel assessment ended without a result.
    ClearChannelAssessmentFailed {
        /// Whose assessment.
        interface: Interface,
        /// Its work's identity.
        id: RequestId,
    },
}

impl PhaseEvent<'_> {
    /// The interface and identity of the work this terminal ends; `None`
    /// for a received frame.
    pub const fn ends(&self) -> Option<(Interface, RequestId)> {
        match *self {
            Self::Received(_) => None,
            Self::TransmitDone { interface, id, .. }
            | Self::EnergyScanDone { interface, id, .. }
            | Self::EnergyScanFailed { interface, id }
            | Self::ClearChannelAssessmentDone { interface, id, .. }
            | Self::ClearChannelAssessmentFailed { interface, id } => Some((interface, id)),
        }
    }
}

/// The one consumer of a running IEEE 802.15.4 phase's events: received
/// frames and the terminals of admitted work, read through the portable
/// view. A terminal's operation permit is back in its pool before the
/// event is handed out.
pub trait Ieee802154Events: PortPhase {
    /// The portable view of an owned event.
    fn view(event: &Self::Event) -> PhaseEvent<'_>;
}
