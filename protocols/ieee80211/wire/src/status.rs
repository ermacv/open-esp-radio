//! Status and reason codes of management frames.

/// A Status Code: any number, some of them named.
///
/// SOURCE: IEEE Std 802.11-2020, 9.4.1.9, Table 9-50 (Status codes).
///
/// A number is no code until it is named one:
///
/// ```compile_fail,E0308
/// use oer_protocols_ieee80211_wire::status::StatusCode;
/// let code: StatusCode = 1;
/// ```
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(transparent)]
pub struct StatusCode(u16);

impl StatusCode {
    /// Successful.
    pub const SUCCESS: Self = Self(0);
    /// Unspecified failure.
    pub const UNSPECIFIED_FAILURE: Self = Self(1);
    /// Authentication is rejected because of challenge failure.
    pub const CHALLENGE_FAILURE: Self = Self(15);
    /// Association is denied because the AP is unable to handle additional
    /// associated STAs.
    pub const DENIED_NO_MORE_STAS: Self = Self(17);
    /// Association is denied because the requesting STA does not support
    /// all of the data rates in the BSSBasicRateSet.
    pub const DENIED_RATES: Self = Self(18);
    /// Association request is rejected temporarily; try again later.
    pub const REFUSED_TEMPORARILY: Self = Self(30);
    /// The request has been declined.
    pub const REQUEST_DECLINED: Self = Self(37);
    /// The request has not been successful as one or more parameters have
    /// invalid values.
    pub const INVALID_PARAMETERS: Self = Self(38);
    /// Invalid element: an element whose contents do not meet its
    /// specification.
    pub const INVALID_ELEMENT: Self = Self(40);
    /// Invalid PMKID.
    pub const INVALID_PMKID: Self = Self(53);
    /// Anti-clogging token required.
    pub const ANTI_CLOGGING_TOKEN_REQUIRED: Self = Self(76);
    /// Authentication is rejected because the offered finite cyclic group
    /// is not supported.
    pub const UNSUPPORTED_FINITE_CYCLIC_GROUP: Self = Self(77);
    /// SAE authentication uses the hash-to-element password element.
    pub const SAE_HASH_TO_ELEMENT: Self = Self(126);

    /// The code `value`, a named one or not.
    pub const fn new(value: u16) -> Self {
        Self(value)
    }

    /// Its number.
    pub const fn value(self) -> u16 {
        self.0
    }
}

/// A Reason Code: any number, some of them named.
///
/// SOURCE: IEEE Std 802.11-2020, 9.4.1.7, Table 9-49 (Reason codes).
///
/// A number is no code until it is named one:
///
/// ```compile_fail,E0308
/// use oer_protocols_ieee80211_wire::status::ReasonCode;
/// let code: ReasonCode = 1;
/// ```
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(transparent)]
pub struct ReasonCode(u16);

impl ReasonCode {
    /// Unspecified reason.
    pub const UNSPECIFIED: Self = Self(1);
    /// Deauthenticated because the sending STA is leaving (or has left) the
    /// IBSS or ESS.
    pub const DEAUTH_LEAVING: Self = Self(3);
    /// Class 2 frame received from a nonauthenticated STA.
    pub const INVALID_CLASS2_FRAME: Self = Self(6);
    /// Class 3 frame received from a nonassociated STA.
    pub const INVALID_CLASS3_FRAME: Self = Self(7);
    /// 4-way handshake timeout.
    pub const FOUR_WAY_HANDSHAKE_TIMEOUT: Self = Self(15);
    /// Requested from the peer STA due to timeout.
    pub const TIMEOUT: Self = Self(39);

    /// The code `value`, a named one or not.
    pub const fn new(value: u16) -> Self {
        Self(value)
    }

    /// Its number.
    pub const fn value(self) -> u16 {
        self.0
    }
}
