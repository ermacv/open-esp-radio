#![no_std]
#![forbid(unsafe_code)]
#![deny(missing_docs)]
#![cfg_attr(
    not(feature = "test-support"),
    doc = r"
The shared backend conformance suite requires `test-support`:

```compile_fail,E0432
use oer_ieee80211_port::conformance;
```
"
)]
#![deny(
    clippy::let_underscore_must_use,
    clippy::let_underscore_untyped,
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::unreachable,
    clippy::todo,
    clippy::unimplemented,
    clippy::disallowed_macros
)]
#![cfg_attr(
    test,
    allow(
        clippy::let_underscore_must_use,
        clippy::let_underscore_untyped,
        clippy::arithmetic_side_effects,
        clippy::indexing_slicing,
        clippy::panic,
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::unreachable,
        clippy::todo,
        clippy::unimplemented,
        clippy::disallowed_macros,
    )
)]

//! The IEEE 802.11 lower-MAC radio port.
//!
//! The port is the contract between portable Wi-Fi MAC logic and the
//! backend that puts frames on the air, whether a chip, a family driver or
//! a host model, on the owner shape of `oer-radio-port`: a
//! [`LowerMacBackend`] runs under its owner and lends a [`LowerMacPhase`]
//! with one event stream and role-specific interface phases. Disabled-only
//! settings are a [`LowerMacSetup`]. One submitted operation owns its context,
//! ticket and MPDU until its terminal. The backend executes the retry plan;
//! a lifecycle barrier waits for the current publication alone. Portable
//! code selects rates and describes frames, while the backend numbers and
//! publishes them. The services it performs autonomously are its
//! [`LowerMacCapabilities::services`].
//!
//! The values its backends' cores take (attempts, settings, key installs,
//! A-MPDU and beacon timing values) live beside it ([`tx`], [`control`],
//! [`extensions`]).
//!
//! The PHY and channel values the port carries ([`Channel`], [`PhyRate`])
//! are the portable ones of `oer-ieee80211-wire`. This package only declares
//! the port and its values; it never waits on it.

mod in_flight;
pub use in_flight::{
    Completed, ExecutionPayload, InFlight, TerminalAccess, TerminalParts, TransmitPool, TxMpdu,
};

pub mod association;
pub mod capabilities;
/// Backend conformance cases and observation checks, without an allocator.
#[cfg(feature = "test-support")]
pub mod conformance;
mod layout;
pub use layout::{
    CheckedAggregate, CheckedSubframe, CheckedTxSource, FreshSource, FreshSubframe, HeaderLayout,
    LaidOut, LayoutError, LayoutRefused, MpduKind, MpduLayout, MpduProtection,
};
pub mod control;
pub mod extensions;
pub mod operation;
pub mod owner;
pub mod phase;
pub mod rx;
pub mod tx;

mod management;
pub use management::ManagementTxPlan;

pub use oer_ieee80211_wire::channel::{Band, Channel, ChannelError, ChannelWidth};
pub use oer_ieee80211_wire::phy::PhyRate;

/// The clock domain of the IEEE 802.11 lower-MAC port: its instants never
/// mix with another port's.
pub enum Ieee80211Radio {}

/// The installation's unique slot for the station's connection-epoch source.
/// A backend mints it once; Station consumes it to issue branded epochs.
/// The backend does not construct or interpret those protocol identities.
pub enum ConnectionEpochKind {}

/// An instant on the IEEE 802.11 lower-MAC port's clock.
pub type Ieee80211Instant = oer_time::RadioInstant<Ieee80211Radio>;

/// A span of the port's radio clock.
pub type Ieee80211Duration = oer_time::RadioDuration<Ieee80211Radio>;

/// An instant on the IEEE 802.11 lower-MAC port's clock with the generation
/// of its relation to the monotonic clock.
pub type Ieee80211Stamp = oer_radio_port::RadioStamp<Ieee80211Radio>;

/// A generation of the port's radio-clock relation to monotonic time.
pub type Ieee80211ClockGeneration = oer_radio_port::Generation<Ieee80211Instant, oer_time::Instant>;

/// A paired reading of the IEEE 802.11 lower-MAC port's clock and the
/// monotonic clock.
pub type Ieee80211ClockSample = oer_radio_port::ClockSample<Ieee80211Radio>;

