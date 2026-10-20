//! Station association request construction and response admission.

use super::*;
use crate::address::OwnAddress;

/// Relative HE UL-MU transmit-power capability for MAC rates 16 through 25.
///
/// The caller supplies calibrated Rust-owned PHY gain-table indices. No ROM
/// function, C ABI callback or vendor global is retained at this boundary.
///
/// SOURCE(esp32s31): complete `libnet80211.a[ieee80211_he.o]::
/// ieee80211_add_ulmu_pwrcap` queries `phy_get_max_pwr` for rates 16..=25,
/// subtracts every rate 17..=25 primary byte from rate 16, then writes the
/// nine differences after Extension ID 60 and two reserved zero bytes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HeUlMuPowerCapability {
    relative_to_rate_16: [u8; 9],
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HeUlMuPowerCapabilityError {
    HigherPowerThanRate16 { rate: u8 },
}

impl HeUlMuPowerCapability {
    pub fn from_rate_power_indices(
        rate_16_through_25: [i8; 10],
    ) -> Result<Self, HeUlMuPowerCapabilityError> {
        let base = i16::from(rate_16_through_25[0]);
        let mut relative_to_rate_16 = [0_u8; 9];
        for (offset, (relative, power)) in relative_to_rate_16
            .iter_mut()
            .zip(rate_16_through_25[1..].iter().copied())
            .enumerate()
        {
            let difference = base - i16::from(power);
            if !(0..=i16::from(u8::MAX)).contains(&difference) {
                return Err(HeUlMuPowerCapabilityError::HigherPowerThanRate16 {
                    rate: 17 + offset as u8,
                });
            }
            *relative = difference as u8;
        }
        Ok(Self {
            relative_to_rate_16,
        })
    }

    pub const fn relative_to_rate_16(self) -> [u8; 9] {
        self.relative_to_rate_16
    }

    fn encode(self) -> [u8; HE_UL_MU_POWER_CAPABILITY_IE_LEN] {
        let mut element = [0_u8; HE_UL_MU_POWER_CAPABILITY_IE_LEN];
        element[0] = 255;
        element[1] = 12;
        element[2] = HE_UL_MU_POWER_CAPABILITY_EXTENSION_ID;
        element[3..12].copy_from_slice(&self.relative_to_rate_16);
        element
    }
}

/// Minimum and maximum transmit power advertised by an HE STA, in dBm.
///
/// SOURCE(esp32s31): complete `libnet80211.a[ieee80211_he.o]::
/// ieee80211_add_power_cap` writes Element ID 33, the result of
/// `hal_get_tx_min_pwr`, and `hal_get_tx_pwr(16, 1)`. Complete
/// `ieee80211_assoc_req_construct` emits this element immediately after RSN
/// and before Extended Supported Rates whenever the HE cipher path is active.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StaPowerCapability {
    minimum_dbm: i8,
    maximum_dbm: i8,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StaPowerCapabilityError {
    MinimumAboveMaximum,
}

impl StaPowerCapability {
    pub const fn new(minimum_dbm: i8, maximum_dbm: i8) -> Result<Self, StaPowerCapabilityError> {
        if minimum_dbm > maximum_dbm {
            return Err(StaPowerCapabilityError::MinimumAboveMaximum);
        }
        Ok(Self {
            minimum_dbm,
            maximum_dbm,
        })
    }

    pub const fn minimum_dbm(self) -> i8 {
        self.minimum_dbm
    }

    pub const fn maximum_dbm(self) -> i8 {
        self.maximum_dbm
    }

    fn encode(self) -> [u8; POWER_CAPABILITY_IE_LEN] {
        [33, 2, self.minimum_dbm as u8, self.maximum_dbm as u8]
    }
}

/// Complete local capability elements supplied by the hardware/role profile.
///
/// The portable encoder owns peer admission, security selection and IE order.
/// The caller owns which local capabilities can be advertised; this type has
/// no default chip profile and does not derive capabilities from peer claims.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AssociationCapabilities {
    pub ht20: [u8; 28],
    pub ht40: [u8; 28],
    pub he20_ht: [u8; 28],
    pub he20: [u8; 24],
    pub he20_extended: [u8; 14],
    pub wmm: [u8; 9],
}

/// The capability elements selected by the caller's association policy.
pub struct AssociationElements<'a> {
    /// The complete local HT Capability element when selected.
    pub ht_capability: Option<&'a [u8; 28]>,
    /// The complete local HE Capability element when selected.
    pub he_capability: Option<&'a [u8; 24]>,
    /// The local HE transmit-power capability when required.
    pub power_capability: Option<StaPowerCapability>,
    /// The local HE UL-MU capability when required.
    pub he_ul_mu_power: Option<HeUlMuPowerCapability>,
}

