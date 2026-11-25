//! Station association request construction and response admission.

use super::*;
use crate::address::OwnAddress;
use oer_portable_wire::PrefixLength;

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
        for (rate, (relative, power)) in (17_u8..=25).zip(
            relative_to_rate_16
                .iter_mut()
                .zip(rate_16_through_25[1..].iter().copied()),
        ) {
            let difference = base
                .checked_sub(i16::from(power))
                .ok_or(HeUlMuPowerCapabilityError::HigherPowerThanRate16 { rate })?;
            *relative = u8::try_from(difference)
                .map_err(|_| HeUlMuPowerCapabilityError::HigherPowerThanRate16 { rate })?;
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
        crate::encoding::write_array::<3, _, _>(&mut element, &self.relative_to_rate_16);
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
        [
            33,
            2,
            self.minimum_dbm.cast_unsigned(),
            self.maximum_dbm.cast_unsigned(),
        ]
    }
}

/// Complete local capability elements supplied by the hardware/role profile.
///
/// The portable encoder owns peer admission, security selection and IE order.
/// The caller owns which local capabilities can be advertised; this type has
/// no default chip profile and does not derive capabilities from peer claims.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AssociationCapabilities {
    /// The HT Capabilities element of an HT request, for each channel width.
    pub ht: HtCapabilityElements,
    /// The HT Capabilities element an HE request carries beside its HE
    /// Capabilities.
    pub ht_with_he: [u8; 28],
    /// The HE Capabilities element of an HE request.
    pub he: [u8; 24],
    /// The Extended Capabilities element an HE request carries after its
    /// WMM element; `None` sends none.
    pub he_extended: Option<[u8; 14]>,
    pub wmm: [u8; 9],
}

/// A station's HT Capabilities element for each channel width it requests
/// HT at: the element's Supported Channel Width Set and short guard
/// intervals follow the width (IEEE Std 802.11-2020, 9.4.2.55.2).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HtCapabilityElements {
    /// At 20 MHz.
    pub mhz20: [u8; 28],
    /// At 40 MHz.
    pub mhz40: [u8; 28],
}