pub use capabilities::{
    BandSet, HardwareServices, LowerMacCapabilities, PhyFormatSet, RateSupport, WidthSet,
};
pub use control::{ReceiveFilter, VifRole, VifRoleSet};
pub use extensions::{
    AmpduCapabilities, BeaconTimingCapabilities, MonitorCapabilities, TSF_DRIFT_PPM, TSF_RELATION,
    TbttEvent, TsfArithmeticError, TsfGeneration, TsfProjectionError, TsfRelation, TsfSample,
    TsfSetKind, TsfTimingError, TsfVifMismatch, VifTsf,
};
pub use oer_radio_port::{
    ClockInfo, ClockRelation, EventsLost, Generation, GenerationSource, Poisoned, PortResult,
    Projected, ProjectionError, RelationRate,
};
pub use operation::{ConnectionFrame, Operation, TxOperation};
pub use owner::{
    AccessPoint, AckHeader, AddressSetup, BssColor, FrameHeader, GroupAddress, HeaderSpec,
    Ieee80211Capabilities, InterfaceRole, LegacyPhaseRate, LowerMacBackend, LowerMacSetup,
    MpduLength, NoAckHeader, PhaseRate, QosAckPolicy, QosControl, ReceiverAddress, ResponseHeader,
    SizedFrame, Station, StationReceive, SupportedBlockAck, SupportedChannel, SupportedEdca,
    SupportedFilter, SupportedNoAck, SupportedPower, Tid, TxFrame,
};
pub use owner::{
    AdmittedAggregate, AggregateAttempt, AggregatePushResult, AggregateRefused, AggregateState,
    AirReservationPhase, AmpduPhase, AmpduRate, BeaconTimingPhase, BeaconWindow, ClassSubframe,
    EmptyAggregate, InterfaceAggregate, InterfaceAggregateAttempt, InterfaceAggregateSubmission,
    InterfaceProtection, MAX_RESERVATION_MICROS, MonitorPhase, MpduStartSpacing, NonEmptyAggregate,
    NonEmptyState, PhaseAggregate, PoolSubframe, SubframeSource, TbttPlan,
};
mod interface_commands;
pub use interface_commands::{
    ConfigurePermission, Configured, InterfaceCommandPool, InterfacePermissions,
    SampledTsfPermission, TsfPermission,
};
pub use phase::{
    AccessPointPhase, AdmittedAttempt, AggregateCompleted, Attempt, AttemptKey, CheckedPoolSource,
    ClassFrame, ClassSizedFrame, GateClosed, GateIssuer, GateOpen, GatePool, InstalledGroup,
    InterfaceAttempt, InterfaceKeys, InterfacePhase, InterfaceSubmission, KeyTypes, LowerMacEvent,
    LowerMacPhase, LowerMacTerminal, PhaseAggregateOutcome, PhaseBlockAckSlot, PhaseSingleOutcome,
    PhaseTypes, PoolMpdu, PoolPreparation, PoolSource, PreparationError, PreparationRefused,
    PreparedTx, RETRY_STAGES, Removed, RemovedKey, RetransmissionPlan, RetryPlan, RetryStage,
    ReturnedMpdu, RoleKeys, SingleCompleted, StationPhase, TxBufferClass, TxBufferMemory,
    TxBufferPool, TxCompleted, TxProtection, TxSource,
};
pub use rx::{
    LinkMetric, RxBuffer, RxCryptoStatus, RxEvidence, RxMeta, RxMetadata, SignalMetadata,
};
pub use tx::{
    AckSnr, AggregateEnd, AggregateOutcome, Air, BlockAckReport, BlockAckSnr, IndeterminateCause,
    Protection, RateSample, SingleEnd, SingleOutcome, SingleResponse, SnrSample, TxBody, TxOutcome,
    TxPower, TxResponse, TxStatus, WithdrawCause,
};

#[cfg(test)]
extern crate alloc;
#[cfg(test)]
extern crate std;

pub use owner::CheckedPoolAggregate;

