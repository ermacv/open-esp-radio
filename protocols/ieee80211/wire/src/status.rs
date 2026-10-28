//! Status and reason codes of management frames.

/// Status Code field values (IEEE 802.11-2020 9.4.1.9).
///
/// SOURCE: IEEE Std 802.11-2020, 9.4.1.9, Table 9-50 (Status codes).
pub mod status_code {
    /// Successful.
    pub const SUCCESS: u16 = 0;
    /// Unspecified failure.
    pub const UNSPECIFIED_FAILURE: u16 = 1;
    /// Authentication is rejected because of challenge failure.
    pub const CHALLENGE_FAILURE: u16 = 15;
    /// Association is denied because the AP is unable to handle additional
    /// associated STAs.
    pub const DENIED_NO_MORE_STAS: u16 = 17;
    /// Association is denied because the requesting STA does not support
    /// all of the data rates in the BSSBasicRateSet.
    pub const DENIED_RATES: u16 = 18;
    /// Association request is rejected temporarily; try again later.
    pub const REFUSED_TEMPORARILY: u16 = 30;
    /// The request has been declined.
    pub const REQUEST_DECLINED: u16 = 37;
    /// The request has not been successful as one or more parameters have
    /// invalid values.
    pub const INVALID_PARAMETERS: u16 = 38;
    /// Invalid element: an element whose contents do not meet its
    /// specification.
    pub const INVALID_ELEMENT: u16 = 40;
    /// Invalid PMKID.
    pub const INVALID_PMKID: u16 = 53;
    /// Anti-clogging token required.
    pub const ANTI_CLOGGING_TOKEN_REQUIRED: u16 = 76;
    /// Authentication is rejected because the offered finite cyclic group
    /// is not supported.
    pub const UNSUPPORTED_FINITE_CYCLIC_GROUP: u16 = 77;
    /// SAE authentication uses the hash-to-element password element.
    pub const SAE_HASH_TO_ELEMENT: u16 = 126;
}

/// Reason Code field values (IEEE 802.11-2020 9.4.1.7).
///
/// SOURCE: IEEE Std 802.11-2020, 9.4.1.7, Table 9-49 (Reason codes).
pub mod reason_code {
    /// Unspecified reason.
    pub const UNSPECIFIED: u16 = 1;
    /// Deauthenticated because the sending STA is leaving (or has left) the
    /// IBSS or ESS.
    pub const DEAUTH_LEAVING: u16 = 3;
    /// 4-way handshake timeout.
    pub const FOUR_WAY_HANDSHAKE_TIMEOUT: u16 = 15;
    /// Requested from the peer STA due to timeout.
    pub const TIMEOUT: u16 = 39;
}
