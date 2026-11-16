#![no_std]
#![forbid(unsafe_code)]
#![deny(missing_docs)]
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
    clippy::disallowed_macros,
    clippy::disallowed_methods
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
        clippy::disallowed_methods,
    )
)]

//! The IEEE 802.11 lower-MAC radio port.
//!
//! The port is the contract between portable Wi-Fi MAC logic and the
//! backend that puts frames on the air, whether a chip, a family driver or
//! a host model. One actor owns the radio as a [`DataPort`], the shared
//! physical owner, and runs the station lifecycle's [`PortCommand`]s and
//! the access point's [`AccessPointCommand`]s ([`AccessPointPort`]) in its
//! turns.
//! One submitted operation owns its context, ticket and MPDU until its
//! terminal. The backend executes the retry plan; a barrier waits for the
//! current publication alone. Portable code selects rates and describes
//! frames, while the backend numbers and publishes them. The services it
//! performs autonomously are its [`LowerMacCapabilities::services`].
//!
//! The values that cross the port (attempts, settings, key installs,
//! A-MPDU and beacon timing values, outcomes and the backend's service
//! description) live beside it ([`tx`], [`control`], [`extensions`],
//! [`contract`], [`monitor`]). Data frames go out on the two DMA arenas of
//! [`TxArena`] ([`arena`]). Codecs, policy, vendor records and chip
//! models live in the layers that own them.
//!
//! The PHY and channel values the port carries ([`Channel`], [`PhyRate`])
//! are the portable ones of `oer-protocols-ieee80211-wire`. This package only declares
//! the port and its values; it never waits on it.


pub mod access_point;
pub use access_point::{
    AccessPointCapabilities, AccessPointReceive, AccessPointServices, BEACON_ACCESS_CATEGORY,
    BeaconStamp, BeaconTimestamp, SupportedTsfControl, TsfControl,
};
pub mod capabilities;
pub mod control;
pub mod extensions;
pub mod he;
pub use he::{
    BssColorScope, BssColorUse, He20Association, He20AssociationError, HeBssColor, StationHe,
};
pub mod operation;
pub mod owner;
pub mod regulated;
pub mod types;
pub use regulated::RegulatedChannel;
pub mod rx;
pub mod tsf;
pub mod tx;

mod management;
pub use management::ManagementTxPlan;

// Vocabulary the port's signatures carry, so a runtime over the port names
// it without depending on the air formats.
pub use oer_protocols_ieee80211_wire::ccmp::GroupKeyId;
pub use oer_protocols_ieee80211_wire::channel::{
    Band, Channel, ChannelError, ChannelWidth, Ghz2_4Channel,
};
pub use oer_protocols_ieee80211_wire::phy::PhyRate;

/// The clock domain of the IEEE 802.11 lower-MAC port: its instants never
/// mix with another port's.
pub enum Ieee80211Radio {}

/// The installation's unique slot for the station's connection-epoch source.
/// A backend mints it once; Station consumes it to issue branded epochs.
/// The backend does not construct or interpret those protocol identities.
pub enum ConnectionEpochKind {}

/// An instant on the IEEE 802.11 lower-MAC port's clock.
pub type Ieee80211Instant = oer_portable_time::RadioInstant<Ieee80211Radio>;

/// A span of the port's radio clock.
pub type Ieee80211Duration = oer_portable_time::RadioDuration<Ieee80211Radio>;

/// An instant on the IEEE 802.11 lower-MAC port's clock with the generation
/// of its relation to the monotonic clock.
pub type Ieee80211Stamp = oer_protocols_radio_port::RadioStamp<Ieee80211Radio>;

/// A generation of the port's radio-clock relation to monotonic time.
pub type Ieee80211ClockGeneration =
    oer_protocols_radio_port::Generation<Ieee80211Instant, oer_portable_time::Instant>;

/// A paired reading of the IEEE 802.11 lower-MAC port's clock and the
/// monotonic clock.
pub type Ieee80211ClockSample = oer_protocols_radio_port::ClockSample<Ieee80211Radio>;

pub use capabilities::{
    BandSet, HardwareServices, LowerMacCapabilities, PhyFormatSet, RateSupport, Support, WidthSet,
};
pub use control::{ReceiveFilter, VifRole, VifRoleSet};
pub use extensions::{
    AmpduCapabilities, AmpduLengthTable, AmpduRateRefused, BeaconTimingCapabilities,
    MonitorCapabilities, TSF_DRIFT_PPM, TSF_RELATION, TbttEvent, TsfArithmeticError, TsfGeneration,
    TsfProjectionError, TsfSample, TsfVifMismatch, VifTsf,
};
pub use oer_protocols_radio_port::{
    ClockInfo, ClockRelation, EventsLost, Generation, GenerationSource, Poisoned, PortResult,
    Projected, ProjectionError, RelationRate,
};
pub use operation::{ConnectionFrame, Operation, TxOperation};
pub use owner::{
    AccessPoint, AckHeader, AmpduRate, BssColor, FrameHeader, GroupAddress, HeaderSpec,
    Ieee80211Capabilities, InterfaceRole, LegacyPortRate, MpduLength, MpduStartSpacing,
    NoAckHeader, PortRate, QosBody, QosControl, ReceiverAddress, ResponseHeader, ServicePeriod,
    SizedFrame, Station, StationReceive, SupportedBlockAck, SupportedChannel, SupportedEdca,
    SupportedFilter, SupportedNoAck, SupportedPower, Tid, TxFrame,
};
pub use rx::{
    LinkMetric, ReceiveOperation, RxCryptoStatus, RxEvidence, RxMeta, RxMetadata, SignalMetadata,
};
pub use tx::{
    AckSnr, AggregateEnd, AggregateOutcome, Air, BlockAckReport, BlockAckSnr, IndeterminateCause,
    Protection, RateSample, SingleEnd, SingleOutcome, SingleResponse, SnrSample, TxBody,
    TxEstimate, TxOutcome, TxPower, TxResponse, TxStatus, WithdrawCause,
};
pub use types::{ArbiterAuthority, ArbiterPool, GateClosed, GateIssuer, GateOpen, GatePool};

#[cfg(test)]
extern crate alloc;
#[cfg(test)]
extern crate std;

#[allow(
    missing_docs,
    reason = "preserve the carried service vocabulary's documentation coverage"
)]
pub mod contract;
#[allow(
    missing_docs,
    reason = "preserve the carried service vocabulary's documentation coverage"
)]
pub mod edca;
#[allow(
    missing_docs,
    reason = "preserve the carried service vocabulary's documentation coverage"
)]
pub mod monitor;

pub use contract::{
    MacInterfaceCapabilities, MacOperationOwner, MacOperationOwnership, MacResourceLimits,
    MacRxMetadata, MacServiceCapabilities, MacTxResult, MacTxStatus,
};
pub use edca::{EdcaAcParameters, EdcaParameterSet};
pub use monitor::{
    ChannelContextId, MonitorDropReason, MonitorFrame, MonitorInjectionBindingError,
    MonitorInjectionChannelBinding, MonitorInjectionRate, MonitorInjectionRequest,
    MonitorPublishOutcome, MonitorSink, MonitorTapPoint,
};

pub mod backend;

mod kept;
pub mod arena; // SCRATCH: named by core/datapath
pub use kept::{
    AdvanceRefused, ClassFrame, ClassSizedFrame, LayoutError, PreparationError, TxBufferClass, LendRefusal, LendRefused, LentRxBuffer, MpduProtection, NumberedKey,
    RETRY_STAGES, ReceiveAccounting, RecycleRefused, RetryPlan, RetryStage, RxBuffer,
};

