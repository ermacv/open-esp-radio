//! Portable security-cause conversions shared by the image and host regressions.
use oer_hil_protocol::wifi::{StationEapolParseError, StationRsnFrameError, StationSaePwe};
use oer_ieee80211_rsn::EapolParseError;
use oer_ieee80211_wire::security::SaePwe;

pub(super) fn eapol(error: EapolParseError) -> StationEapolParseError {
    match error {
        EapolParseError::Truncated => StationEapolParseError::Truncated,
        EapolParseError::LengthMismatch => StationEapolParseError::LengthMismatch,
        EapolParseError::NotKeyPacket => StationEapolParseError::NotKeyPacket,
        EapolParseError::NotRsnKeyDescriptor => StationEapolParseError::NotRsnKeyDescriptor,
    }
}

pub(super) fn sae_pwe(pwe: SaePwe) -> StationSaePwe {
    match pwe {
        SaePwe::HuntingAndPecking => StationSaePwe::HuntingAndPecking,
        SaePwe::HashToElement => StationSaePwe::HashToElement,
    }
}

pub(super) fn retained_security_ies(
    error: oer_ieee80211_rsn::frames::RsnFrameError,
) -> StationRsnFrameError {
    use oer_ieee80211_rsn::frames::RsnFrameError as R;
    match error {
        R::CapacityExceeded => StationRsnFrameError::CapacityExceeded,
        R::InvalidRsnIe => StationRsnFrameError::InvalidRsnIe,
        R::InvalidRsnxe => StationRsnFrameError::InvalidRsnxe,
        R::InvalidKeyId => StationRsnFrameError::InvalidKeyId,
        R::ZeroNonce => StationRsnFrameError::ZeroNonce,
        R::EmptyKeyData => StationRsnFrameError::EmptyKeyData,
        R::MalformedKeyData => StationRsnFrameError::MalformedKeyData,
        R::UnsupportedKeyData => StationRsnFrameError::UnsupportedKeyData,
        R::DuplicateRsnIe => StationRsnFrameError::DuplicateRsnIe,
        R::DuplicateRsnxe => StationRsnFrameError::DuplicateRsnxe,
        R::DuplicateGtk => StationRsnFrameError::DuplicateGtk,
        R::DuplicateIgtk => StationRsnFrameError::DuplicateIgtk,
        R::MissingRsnIe => StationRsnFrameError::MissingRsnIe,
        R::MissingRsnxe => StationRsnFrameError::MissingRsnxe,
        R::UnexpectedRsnxe => StationRsnFrameError::UnexpectedRsnxe,
        R::MissingGtk => StationRsnFrameError::MissingGtk,
        R::MissingIgtk => StationRsnFrameError::MissingIgtk,
        R::UnexpectedIgtk => StationRsnFrameError::UnexpectedIgtk,
        R::RsnIeMismatch => StationRsnFrameError::RsnIeMismatch,
        R::RsnxeMismatch => StationRsnFrameError::RsnxeMismatch,
        R::UnexpectedTransmitAction => StationRsnFrameError::UnexpectedTransmitAction,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use oer_ieee80211_rsn::frames::RsnFrameError;

    #[test]
    fn local_security_failures_retain_their_sources_on_the_wire() {
        assert_eq!(
            retained_security_ies(RsnFrameError::InvalidRsnIe),
            StationRsnFrameError::InvalidRsnIe
        );
        assert_eq!(
            retained_security_ies(RsnFrameError::InvalidRsnxe),
            StationRsnFrameError::InvalidRsnxe
        );
        assert_eq!(
            sae_pwe(SaePwe::HuntingAndPecking),
            StationSaePwe::HuntingAndPecking
        );
        assert_eq!(sae_pwe(SaePwe::HashToElement), StationSaePwe::HashToElement);
        assert_eq!(
            eapol(EapolParseError::Truncated),
            StationEapolParseError::Truncated
        );
        assert_eq!(
            eapol(EapolParseError::LengthMismatch),
            StationEapolParseError::LengthMismatch
        );
        assert_eq!(
            eapol(EapolParseError::NotKeyPacket),
            StationEapolParseError::NotKeyPacket
        );
        assert_eq!(
            eapol(EapolParseError::NotRsnKeyDescriptor),
            StationEapolParseError::NotRsnKeyDescriptor
        );
    }
}
