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
//! The port is the boundary between a chip's port and the sans-IO cores
//! (the installation machine, the engine), as
//! `design/port-contract-2026-11-16.md` of the notes repository sets it:
//! the radio's life ([`life`]: start, retune, stop, retire, release), the
//! data path ([`data`]: aggregates as places filled and published once, a
//! single a place a purpose, commands) and receive as a round
//! ([`receive`]). No call returns an error of the port and the contract
//! has no fault type; a call that needs a state takes its right, which
//! only this crate builds, and the port's facts carry its one indication,
//! that it needs a stop.
//!
//! The values that cross the port (rates, frames, settings, outcomes and
//! the backend's service description) live beside it ([`tx`],
//! [`control`], [`extensions`], [`contract`]). The PHY and channel values
//! it carries ([`Channel`], [`PhyRate`]) are the portable ones of
//! `oer-protocols-ieee80211-wire`. This package only declares the port and
//! its values; it never waits on it.

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
    BssColorScope, BssColorUse, HeAssociation, HeAssociationError, HeBssColor, StationHe,
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
    BandSet, FormatWidths, HardwareServices, LowerMacCapabilities, PhyFormat, PhyFormatSet,
    RateSupport, Support, Width, WidthSet,
};
pub use control::{ReceiveFilter, VifRole, VifRoleSet};
pub use extensions::{
    AmpduCapabilities, AmpduLengthTable, AmpduRateRefused, BeaconTimingCapabilities, TSF_DRIFT_PPM,
    TSF_RELATION, TbttEvent, TsfArithmeticError, TsfGeneration, TsfProjectionError, TsfSample,
    TsfVifMismatch, VifTsf,
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
    SizedFrame, Station, StationReceive, StorageTooShort, SupportedBlockAck, SupportedChannel,
    SupportedEdca, SupportedFilter, SupportedNoAck, SupportedPower, Tid, TxFrame,
};
pub use rx::{LinkMetric, RxCryptoStatus, RxEvidence, RxMeta, RxMetadata, SignalMetadata};
pub use tx::{
    AckSnr, AggregateEnd, AggregateOutcome, Air, BlockAckReport, BlockAckSnr, IndeterminateCause,
    Protection, RETRY_STAGES, RateSample, RetryPlan, RetryStage, SingleEnd, SingleOutcome,
    SingleResponse, SnrSample, TxBody, TxEstimate, TxOutcome, TxPower, TxResponse, TxStatus,
    WithdrawCause,
};
pub use types::{
    ArbiterAuthority, ArbiterPool, GateClosed, GateIssuer, GateLent, GateOpen, GatePool,
};

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
    ChannelContextId, MonitorFrame, MonitorInjectionBindingError, MonitorInjectionChannelBinding,
    MonitorInjectionRate, MonitorInjectionRequest, MonitorTapPoint,
};

pub mod backend;
pub mod data;
pub mod life;
pub mod receive;

pub use data::{
    Agreement, AgreementCommand, AgreementEnd, AgreementHeld, AgreementSlot, Arena, ArenaOutcome,
    ArenaSide, CloseGate, CommandPlace, CommandPoll, Complete, DATA_SINGLES, DataLayout, DataPort,
    DataRights, Ended, Facts, FrameSet, GroupCommand, GroupEnd, GroupHeld, GroupSlot, HEADROOM,
    HeaderLen, IdleArena, InCommand, InstalledGroup, InstalledPairwise, MIC_ROOM, OnAir,
    PairwiseCommand, PairwiseEnd, PairwiseHeld, PairwiseSlot, Place, Placing, PortSink, Ppdu,
    QUEUES, QueueRight, Settled, Signal, SinglePlace, TxFacts, TxSlot, TxSlotOf, TxSlotRefused,
    ValueCommand, ValueEnd, ValueKind, tx_slot,
};
pub use life::{
    Install, MintData, MintSeal, PortKey, PortPools, RadioPort, Retired, RetunePoll, Retuning,
    Running, StartFailure, StartPoll, Starting, StopFrom, StopPoll, StopRight, Stopped, Stopping,
    UnitsHome,
};
pub use receive::{
    EthernetAddresses, Lendable, OutsideTheUnit, PLACES, ReceivePort, Received, Rest, Round,
    RxHead, RxHeader, Staged, StagedPlaces, Unit, UnitHandle,
};
