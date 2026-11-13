//! The station's application side and the vocabulary its two tasks share.
//!
//! The station runs as two tasks on one executor: the lifecycle
//! ([`crate::lifecycle`]), which owns the station machine and its arbiter,
//! and the datapath actor ([`crate::datapath`]), which owns the radio's
//! data port. This module holds what they share with each other and with
//! the composition:
//!
//! - [`Control`], its [`State`] and [`Operation`]s: the application's sole
//!   command producer, its product status and the cooperative stop
//!   connection ([`RunnerControl`]) the lifecycle serves;
//! - the application's commands ([`ServiceCommand`]) and what the lifecycle
//!   tells the application ([`ServiceOutput`], through a
//!   [`ServiceOutputSink`]);
//! - the network's ends: the MSDUs it sends ([`Outgoing`]), the frames it
//!   receives lent in place or copied ([`Received`]), its stop
//!   ([`NetworkStop`]) and the wake topology of their queues
//!   ([`Notifies`]: [`SameHart`] or [`AcrossHarts`]);
//! - the events both tasks report to the composition's sink
//!   ([`StationEvents`]);
//! - the station's command credits, the admissions of the port operations
//!   the lifecycle waits for ([`PendingTerminal`]) and the places of its
//!   frames' contexts ([`TxContextStorage`]);
//! - the receive Block Ack owner the actor holds on the port's owners
//!   ([`RxBlockAckStorage`]);
//! - the optional reconnect policy ([`ReconnectPolicy`]).
//!
//! Commands and network frames travel through affine SPSC queues
//! (`oer_portable_memory::WakingSpscQueue`); each readiness future holds its
//! endpoint exclusively and consumes no value. Received network frames are
//! checked [`EthernetFrame`]s; a received payload too long for one is
//! refused, never clipped.
//!
//! [`EthernetFrame`]: oer_protocols_ieee80211_core_roles_sta::data::EthernetFrame

mod local_tx;
pub use local_tx::{LocalTx, LocalTxState, PoolRefused, TxPool};
mod commands;
pub mod contract;
mod events;
pub use events::{RxDrop, RxMsdusRefused, ServiceEvent, StationEvents};
mod control;
mod network;
mod network_link;
mod notifies;
pub use network::{
    CopiedMark, NetworkRx, NetworkStop, NetworkStopLost, NoNetwork, RX_COPIED_DEPTH, Received,
    RxMsdusCopied, StopPolicy,
};
pub use network_link::NetworkLink;
pub use notifies::{AcrossHarts, Notifies, SameHart};
pub(crate) mod reconnect;
pub(crate) use commands::CommandCredits;
pub use commands::{CommandRefused, CommandTerminal, Request};
pub use control::{
    Control, ControlCommand, ControlCreateError, ControlCreateRefused, ControlError,
    ControlRefused, Device, Events, Operation, OperationEnd, RunnerControl, State, Status,
    StatusReader, UnreadOperation,
};
pub use reconnect::{ReconnectPolicy, ZeroReconnectInterval};

use oer_protocols_ieee80211_core_roles_sta::data;

pub(crate) mod admissions;
mod rx_block_ack;
pub use admissions::{ControlTerminal, PendingTerminal};

pub use rx_block_ack::{PortRxBlockAck, RxBlockAckStorage, RxBlockAckView};

pub(crate) mod tx_contexts;
pub use tx_contexts::TxContextStorage;

/// What the application asks of the station.
#[derive(Debug, Eq, PartialEq)]
pub enum ServiceCommand<K> {
    /// A command of the station.
    Station(Request),
    /// A fact of the access point beside the station.
    AccessPoint(contract::AccessPointFact<K>),
    /// The access point beside the station is wanted.
    WantAccessPoint,
    /// Stop: the lifecycle stops the station and the radio, and the actor
    /// retires its installation.
    Stop,
}

/// What the station tells the application.
#[derive(Debug)]
pub enum ServiceOutput<K> {
    CommandStarted(Request),
    /// The original network operation refused before data admission.
    /// The packet itself went back to the network stack.
    MsduRefused {
        reason: contract::MsduRefusal,
    },
    /// A station frame refused before admission. Its resources return and
    /// the station continues; this is not a TX completion or radio
    /// statistic.
    TxRefused {
        /// The preparation failure.
        reason: contract::TransmitRefusal,
    },
    /// The station's notice.
    Notice(contract::Notice),
    /// A request refused synchronously, with its original correlation.
    CommandRefused(CommandRefused),
    /// The unique terminal of one admitted station request.
    CommandCompleted(CommandTerminal),
    /// The arbiter's command to the access point beside the station.
    AccessPoint(contract::AccessPointCommand<K>),
}

/// The port returned owners that cannot belong to an operation the
/// lifecycle sent.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BackendContractViolation {
    UnexpectedTerminal(PendingTerminal),
    UnexpectedAccessPointTerminal,
    GroupTerminalWithoutRequest,
    GroupSlotMismatch,
    GroupTerminalKindMismatch,
    /// A station frame's terminal returned a buffer of another class than
    /// the station's.
    TxBufferClass,
    /// The pairwise installation terminal returned a key of another
    /// installation; the outstanding request stays retained.
    ForeignPairwiseKey,
    /// A transmission's context found no place at its ticket's index when
    /// the port took it, or none waited there when its terminal came: the
    /// ticket pool is larger than the context storage, or a ticket came
    /// back twice.
    TxContextPlace(u8),
}

/// A concrete output recipient. Events carry their original owners by value.
pub trait ServiceOutputSink<K> {
    fn emit(&mut self, output: ServiceOutput<K>);
}

impl<K, F: FnMut(ServiceOutput<K>)> ServiceOutputSink<K> for F {
    fn emit(&mut self, output: ServiceOutput<K>) {
        self(output);
    }
}

/// The memory of one MSDU whose frame ended, on the air or before it, back
/// for the network. Its air was charged once, to its PPDU's queue, on the
/// MAC.
#[derive(Debug)]
pub struct TxEnded<M> {
    pub memory: M,
}

/// One MSDU the network stack hands the station: the network's memory its
/// frame is written in, and the user priority its marking requests.
#[derive(Debug)]
pub struct Outgoing<M> {
    /// The network's memory holding the frame, at the data plane's
    /// [`MSDU_ETHERNET_OFFSET`](data::MSDU_ETHERNET_OFFSET) of an arena
    /// slot's frame region.
    pub memory: M,
    /// The length of the Ethernet frame, header and payload.
    pub length: u16,
    pub priority: data::WmmUserPriority,
}
