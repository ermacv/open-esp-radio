//! Bounded parsing and encoding of the vendor-specific WMM Parameter Element.
//!
//! This extension retains the advertised AC parameters. Shared user-priority
//! values and traffic classification live in `qos`; parsing does not admit TX.

use crate::qos::WmmAccessCategory;

const VENDOR_ELEMENT_ID: u8 = 221;
const WMM_PARAMETER_BODY_LEN: u8 = 24;
const WMM_OUI_AND_TYPE: [u8; 4] = [0x00, 0x50, 0xf2, 0x02];
const WMM_PARAMETER_SUBTYPE: u8 = 1;
const WMM_VERSION: u8 = 1;
pub(crate) const WMM_PARAMETER_ELEMENT_LEN: usize = WMM_PARAMETER_BODY_LEN as usize + 2;
pub(crate) const WMM_NIBBLE_MAX: u8 = 0x0f;
const ACI_SHIFT: u32 = 5;
const ACI_MASK: u8 = 0x03;
const ADMISSION_CONTROL_MANDATORY: u8 = 0x10;
const ECW_MAX_SHIFT: u32 = 4;
const UAPSD: u8 = 0x80;
const ALL_ACCESS_CATEGORIES: u8 = 0b1111;

/// One four-byte AC Parameter Record from a WMM Parameter Element.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct WmmAcParameters {
    pub admission_control_mandatory: bool,
    pub aifsn: u8,
    pub ecw_min: u8,
    pub ecw_max: u8,
    /// TXOP limit in the element's native 32-us units.
    pub txop_limit_units_32_us: u16,
}

/// Complete WMM Parameter Set indexed by the standard ACI value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WmmParameterSet {
    pub parameter_set_count: u8,
    pub uapsd: bool,
    access_categories: [WmmAcParameters; 4],
}

impl WmmParameterSet {
    /// The set of `parameter_set_count` with one record per access
    /// category, indexed by ACI.
    pub const fn new(
        parameter_set_count: u8,
        uapsd: bool,
        access_categories: [WmmAcParameters; 4],
    ) -> Self {
        Self {
            parameter_set_count,
            uapsd,
            access_categories,
        }
    }

    pub const fn access_category(self, category: WmmAccessCategory) -> WmmAcParameters {
        *category.select(&self.access_categories)
    }
}

/// Encode the four records already validated by the AP advertisement.
/// Destructuring fixes every record's ACI and every output byte's place;
/// no numeric category index or offset calculation participates in encoding.
pub(crate) const fn encode_wmm_parameter_element(
    parameter_set_count: u8,
    uapsd: bool,
    categories: [WmmAcParameters; 4],
) -> [u8; WMM_PARAMETER_ELEMENT_LEN] {
    const fn record(category: WmmAccessCategory, parameters: WmmAcParameters) -> [u8; 4] {
        let admission = if parameters.admission_control_mandatory {
            ADMISSION_CONTROL_MANDATORY
        } else {
            0
        };
        let [low, high] = parameters.txop_limit_units_32_us.to_le_bytes();
        [
            ((category as u8) << ACI_SHIFT) | admission | parameters.aifsn,
            (parameters.ecw_max << ECW_MAX_SHIFT) | parameters.ecw_min,
            low,
            high,
        ]
    }
    let [be, bk, vi, vo] = categories;
    let [be0, be1, be2, be3] = record(WmmAccessCategory::BestEffort, be);
    let [bk0, bk1, bk2, bk3] = record(WmmAccessCategory::Background, bk);
    let [vi0, vi1, vi2, vi3] = record(WmmAccessCategory::Video, vi);
    let [vo0, vo1, vo2, vo3] = record(WmmAccessCategory::Voice, vo);
    let [oui0, oui1, oui2, kind] = WMM_OUI_AND_TYPE;
    [
        VENDOR_ELEMENT_ID,
        WMM_PARAMETER_BODY_LEN,
        oui0,
        oui1,
        oui2,
        kind,
        WMM_PARAMETER_SUBTYPE,
        WMM_VERSION,
        parameter_set_count | if uapsd { UAPSD } else { 0 },
        0, // Reserved octet before the AC records.
        be0,
        be1,
        be2,
        be3,
        bk0,
        bk1,
        bk2,
        bk3,
        vi0,
        vi1,
        vi2,
        vi3,
        vo0,
        vo1,
        vo2,
        vo3,
    ]
}

/// Parse one complete vendor-specific WMM Parameter Element.
///
/// SOURCE(esp32s31): complete
/// `libnet80211.a[ieee80211_sta.o]::ieee80211_parse_wmeparams`
/// (size `0xae`) and
/// `ieee80211_wme_standard_ac_to_esp_ac` (size `0x48`). The blob accepts a
/// 24-byte-or-larger body, reads QoS Info byte eight, then consumes four
/// four-byte records from byte ten. It maps the standard record order
/// BE/BK/VI/VO to its internal queue order 2/3/1/0 and extracts ACM, AIFSN,
/// ECWmin, ECWmax, and the low TXOP byte.
///
/// This hardware-independent parser keys records by their explicit ACI field
/// and retains the complete standard little-endian 16-bit TXOP value. The
/// blob's discarded high byte is an implementation limitation, not a
/// different on-air field.
pub fn parse_wmm_parameter_element(element: &[u8]) -> Option<WmmParameterSet> {
    let (fixed, _) = element.split_first_chunk::<WMM_PARAMETER_ELEMENT_LEN>()?;
    let &[
        id,
        length,
        oui0,
        oui1,
        oui2,
        kind,
        subtype,
        version,
        qos_info,
        _reserved,
        records @ ..,
    ] = fixed;
    if id != VENDOR_ELEMENT_ID
        || length < WMM_PARAMETER_BODY_LEN
        || element.len() < usize::from(length) + 2
        || [oui0, oui1, oui2, kind] != WMM_OUI_AND_TYPE
        || subtype != WMM_PARAMETER_SUBTYPE
        || version != WMM_VERSION
    {
        return None;
    }

    let mut access_categories = [WmmAcParameters::default(); 4];
    let mut seen = 0_u8;
    for &[aci_aifsn, ecw, txop_low, txop_high] in records.as_chunks::<4>().0 {
        let aci = (aci_aifsn >> ACI_SHIFT) & ACI_MASK;
        let category = WmmAccessCategory::from_aci(aci)?;
        let bit = 1_u8 << aci;
        if seen & bit != 0 {
            return None;
        }
        seen |= bit;
        *category.select_mut(&mut access_categories) = WmmAcParameters {
            admission_control_mandatory: aci_aifsn & ADMISSION_CONTROL_MANDATORY != 0,
            aifsn: aci_aifsn & WMM_NIBBLE_MAX,
            ecw_min: ecw & WMM_NIBBLE_MAX,
            ecw_max: ecw >> ECW_MAX_SHIFT,
            txop_limit_units_32_us: u16::from_le_bytes([txop_low, txop_high]),
        };
    }
    if seen != ALL_ACCESS_CATEGORIES {
        return None;
    }

    Some(WmmParameterSet {
        parameter_set_count: qos_info & WMM_NIBBLE_MAX,
        uapsd: qos_info & UAPSD != 0,
        access_categories,
    })
}

#[cfg(test)]
mod tests;