/// The PHY elements a request carries, chosen with its access point
/// before the request is built: an HE request carries its power elements
/// by type, and a request of another format carries none.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AssociationPhyElements<'a> {
    /// Non-HT: no HT or HE element.
    NonHt,
    /// HT: its HT Capabilities.
    Ht { ht_capability: &'a [u8; 28] },
    /// HE: the HT Capabilities beside it, its HE Capabilities, the HE Power
    /// Capability and the HE UL-MU power capability.
    He {
        ht_capability: &'a [u8; 28],
        he_capability: &'a [u8; 24],
        power_capability: StaPowerCapability,
        he_ul_mu_power: HeUlMuPowerCapability,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AssociationRequest<'a> {
    pub source: OwnAddress,
    pub access_point: &'a ScanRecord,
    pub sequence_number: SequenceNumber,
    pub listen_interval: u16,
    /// The PHY elements chosen with the access point.
    pub phy: AssociationPhyElements<'a>,
    /// The security elements this station selected for the access point.
    pub security: &'a SelectedRsn,
}

impl AssociationRequest<'_> {
    pub fn encode(
        self,
        output: &mut [u8],
        capabilities: &AssociationCapabilities,
    ) -> Result<usize, AssociationRequestError> {
        validate_peer(self.access_point.bssid.get()).map_err(AssociationRequestError::Frame)?;
        let ssid = self.access_point.ssid_bytes();
        let ssid_len = PrefixLength::<MAX_SSID_LEN>::new(ssid.len()).ok_or(
            AssociationRequestError::Frame(StationFrameError::SsidTooLong),
        )?;

        let rates_len = self
            .access_point
            .supported_rates_bytes()
            .len()
            .checked_add(self.access_point.extended_supported_rates_bytes().len())
            .ok_or(AssociationRequestError::Frame(
                StationFrameError::OutputTooSmall {
                    required: usize::MAX,
                },
            ))?;
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
        let selected_rsn = self.security;
        let (ht_capability, he_capability, power_capability, he_ul_mu_power) = match self.phy {
            AssociationPhyElements::NonHt => (None, None, None, None),
            AssociationPhyElements::Ht { ht_capability } => (Some(ht_capability), None, None, None),
            AssociationPhyElements::He {
                ht_capability,
                he_capability,
                power_capability,
                he_ul_mu_power,
            } => (
                Some(ht_capability),
                Some(he_capability),
                Some(power_capability),
                Some(he_ul_mu_power),
            ),
        };
        let mut fixed = [0; MANAGEMENT_HEADER_LEN + ASSOCIATION_FIXED_BODY_LEN];
        crate::encoding::write_array::<0, _, _>(
            &mut fixed,
            &management_header(
                ASSOCIATION_REQUEST_FRAME_CONTROL,
                self.access_point.bssid.get(),
                self.source.get(),
                self.access_point.bssid.get(),
                self.sequence_number,
            ),
        );
        // Privacy comes from the selected security mode, not an untrusted
        // scan record, even if that record bypassed candidate admission.
        let advertised = self.access_point.capability_info;
        let capability = crate::management::CapabilityInformation::from_bytes([0; 2])
            .with_ess(true)
            .with_short_preamble(advertised.short_preamble())
            .with_short_slot(advertised.short_slot())
            .with_privacy(matches!(
                self.security.security().link_protection(),
                LinkProtection::Ccmp
            ));
        crate::encoding::write_array::<24, _, _>(&mut fixed, &capability.to_bytes());
        crate::encoding::write_array::<26, _, _>(
            &mut fixed,
            &crate::management::ListenIntervalField::of(self.listen_interval).to_bytes(),
        );
        let mut rates = [0; MAX_SUPPORTED_RATES_LEN];
        let rates = rates
            .get_mut(..rates_len)
            .ok_or(AssociationRequestError::Frame(
                StationFrameError::TooManySupportedRates,
            ))?;
        copy_rates(self.access_point, rates);
        let mut reader = oer_portable_wire::Reader::new(rates);
        let first_rates = reader.take(first_rates_len).map_err(|_| {
            AssociationRequestError::Frame(StationFrameError::TooManySupportedRates)
        })?;
        let extended_rates = reader.remaining();
        let too_many = AssociationRequestError::Frame(StationFrameError::TooManySupportedRates);
        let first_rates_len =
            PrefixLength::<SUPPORTED_RATES_ELEMENT_CAPACITY>::new(first_rates.len())
                .ok_or(too_many)?;
        let extended_rates_len =
            PrefixLength::<MAX_SUPPORTED_RATES_LEN>::new(extended_rates.len()).ok_or(too_many)?;
        let extended_header = [50, extended_rates_len.to_u8()];
        let power_capability = power_capability.map(|value| value.encode());
        let he_ul_mu_power = he_ul_mu_power.map(|value| value.encode());
        // SOURCE(esp32s31): complete `libnet80211.a[ieee80211_output.o]::
        // ieee80211_assoc_req_construct` appends Extended Supported Rates
        // only after the selected RSN and the HE Power Capability.
        crate::encoding::FrameParts([
            fixed.as_slice(),
            &[0, ssid_len.to_u8()],
            ssid,
            &[1, first_rates_len.to_u8()],
            first_rates,
            selected_rsn.as_bytes(),
            power_capability
                .as_ref()
                .map_or(&[], |value| value.as_slice()),
            if extended_rates.is_empty() {
                &[]
            } else {
                &extended_header
            },
            extended_rates,
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
            match (he_capability, &capabilities.he_extended) {
                (Some(_), Some(extended)) => extended.as_slice(),
                _ => &[],
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
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AssociationResponse {
    pub capability_info: crate::management::CapabilityInformation,
    pub status_code: u16,
    pub association_id: u16,
    /// An HT Capabilities element of its 26 octets.
    pub ht_capability: crate::scan::ElementPresence,
    /// The response's HE Capabilities and HE Operation elements.
    pub he: ResponseHeElements,
    /// A WMM vendor element (OUI 00:50:f2, type 2) of at least six octets.
    pub wmm_element: crate::scan::ElementPresence,
    pub wmm_parameters: Option<WmmParameterSet>,
    /// Association comeback time in TUs from a Timeout Interval element of
    /// type 3; an access point that refuses a protected association
    /// temporarily (status 30) names it.
    pub association_comeback_tu: Option<u32>,
}

impl AssociationResponse {
    /// Whether the access point accepted the WMM/QoS data path: a WMM
    /// element, or HT or HE Capabilities, as an AP that returned HT
    /// Capabilities accepted it even if a bounded RX prefix omitted a later
    /// vendor WMM element.
    pub fn wmm(&self) -> bool {
        self.wmm_element == crate::scan::ElementPresence::Present
            || self.ht_capability == crate::scan::ElementPresence::Present
            || !self.he.capability().is_empty()
    }
}

/// The HE Capabilities and HE Operation elements of an Association
/// Response, as received: each empty when the response carries none.
///
/// An element longer than the response keeps, or a second element of either
/// kind, makes the pair unusable ([`Self::unusable`]); the bytes kept are
/// then not the access point's whole statement.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ResponseHeElements {
    capability: [u8; crate::scan::HE_CAPABILITY_IE_CAPACITY],
    capability_len: oer_portable_wire::PrefixLength<{ crate::scan::HE_CAPABILITY_IE_CAPACITY }>,
    operation: [u8; crate::scan::HE_OPERATION_IE_CAPACITY],
    operation_len: oer_portable_wire::PrefixLength<{ crate::scan::HE_OPERATION_IE_CAPACITY }>,
    unusable: bool,
}

impl ResponseHeElements {
    /// A response without HE elements.
    pub const NONE: Self = Self {
        capability: [0; crate::scan::HE_CAPABILITY_IE_CAPACITY],
        capability_len: oer_portable_wire::PrefixLength::ZERO,
        operation: [0; crate::scan::HE_OPERATION_IE_CAPACITY],
        operation_len: oer_portable_wire::PrefixLength::ZERO,
        unusable: false,
    };

    /// The complete elements `capability` and `operation`, each empty for
    /// its absence; `None` when one does not fit.
    pub fn new(capability: &[u8], operation: &[u8]) -> Option<Self> {
        let mut elements = Self::NONE;
        elements.capability_len = crate::scan::store_value(&mut elements.capability, capability)?;
        elements.operation_len = crate::scan::store_value(&mut elements.operation, operation)?;
        Some(elements)
    }

    /// The complete HE Capabilities element; empty without one.
    pub fn capability(&self) -> &[u8] {
        self.capability_len.borrow(&self.capability)
    }

    /// The complete HE Operation element; empty without one.
    pub fn operation(&self) -> &[u8] {
        self.operation_len.borrow(&self.operation)
    }

    /// Whether either element was too long to keep or repeated.
    pub const fn unusable(&self) -> bool {
        self.unusable
    }

    /// Keep `element` (header included), the response's HE Capabilities or
    /// HE Operation by its Element ID Extension; mark the pair unusable for
    /// a repeated element or one that does not fit.
    fn record(&mut self, element: &[u8]) {
        use crate::he::{HE_CAPABILITIES_EXTENSION_ID, HE_OPERATION_EXTENSION_ID};
        let kept = match *element {
            [255, _, HE_CAPABILITIES_EXTENSION_ID, ..] if self.capability().is_empty() => {
                crate::scan::store_value(&mut self.capability, element)
                    .map(|length| self.capability_len = length)
            }
            [255, _, HE_OPERATION_EXTENSION_ID, ..] if self.operation().is_empty() => {
                crate::scan::store_value(&mut self.operation, element)
                    .map(|length| self.operation_len = length)
            }
            [
                255,
                _,
                HE_CAPABILITIES_EXTENSION_ID | HE_OPERATION_EXTENSION_ID,
                ..,
            ] => None,
            _ => Some(()),
        };
        if kept.is_none() {
            self.unusable = true;
        }
    }
}

impl AssociationResponse {
    /// Match the AP's successful response to the exact security mode selected
    /// from scan admission. There is no fallback between Open and WPA2.
    pub const fn matches_security(self, security: LinkProtection) -> bool {
        let privacy = self.capability_info.privacy();
        match security {
            LinkProtection::Open => !privacy,
            LinkProtection::Ccmp => privacy,
        }
    }
}

pub fn parse_association_response(
    frame: &[u8],
    local: [u8; 6],
    bssid: [u8; 6],
) -> Option<AssociationResponse> {
    let (header, [c0, c1, s0, s1, a0, a1], information_elements) =
        split_received_management(frame)?;
    let capability_info = crate::management::CapabilityInformation::from_bytes([c0, c1]);
    let status_code = crate::management::StatusCodeField::from_bytes([s0, s1]).code();
    let association_id = crate::management::AssociationId::from_bytes([a0, a1]);
    if header.frame_control.type_and_subtype()
        != ASSOCIATION_RESPONSE_FRAME_CONTROL.type_and_subtype()
        || header.receiver != local
        || header.transmitter != bssid
        || header.bssid != bssid
    {
        return None;
    }

    let mut ht_capability = crate::scan::ElementPresence::Absent;
    let mut he = ResponseHeElements::NONE;
    let mut wmm_element = crate::scan::ElementPresence::Absent;
    let mut wmm_parameters = None;
    let mut association_comeback_tu = None;
    let mut remaining = information_elements;
    while let [id, length, ..] = *remaining {
        let length = usize::from(length);
        let mut reader = oer_portable_wire::Reader::new(remaining);
        let element = reader.take(length.checked_add(2)?).ok()?;
        let tail = reader.remaining();
        if id == 45 && length == 26 {
            ht_capability = crate::scan::ElementPresence::Present;
        }
        he.record(element);
        let is_wmm = length >= 6 && matches!(element, [221, _, 0x00, 0x50, 0xf2, 0x02, ..]);
        if is_wmm {
            wmm_element = crate::scan::ElementPresence::Present;
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
            let comeback =
                crate::management::TimeoutIntervalValue::from_bytes([a, b, c, d]).value();
            association_comeback_tu = (comeback != 0).then_some(comeback);
        }
        remaining = tail;
    }

    Some(AssociationResponse {
        capability_info,
        status_code,
        association_id: association_id.aid(),
        ht_capability,
        he,
        wmm_element,
        wmm_parameters,
        association_comeback_tu,
    })
}

/// Timeout Interval element identifier.
const TIMEOUT_INTERVAL_ELEMENT_ID: u8 = 56;
/// Timeout Interval type of the association comeback time.
const TIMEOUT_INTERVAL_ASSOCIATION_COMEBACK: u8 = 3;

fn copy_rates(access_point: &ScanRecord, output: &mut [u8]) {
    let rates = access_point
        .supported_rates_bytes()
        .iter()
        .chain(access_point.extended_supported_rates_bytes());
    for (destination, &rate) in output.iter_mut().zip(rates) {
        *destination = rate;
    }
}

#[cfg(test)]
mod tests {
    use super::StaPowerCapability;

    /// IEEE Std 802.11-2020, 9.4.2.14 (Power Capability element): both
    /// powers are signed, in dBm.
    #[test]
    fn a_power_capability_carries_both_powers_in_two_s_complement() {
        let capability = StaPowerCapability::new(-2, 20).unwrap();
        assert_eq!(capability.encode(), [33, 2, 0xfe, 20]);
    }
}
