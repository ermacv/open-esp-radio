//! Bounded parsing and encoding of the vendor-specific WMM Parameter Element.
//!
//! This extension retains the advertised AC parameters. Shared user-priority
//! values and traffic classification live in `qos`; parsing does not admit TX.
//!
//! Received bytes reach this module; nothing here panics on them.
#![deny(
    clippy::indexing_slicing,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unreachable,
    clippy::arithmetic_side_effects
)]

use crate::qos::WmmAccessCategory;

const VENDOR_ELEMENT_ID: u8 = 221;
const WMM_PARAMETER_BODY_LEN: u8 = 24;
const WMM_OUI_AND_TYPE: [u8; 4] = [0x00, 0x50, 0xf2, 0x02];
const WMM_PARAMETER_SUBTYPE: u8 = 1;
const WMM_VERSION: u8 = 1;
pub(crate) const WMM_PARAMETER_ELEMENT_LEN: usize = WMM_PARAMETER_BODY_LEN as usize + 2;
pub(crate) const WMM_NIBBLE_MAX: u8 = 0x0f;
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

    /// The EDCA parameters of a QoS station whose access point advertises
    /// none: the default dot11EDCATable of IEEE 802.11-2020 Table 9-155
    /// for an OFDM, ERP, HT or HE PHY (aCWmin 15, aCWmax 1023), records in
    /// ACI order.
    pub const STANDARD_DEFAULT: Self = Self::new(
        0,
        false,
        [
            // AC_BE: aCWmin, aCWmax, AIFSN 3.
            WmmAcParameters::contention(3, 4, 10, 0),
            // AC_BK: aCWmin, aCWmax, AIFSN 7.
            WmmAcParameters::contention(7, 4, 10, 0),
            // AC_VI: (aCWmin+1)/2-1, aCWmin, AIFSN 2, TXOP 3.008 ms.
            WmmAcParameters::contention(2, 3, 4, 94),
            // AC_VO: (aCWmin+1)/4-1, (aCWmin+1)/2-1, AIFSN 2, TXOP 1.504 ms.
            WmmAcParameters::contention(2, 2, 3, 47),
        ],
    );

    /// The contention of a QoS station in a non-QoS BSS, which uses the
    /// DCF (IEEE 802.11-2020 10.3): every access category waits DIFS
    /// (AIFSN 2) with aCWmin 15 and aCWmax 1023 and holds no TXOP.
    pub const DCF: Self = Self::new(0, false, [WmmAcParameters::contention(2, 4, 10, 0); 4]);
}

impl WmmAcParameters {
    /// A record without admission control.
    const fn contention(aifsn: u8, ecw_min: u8, ecw_max: u8, txop_limit_units_32_us: u16) -> Self {
        Self {
            admission_control_mandatory: false,
            aifsn,
            ecw_min,
            ecw_max,
            txop_limit_units_32_us,
        }
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
        let fields = AcRecord::from_bytes([0; 4]).with_acm(parameters.admission_control_mandatory);
        let fields = crate::encoding::validated(fields.with_aifsn(parameters.aifsn));
        let fields = crate::encoding::validated(fields.with_aci(category as u8));
        let fields = crate::encoding::validated(fields.with_ecw_min(parameters.ecw_min));
        let fields = crate::encoding::validated(fields.with_ecw_max(parameters.ecw_max));
        crate::encoding::validated(fields.with_txop(parameters.txop_limit_units_32_us)).to_bytes()
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
        QosInfo::from_bytes([parameter_set_count])
            .with_uapsd(uapsd)
            .to_bytes()[0],
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
    let mut reader = oer_wire::Reader::new(element);
    let fixed = reader.array::<WMM_PARAMETER_ELEMENT_LEN>().ok()?;
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
        let fields = AcRecord::from_bytes([aci_aifsn, ecw, txop_low, txop_high]);
        let aci = fields.aci();
        let category = WmmAccessCategory::from_aci(aci)?;
        let bit = 1_u8 << aci;
        if seen & bit != 0 {
            return None;
        }
        seen |= bit;
        *category.select_mut(&mut access_categories) = WmmAcParameters {
            admission_control_mandatory: fields.acm(),
            aifsn: fields.aifsn(),
            ecw_min: fields.ecw_min(),
            ecw_max: fields.ecw_max(),
            txop_limit_units_32_us: fields.txop(),
        };
    }
    if seen != ALL_ACCESS_CATEGORIES {
        return None;
    }

    Some(WmmParameterSet {
        parameter_set_count: QosInfo::from_bytes([qos_info]).count(),
        uapsd: QosInfo::from_bytes([qos_info]).uapsd(),
        access_categories,
    })
}

oer_wire::bitfield! {
    /// SOURCE: IEEE Std 802.11-2020, 9.4.2.28 (EDCA Parameter Set element).
    pub struct AcRecord[4] (little) {
        /// SOURCE: IEEE Std 802.11-2020, 9.4.2.28 (EDCA Parameter Set element).
        pub aifsn, with_aifsn: u8 = 0..4;
        /// SOURCE: IEEE Std 802.11-2020, 9.4.2.28 (EDCA Parameter Set element).
        pub acm, with_acm: bool = 4..5;
        /// SOURCE: IEEE Std 802.11-2020, 9.4.2.28 (EDCA Parameter Set element).
        pub aci, with_aci: u8 = 5..7;
        /// SOURCE: IEEE Std 802.11-2020, 9.4.2.28 (EDCA Parameter Set element).
        pub ecw_min, with_ecw_min: u8 = 8..12;
        /// SOURCE: IEEE Std 802.11-2020, 9.4.2.28 (EDCA Parameter Set element).
        pub ecw_max, with_ecw_max: u8 = 12..16;
        /// SOURCE: IEEE Std 802.11-2020, 9.4.2.28 (EDCA Parameter Set element).
        pub txop, with_txop: u16 = 16..32;
    }
}
oer_wire::bitfield! {
    /// SOURCE: IEEE Std 802.11-2020, 9.4.1 (QoS Info field).
    pub struct QosInfo[1] (little) {
        /// SOURCE: IEEE Std 802.11-2020, 9.4.1 (QoS Info field).
        pub count, with_count: u8 = 0..4;
        /// SOURCE: IEEE Std 802.11-2020, 9.4.1 (QoS Info field).
        pub uapsd, with_uapsd: bool = 7..8;
    }
}

#[cfg(test)]
mod tests;