#[allow(
    missing_docs,
    reason = "preserve the carried service vocabulary's documentation coverage"
)]
#[warn(
    clippy::let_underscore_must_use,
    clippy::let_underscore_untyped,
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::unreachable,
    clippy::todo,
    clippy::unimplemented,
    clippy::disallowed_macros
)]
pub mod configuration;
#[allow(
    missing_docs,
    reason = "preserve the carried service vocabulary's documentation coverage"
)]
#[warn(
    clippy::let_underscore_must_use,
    clippy::let_underscore_untyped,
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::unreachable,
    clippy::todo,
    clippy::unimplemented,
    clippy::disallowed_macros
)]
pub mod contract;
#[allow(
    missing_docs,
    reason = "preserve the carried service vocabulary's documentation coverage"
)]
#[warn(
    clippy::let_underscore_must_use,
    clippy::let_underscore_untyped,
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::unreachable,
    clippy::todo,
    clippy::unimplemented,
    clippy::disallowed_macros
)]
pub mod edca;
#[allow(
    missing_docs,
    reason = "preserve the carried service vocabulary's documentation coverage"
)]
#[warn(
    clippy::let_underscore_must_use,
    clippy::let_underscore_untyped,
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::unreachable,
    clippy::todo,
    clippy::unimplemented,
    clippy::disallowed_macros
)]
pub mod interface;
#[allow(
    missing_docs,
    reason = "preserve the carried service vocabulary's documentation coverage"
)]
#[warn(
    clippy::let_underscore_must_use,
    clippy::let_underscore_untyped,
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::unreachable,
    clippy::todo,
    clippy::unimplemented,
    clippy::disallowed_macros
)]
pub mod monitor;
#[allow(
    missing_docs,
    reason = "preserve the carried service vocabulary's documentation coverage"
)]
#[warn(
    clippy::let_underscore_must_use,
    clippy::let_underscore_untyped,
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::unreachable,
    clippy::todo,
    clippy::unimplemented,
    clippy::disallowed_macros
)]
pub mod tx_cost;
#[allow(
    missing_docs,
    reason = "preserve the carried service vocabulary's documentation coverage"
)]
#[warn(
    clippy::let_underscore_must_use,
    clippy::let_underscore_untyped,
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::unreachable,
    clippy::todo,
    clippy::unimplemented,
    clippy::disallowed_macros
)]
pub mod tx_work;

pub use configuration::{
    WifiAccessPointConfig, WifiConfig, WifiConfigError, WifiMonitorConfig, WifiPlan,
    WifiStandaloneEspNowPlan, WifiStandaloneMonitorPlan, WifiStationConfig,
};
pub use contract::{
    MacAmpduTxResult, MacAmpduTxStatus, MacIndividualRetries, MacInterfaceCapabilities,
    MacOperationOwner, MacOperationOwnership, MacResourceLimits, MacRxMetadata,
    MacServiceCapabilities, MacTxPlan, MacTxQueueState, MacTxResult, MacTxStatus,
};
pub use edca::{BackoffEntropy, EdcaContention, draw_backoff};
pub use monitor::{
    MONITOR_CHANNEL_SEQUENCE_CAPACITY, MonitorChannelPolicy, MonitorChannelSequence,
    MonitorChannelSequenceError, MonitorDropReason, MonitorFilter, MonitorFrame, MonitorFrameType,
    MonitorFrameTypeMask, MonitorInjectionBindingError, MonitorInjectionChannelBinding,
    MonitorInjectionFrameError, MonitorInjectionFrameType, MonitorInjectionMpdu,
    MonitorInjectionRate, MonitorInjectionRequest, MonitorPublishOutcome, MonitorSink,
};
pub use tx_work::MacTxWork;

/// Association PHY mode and caller preference.
pub mod phy;
pub use phy::{PhyMode, Preference};

/// A literal test sequence number; panics outside twelve bits.
#[cfg(test)]
pub(crate) const fn seq(value: u16) -> oer_ieee80211_wire::sequence::SequenceNumber {
    match oer_ieee80211_wire::sequence::SequenceNumber::new(value) {
        Some(sequence) => sequence,
        None => panic!("test sequence number exceeds twelve bits"),
    }
}

pub mod espressif;

pub mod backend;
pub mod backend_protection;

pub mod backend_aggregate;

pub use phase::{NumberedKey, TxNumbering};
