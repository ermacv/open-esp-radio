//! Allocation-free data-fragment reassembly.
//!
//! Fragmentation is an MSDU transform, not a receive-BlockAck sequence
//! space. This owner therefore accepts only individually delivered data MPDUs
//! and binds every retained byte to the complete Sequence Control, QoS/TID,
//! three-address identity and protection class. CCMP fragments additionally
//! retain the exact per-fragment packet number and authenticated plaintext so
//! Retry can suppress only a byte-identical retransmission. Replay-frontier
//! ownership remains outside this module: a caller commits each new PN only
//! after the corresponding fragment has reached this bounded owner.

// Parsing establishes the shared identity and protection contract below.
// Reassembly retains one complete slot/admission owner and cannot release a
// partial MSDU. Both remain inside this fragmentation domain.
mod parsing;
mod reassembly;

pub use parsing::{
    parse_ccmp_data_fragment, parse_ccmp_data_identity, parse_open_data_fragment,
    parse_open_data_identity,
};
pub use reassembly::{
    OpenDataDefragmentation, OpenDataDefragmenter, OpenDataFragmentAdmission,
    OpenDataFragmentPreflight, OpenDataUnfragmentedAdmission, OpenReassembledData,
};

use crate::ccmp::{CcmpKeyId, CcmpPacketNumber};
use crate::data::{DataInterfaceRole, LLC_SNAP_HEADER_LEN};
use crate::sequence::SequenceNumber;

const DATA: u16 = 0x0008;
const QOS_DATA: u16 = 0x0088;
const TYPE_AND_SUBTYPE: u16 = 0x00fc;
const TO_DS: u16 = 0x0100;
const FROM_DS: u16 = 0x0200;
const MORE_FRAGMENTS: u16 = 0x0400;
const RETRY: u16 = 0x0800;
const PROTECTED: u16 = 0x4000;
const ORDER: u16 = 0x8000;
const QOS_AMSDU_PRESENT: u8 = 0x80;

/// Payload bytes retained for one ordinary 1,500-byte Ethernet payload plus
/// its LLC/SNAP header. The Ethernet header is reconstructed from the exact
/// admitted 802.11 address tuple and therefore consumes no retained bytes.
pub const OPEN_DATA_REASSEMBLY_CAPACITY: usize = 1_500 + LLC_SNAP_HEADER_LEN;

/// Software resource lifetime for an incomplete Open MSDU.
///
/// This is deliberately not presented as an ESP32-S31 hardware value. It is
/// a finite host-owned eviction policy used only when a runtime clock sample
/// accompanies the received fragment.
pub const OPEN_DATA_FRAGMENT_TIMEOUT: oer_time::Duration = oer_time::Duration::from_secs(1);

/// Cryptographic class bound to one complete fragment identity.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DataFragmentProtection {
    Open,
    Ccmp { key_id: CcmpKeyId },
}

/// Exact identity shared by every fragment of one Open data MSDU.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct OpenDataFragmentIdentity {
    role: DataInterfaceRole,
    protection: DataFragmentProtection,
    receiver_address: [u8; 6],
    transmitter_address: [u8; 6],
    address3: [u8; 6],
    sequence_number: SequenceNumber,
    qos_control: Option<u16>,
}

impl OpenDataFragmentIdentity {
    pub const fn role(self) -> DataInterfaceRole {
        self.role
    }

    pub const fn protection(self) -> DataFragmentProtection {
        self.protection
    }

    pub const fn receiver_address(self) -> [u8; 6] {
        self.receiver_address
    }

    pub const fn transmitter_address(self) -> [u8; 6] {
        self.transmitter_address
    }

    pub const fn address3(self) -> [u8; 6] {
        self.address3
    }

    pub const fn sequence_number(self) -> SequenceNumber {
        self.sequence_number
    }

    pub const fn tid(self) -> Option<u8> {
        match self.qos_control {
            Some(control) => Some((control & 0x000f) as u8),
            None => None,
        }
    }

    pub const fn destination(self) -> [u8; 6] {
        match self.role {
            DataInterfaceRole::Station => self.receiver_address,
            DataInterfaceRole::AccessPoint => self.address3,
        }
    }

    pub const fn source(self) -> [u8; 6] {
        match self.role {
            DataInterfaceRole::Station => self.address3,
            DataInterfaceRole::AccessPoint => self.transmitter_address,
        }
    }

    fn same_sequence_space(self, other: Self) -> bool {
        self.role == other.role
            && self.protection == other.protection
            && self.transmitter_address == other.transmitter_address
            && self.sequence_number == other.sequence_number
            && self.tid() == other.tid()
    }
}

/// One strictly parsed Open-network data fragment.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct OpenDataFragment<'frame> {
    identity: OpenDataFragmentIdentity,
    sequence_control: u16,
    fragment_number: u8,
    more_fragments: bool,
    retry: bool,
    packet_number: Option<CcmpPacketNumber>,
    payload: &'frame [u8],
}

impl<'frame> OpenDataFragment<'frame> {
    pub const fn identity(self) -> OpenDataFragmentIdentity {
        self.identity
    }

    pub const fn sequence_control(self) -> u16 {
        self.sequence_control
    }

    pub const fn fragment_number(self) -> u8 {
        self.fragment_number
    }

    pub const fn more_fragments(self) -> bool {
        self.more_fragments
    }

    pub const fn retry(self) -> bool {
        self.retry
    }

    pub const fn packet_number(self) -> Option<CcmpPacketNumber> {
        self.packet_number
    }

    pub const fn payload(self) -> &'frame [u8] {
        self.payload
    }
}

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
    ClockUnavailable,
    NoReassemblyContexts,
    Orphan {
        fragment_number: u8,
    },
    IdentityMismatch,
    MoreFragmentsMismatch,
    RetryPacketNumberMismatch {
        fragment_number: u8,
        expected: CcmpPacketNumber,
        observed: CcmpPacketNumber,
    },
    RetryPayloadMismatch {
        fragment_number: u8,
    },
    PacketNumberNotIncreasing {
        previous: CcmpPacketNumber,
        observed: CcmpPacketNumber,
    },
    OutOfOrder {
        expected: u8,
        observed: u8,
    },
    TooManyFragments,
    ReassembledTooLarge {
        capacity: usize,
    },
    InvalidLlcSnap,
}

#[cfg(test)]
mod tests;
