//! Translate driver-owned failures to the target-neutral HIL wire contract.
use oer_esp32s31_ieee80211_runtime::roles::access_point::{
    AccessPointRxRejection, AccessPointRxRejectionReason as DriverReason,
};
use oer_hil_protocol::{wifi::WifiRxRejection, wifi::WifiRxRejectionReason as Reason};
use oer_ieee80211_mac::{
    ccmp::CcmpReplayError, data::DataDecapError, fragmentation::OpenDataFragmentError,
};

pub fn evidence(record: AccessPointRxRejection) -> WifiRxRejection {
    let reason = match record.reason {
        DriverReason::Data(error) => match error {
            DataDecapError::Truncated => Reason::DataTruncated,
            DataDecapError::NotData => Reason::DataNotData,
            DataDecapError::Fragmented => Reason::DataFragmented,
            DataDecapError::RoleMismatch => Reason::DataRoleMismatch,
            DataDecapError::AmsduUnsupported => Reason::DataAmsduUnsupported,
            DataDecapError::InvalidLlcSnap => Reason::DataInvalidLlcSnap,
            DataDecapError::OutputTooSmall { required } => Reason::DataOutputTooSmall {
                required: required.try_into().unwrap_or(u32::MAX),
            },
        },
        DriverReason::PeerQosMismatch => Reason::PeerQosMismatch,
        DriverReason::PairwiseKeyId(observed) => Reason::PairwiseKeyId { observed },
        DriverReason::KeyGenerationMismatch => Reason::KeyGenerationMismatch,
        DriverReason::Replay(error) => match error {
            CcmpReplayError::InvalidTid => Reason::ReplayInvalidTid,
            CcmpReplayError::StaleCandidate => Reason::ReplayStaleCandidate,
            CcmpReplayError::RevisionExhausted => Reason::ReplayRevisionExhausted,
            CcmpReplayError::Replayed {
                packet_number,
                highest,
            } => Reason::Replay {
                packet_number: packet_number.value(),
                highest: highest.value(),
            },
        },
        DriverReason::Fragment(error) => match error {
            OpenDataFragmentError::Truncated => Reason::FragmentTruncated,
            OpenDataFragmentError::NotData => Reason::FragmentNotData,
            OpenDataFragmentError::NotFragmented => Reason::FragmentNotFragmented,
            OpenDataFragmentError::Protected => Reason::FragmentProtected,
            OpenDataFragmentError::Unprotected => Reason::FragmentUnprotected,
            OpenDataFragmentError::OrderedUnsupported => Reason::FragmentOrderedUnsupported,
            OpenDataFragmentError::RoleMismatch => Reason::FragmentRoleMismatch,
            OpenDataFragmentError::InvalidReceiver => Reason::FragmentInvalidReceiver,
            OpenDataFragmentError::InvalidDestination => Reason::FragmentInvalidDestination,
            OpenDataFragmentError::InvalidTransmitter => Reason::FragmentInvalidTransmitter,
            OpenDataFragmentError::AmsduUnsupported => Reason::FragmentAmsduUnsupported,
            OpenDataFragmentError::EmptyPayload => Reason::FragmentEmptyPayload,
            OpenDataFragmentError::ClockUnavailable => Reason::FragmentClockUnavailable,
            OpenDataFragmentError::NoReassemblyContexts => Reason::FragmentNoReassemblyContexts,
            OpenDataFragmentError::IdentityMismatch => Reason::FragmentIdentityMismatch,
            OpenDataFragmentError::MoreFragmentsMismatch => Reason::FragmentMoreFragmentsMismatch,
            OpenDataFragmentError::TooManyFragments => Reason::FragmentTooManyFragments,
            OpenDataFragmentError::InvalidLlcSnap => Reason::FragmentInvalidLlcSnap,
            OpenDataFragmentError::Orphan { fragment_number } => {
                Reason::FragmentOrphan { fragment_number }
            }
            OpenDataFragmentError::RetryPacketNumberMismatch {
                fragment_number,
                expected,
                observed,
            } => Reason::FragmentRetryPacketNumberMismatch {
                fragment_number,
                expected: expected.value(),
                observed: observed.value(),
            },
            OpenDataFragmentError::RetryPayloadMismatch { fragment_number } => {
                Reason::FragmentRetryPayloadMismatch { fragment_number }
            }
            OpenDataFragmentError::PacketNumberNotIncreasing { previous, observed } => {
                Reason::FragmentPacketNumberNotIncreasing {
                    previous: previous.value(),
                    observed: observed.value(),
                }
            }
            OpenDataFragmentError::OutOfOrder { expected, observed } => {
                Reason::FragmentOutOfOrder { expected, observed }
            }
            OpenDataFragmentError::ReassembledTooLarge { capacity } => {
                Reason::FragmentReassembledTooLarge {
                    capacity: capacity.try_into().unwrap_or(u32::MAX),
                }
            }
        },
        DriverReason::ReorderStorageExhausted => Reason::ReorderStorageExhausted,
        DriverReason::ReorderFrameTooLong => Reason::ReorderFrameTooLong,
        DriverReason::DeferredOutputCapacity => Reason::DeferredOutputCapacity,
        DriverReason::InPlaceOutputUnsupported => Reason::InPlaceOutputUnsupported,
    };
    WifiRxRejection {
        reason,
        at_micros: record.at_micros,
        transmitter: record.transmitter,
        frame_control: record.frame_control,
        sequence_control: record.sequence_control,
        tid: record.tid,
        key_id: record.key_id,
        packet_number: record.packet_number,
        mpdu_length: record.mpdu_length.try_into().unwrap_or(u32::MAX),
    }
}

#[cfg(test)]
mod tests;
