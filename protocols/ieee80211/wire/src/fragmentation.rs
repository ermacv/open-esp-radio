//! Data-fragment parsing, identity and protection values.
//!
//! These codecs describe one individually delivered data MPDU, including its
//! complete Sequence Control, QoS/TID, address tuple and protection class.
//! The upper-MAC core owns fragment retention, timeout and replay admission.

mod parsing;

use crate::qos::Tid;
use oer_protocols_ieee802_wire::Eui48;
pub use parsing::{
    parse_ccmp_data_fragment, parse_ccmp_data_identity, parse_open_data_fragment,
    parse_open_data_identity,
};

use crate::ccmp::CcmpPacketNumber;
use crate::data::DataInterfaceRole;
use crate::sequence::SequenceNumber;

/// Cryptographic class bound to one complete fragment identity.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DataFragmentProtection {
    Open,
    Ccmp,
}

/// Exact identity shared by every fragment of one Open data MSDU.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct OpenDataFragmentIdentity {
    role: DataInterfaceRole,
    protection: DataFragmentProtection,
    receiver_address: Eui48,
    transmitter_address: Eui48,
    address3: Eui48,
    sequence_number: SequenceNumber,
    qos_control: Option<crate::frame::QosControl>,
}

impl OpenDataFragmentIdentity {
    pub const fn role(self) -> DataInterfaceRole {
        self.role
    }

    pub const fn protection(self) -> DataFragmentProtection {
        self.protection
    }

    pub const fn receiver_address(self) -> Eui48 {
        self.receiver_address
    }

    pub const fn transmitter_address(self) -> Eui48 {
        self.transmitter_address
    }

    pub const fn address3(self) -> Eui48 {
        self.address3
    }

    pub const fn sequence_number(self) -> SequenceNumber {
        self.sequence_number
    }

    pub const fn tid(self) -> Option<Tid> {
        match self.qos_control {
            Some(control) => Some(control.tid()),
            None => None,
        }
    }

    pub const fn destination(self) -> Eui48 {
        match self.role {
            DataInterfaceRole::Station => self.receiver_address,
            DataInterfaceRole::AccessPoint => self.address3,
        }
    }

    pub const fn source(self) -> Eui48 {
        match self.role {
            DataInterfaceRole::Station => self.address3,
            DataInterfaceRole::AccessPoint => self.transmitter_address,
        }
    }

    pub fn same_sequence_space(self, other: Self) -> bool {
        self.role == other.role
            && self.protection == other.protection
            && self.transmitter_address == other.transmitter_address
            && self.sequence_number == other.sequence_number
            && self.tid() == other.tid()
    }
}

/// One strictly parsed Open-network data fragment: its Frame Control and
/// Sequence Control fields, and what the parse checked of the rest.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct OpenDataFragment<'frame> {
    identity: OpenDataFragmentIdentity,
    frame_control: crate::frame::FrameControlFields,
    sequence_control: crate::sequence::SequenceControl,
    packet_number: Option<CcmpPacketNumber>,
    payload: &'frame [u8],
}

impl<'frame> OpenDataFragment<'frame> {
    pub const fn identity(self) -> OpenDataFragmentIdentity {
        self.identity
    }

    pub const fn sequence_control(self) -> crate::sequence::SequenceControl {
        self.sequence_control
    }

    pub const fn fragment_number(self) -> u8 {
        self.sequence_control.fragment()
    }

    pub const fn more_fragments(self) -> bool {
        self.frame_control.more_fragments()
    }

    pub const fn retry(self) -> bool {
        self.frame_control.retry()
    }

    pub const fn packet_number(self) -> Option<CcmpPacketNumber> {
        self.packet_number
    }

    pub const fn payload(self) -> &'frame [u8] {
        self.payload
    }
}

/// Why a received data MPDU is not an admissible fragment of an Open or
/// CCMP data MSDU: the frame-format refusals of one MPDU.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OpenDataFragmentError {
    Truncated,
    NotData,
    NotFragmented,
    Protected,
    Unprotected,
    OrderedUnsupported,
    RoleMismatch,
    InvalidReceiver,
    InvalidDestination,
    InvalidTransmitter,
    AmsduUnsupported,
    EmptyPayload,
}