/// Stateless selection of the capability elements a request serializes.
/// The port supplies its negotiated mode without an upward wire dependency.
pub trait AssociationPhy {
    /// Select the admitted local elements after fixed request validation.
    fn elements<'a>(
        &self,
        access_point: &ScanRecord,
        capabilities: &'a AssociationCapabilities,
        power_capability: Option<StaPowerCapability>,
        he_ul_mu_power: Option<HeUlMuPowerCapability>,
    ) -> Result<AssociationElements<'a>, AssociationRequestError>;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AssociationRequest<'a, P> {
    pub source: OwnAddress,
    pub access_point: &'a ScanRecord,
    pub sequence_number: SequenceNumber,
    pub listen_interval: u16,
    pub phy: P,
    /// The security elements this station selected for the access point.
    pub security: &'a SelectedRsn,
    /// HE Power Capability derived from the same calibrated rate-16 power
    /// source used by the MAC. Non-HE modes must leave it absent.
    pub power_capability: Option<StaPowerCapability>,
    /// Runtime-calibrated UL-MU power capability required by the complete HE
    /// association contract. Non-HE modes must leave it absent.
    pub he_ul_mu_power: Option<HeUlMuPowerCapability>,
}

impl<P: AssociationPhy> AssociationRequest<'_, P> {
    pub fn encode(
        self,
        output: &mut [u8],
        capabilities: &AssociationCapabilities,
    ) -> Result<usize, AssociationRequestError> {
        validate_peer(self.access_point.bssid.get()).map_err(AssociationRequestError::Frame)?;
        let ssid = self.access_point.ssid_bytes();
        if ssid.len() > MAX_SSID_LEN {
            return Err(AssociationRequestError::Frame(
                StationFrameError::SsidTooLong,
            ));
        }

        let rates_len = self.access_point.supported_rates_bytes().len()
            + self.access_point.extended_supported_rates_bytes().len();
        if rates_len == 0 {
            return Err(AssociationRequestError::Frame(
                StationFrameError::NoSupportedRates,
            ));
        }
        if rates_len > MAX_SUPPORTED_RATES_LEN {
            return Err(AssociationRequestError::Frame(
                StationFrameError::TooManySupportedRates,
            ));
        }
        let first_rates_len = rates_len.min(SUPPORTED_RATES_ELEMENT_CAPACITY);
        let extended_rates_len = rates_len - first_rates_len;
        let selected_rsn = self.security;
        let AssociationElements {
            ht_capability,
            he_capability,
            power_capability,
            he_ul_mu_power,
        } = self.phy.elements(
            self.access_point,
            capabilities,
            self.power_capability,
            self.he_ul_mu_power,
        )?;
        let mut fixed = [0; MANAGEMENT_HEADER_LEN + ASSOCIATION_FIXED_BODY_LEN];
        write_management_header(
            &mut fixed,
            ASSOCIATION_REQUEST_FRAME_CONTROL,
            self.access_point.bssid.get(),
            self.source.get(),
            self.access_point.bssid.get(),
            self.sequence_number,
        );
        // Privacy comes from the selected security mode, not an untrusted
        // scan record, even if that record bypassed candidate admission.
        let capability = ((self.access_point.capability_info & ASSOCIATION_CAPABILITY_MASK) | 1)
            & !0x0010
            | match self.security.security().link_protection() {
                LinkProtection::Open => 0,
                LinkProtection::Ccmp => 0x0010,
            };
        fixed[24..26].copy_from_slice(&capability.to_le_bytes());
        fixed[26..28].copy_from_slice(&self.listen_interval.to_le_bytes());
        let mut rates = [0; MAX_SUPPORTED_RATES_LEN];
        copy_rates(self.access_point, &mut rates[..rates_len]);
        let extended_header = [50, extended_rates_len as u8];
        let power_capability = power_capability.map(|value| value.encode());
        let he_ul_mu_power = he_ul_mu_power.map(|value| value.encode());
        // SOURCE(esp32s31): complete `libnet80211.a[ieee80211_output.o]::
        // ieee80211_assoc_req_construct` appends Extended Supported Rates
        // only after the selected RSN and the HE Power Capability.
        crate::encoding::FrameParts([
            fixed.as_slice(),
            &[0, ssid.len() as u8],
            ssid,
            &[1, first_rates_len as u8],
            &rates[..first_rates_len],
            selected_rsn.as_bytes(),
            power_capability
                .as_ref()
                .map_or(&[], |value| value.as_slice()),
            if extended_rates_len == 0 {
                &[]
            } else {
                &extended_header
            },
            &rates[first_rates_len..rates_len],
            ht_capability.map_or(&[], |value| value.as_slice()),
            he_capability.map_or(&[], |value| value.as_slice()),
            he_ul_mu_power
                .as_ref()
                .map_or(&[], |value| value.as_slice()),
            if ht_capability.is_some() {
                &capabilities.wmm
            } else {
                &[]
            },
            if he_capability.is_some() {
                &capabilities.he20_extended
            } else {
                &[]
            },
        ])
        .write(output)
        .map_err(|required| {
            AssociationRequestError::Frame(StationFrameError::OutputTooSmall { required })
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AssociationRequestError {
    Frame(StationFrameError),
    HtUnsupportedByAccessPoint,
    Ht40UnsupportedByAccessPoint,
    He20UnsupportedByAccessPoint,
    MissingPowerCapability,
    MissingHeUlMuPowerCapability,
    UnexpectedPowerCapability,
    UnexpectedHeUlMuPowerCapability,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AssociationResponse {
    pub capability_info: u16,
    pub status_code: u16,
    pub association_id: u16,
    pub ht_capability: bool,
    pub he_capability: bool,
    pub he_operation: bool,
    pub wmm: bool,
    pub wmm_parameters: Option<WmmParameterSet>,
    /// Association comeback time in TUs from a Timeout Interval element of
    /// type 3; an access point that refuses a protected association
    /// temporarily (status 30) names it.
    pub association_comeback_tu: Option<u32>,
}

impl AssociationResponse {
    /// Match the AP's successful response to the exact security mode selected
    /// from scan admission. There is no fallback between Open and WPA2.
    pub const fn matches_security(self, security: LinkProtection) -> bool {
        let privacy = self.capability_info & 0x0010 != 0;
        match security {
            LinkProtection::Open => !privacy,
            LinkProtection::Ccmp => privacy,
        }
    }
}

#[deny(
    clippy::indexing_slicing,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unreachable,
    clippy::arithmetic_side_effects
)]
pub fn parse_association_response(
    frame: &[u8],
    local: [u8; 6],
    bssid: [u8; 6],
) -> Option<AssociationResponse> {
    let (header, [capability_info, status_code, association_id], information_elements) =
        split_received_management(frame)?;
    if header.frame_control & 0x00fc != ASSOCIATION_RESPONSE_FRAME_CONTROL
        || header.receiver != local
        || header.transmitter != bssid
        || header.bssid != bssid
    {
        return None;
    }

    let mut ht_capability = false;
    let mut he_capability = false;
    let mut he_operation = false;
    let mut wmm = false;
    let mut wmm_parameters = None;
    let mut association_comeback_tu = None;
    let mut remaining = information_elements;
    while let [id, length, ..] = *remaining {
        let length = usize::from(length);
        let (element, tail) = remaining.split_at_checked(length.checked_add(2)?)?;
        ht_capability |= id == 45 && length == 26;
        he_capability |= matches!(element, [255, _, 35, ..]);
        he_operation |= matches!(element, [255, _, 36, ..]);
        let is_wmm = length >= 6 && matches!(element, [221, _, 0x00, 0x50, 0xf2, 0x02, ..]);
        wmm |= is_wmm;
        if is_wmm {
            wmm_parameters = parse_wmm_parameter_element(element).or(wmm_parameters);
        }
        if let [
            TIMEOUT_INTERVAL_ELEMENT_ID,
            _,
            TIMEOUT_INTERVAL_ASSOCIATION_COMEBACK,
            a,
            b,
            c,
            d,
            ..,
        ] = *element
        {
            let comeback = u32::from_le_bytes([a, b, c, d]);
            association_comeback_tu = (comeback != 0).then_some(comeback);
        }
        remaining = tail;
    }

    Some(AssociationResponse {
        capability_info,
        status_code,
        association_id: association_id & 0x3fff,
        ht_capability,
        he_capability,
        he_operation,
        // An AP that returned HT Capability accepted the WMM/QoS data path,
        // even if a bounded RX prefix omitted a later vendor WMM element.
        wmm: wmm || ht_capability || he_capability,
        wmm_parameters,
        association_comeback_tu,
    })
}

/// Timeout Interval element identifier.
const TIMEOUT_INTERVAL_ELEMENT_ID: u8 = 56;
/// Timeout Interval type of the association comeback time.
const TIMEOUT_INTERVAL_ASSOCIATION_COMEBACK: u8 = 3;

#[deny(
    clippy::indexing_slicing,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unreachable,
    clippy::arithmetic_side_effects
)]
fn copy_rates(access_point: &ScanRecord, output: &mut [u8]) {
    let rates = access_point
        .supported_rates_bytes()
        .iter()
        .chain(access_point.extended_supported_rates_bytes());
    for (destination, &rate) in output.iter_mut().zip(rates) {
        *destination = rate;
    }
}
