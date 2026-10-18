//! Exact first AP RX protocol failure, retained independently of text logging.
use postcard_schema::Schema;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub enum WifiRxRejectionReason {
    DataTruncated,
    DataNotData,
    DataFragmented,
    DataRoleMismatch,
    DataAmsduUnsupported,
    DataInvalidLlcSnap,
    DataOutputTooSmall {
        required: u32,
    },
    PeerQosMismatch,
    PairwiseKeyId {
        observed: u8,
    },
    KeyGenerationMismatch,
    ReplayInvalidTid,
    Replay {
        packet_number: u64,
        highest: u64,
    },
    ReplayStaleCandidate,
    ReplayRevisionExhausted,
    FragmentTruncated,
    FragmentNotData,
    FragmentNotFragmented,
    FragmentProtected,
    FragmentUnprotected,
    FragmentOrderedUnsupported,
    FragmentRoleMismatch,
    FragmentInvalidReceiver,
    FragmentInvalidDestination,
    FragmentInvalidTransmitter,
    FragmentAmsduUnsupported,
    FragmentEmptyPayload,
    FragmentClockUnavailable,
    FragmentNoReassemblyContexts,
    FragmentOrphan {
        fragment_number: u8,
    },
    FragmentIdentityMismatch,
    FragmentMoreFragmentsMismatch,
    FragmentRetryPacketNumberMismatch {
        fragment_number: u8,
        expected: u64,
        observed: u64,
    },
    FragmentRetryPayloadMismatch {
        fragment_number: u8,
    },
    FragmentPacketNumberNotIncreasing {
        previous: u64,
        observed: u64,
    },
    FragmentOutOfOrder {
        expected: u8,
        observed: u8,
    },
    FragmentTooManyFragments,
    FragmentReassembledTooLarge {
        capacity: u32,
    },
    FragmentInvalidLlcSnap,
    ReorderStorageExhausted,
    ReorderFrameTooLong,
    DeferredOutputCapacity,
    InPlaceOutputUnsupported,
}

/// Radio monotonic time of rejection (possibly a retained reorder release).
/// Header fields are optional because a malformed MPDU may lack them. This
/// record contains no payload, encryption key or inferred delivery status.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct WifiRxRejection {
    pub reason: WifiRxRejectionReason,
    pub at_micros: u64,
    pub transmitter: Option<[u8; 6]>,
    pub frame_control: Option<u16>,
    pub sequence_control: Option<u16>,
    pub tid: Option<u8>,
    pub key_id: Option<u8>,
    pub packet_number: Option<u64>,
    pub mpdu_length: u32,
}
