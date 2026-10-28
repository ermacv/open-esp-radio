//! Portable EDCA records of a VIF's access categories. A backend's queues
//! keep their contention window and draw their backoff below the lower-MAC
//! port.

use oer_ieee80211_wire::extensions::wmm::{WmmAcParameters, WmmParameterSet};
use oer_ieee80211_wire::qos::WmmAccessCategory;

/// An AC's advertised contention and locally configured MSDU lifetime.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EdcaAcParameters {
    /// Contention, admission-control and TXOP fields advertised on air.
    pub wmm: WmmAcParameters,
    /// Maximum retention from software admission.
    pub msdu_lifetime: oer_time::Duration,
}

/// A VIF's complete EDCA table. Lifetimes are local MIB attributes, not
/// fields of the on-air WMM Parameter Element.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EdcaParameterSet {
    wmm: WmmParameterSet,
    msdu_lifetimes: [oer_time::Duration; 4],
}

impl EdcaParameterSet {
    /// Default and largest standard MSDU retention time (500 TUs).
    // SOURCE: IEEE 802.11-2020 Annex C, dot11EDCATableMSDULifetime.
    pub const DEFAULT_MSDU_LIFETIME: oer_time::Duration =
        oer_time::Duration::from_micros(500 * 1024);

    /// Non-AP defaults before a BSS is known.
    // SOURCE: IEEE 802.11-2020 9.4.2.28, Table 9-155 (OFDM/ERP/HT/HE).
    pub const STANDARD_DEFAULT: Self = Self::from_wmm(WmmParameterSet::STANDARD_DEFAULT);

    /// Lifetimes in ACI order: BE, BK, VI, VO. SupportedEdca validates
    /// the whole table before it can be installed.
    pub const fn new(wmm: WmmParameterSet, msdu_lifetimes: [oer_time::Duration; 4]) -> Self {
        Self {
            wmm,
            msdu_lifetimes,
        }
    }

    /// Convert MIB lifetime values in TUs at the record boundary.
    pub const fn from_tu(wmm: WmmParameterSet, lifetimes_tu: [u16; 4]) -> Self {
        let [be, bk, vi, vo] = lifetimes_tu;
        const fn duration(tu: u16) -> oer_time::Duration {
            oer_time::Duration::from_micros((tu as u64) << 10)
        }
        Self::new(
            wmm,
            [duration(be), duration(bk), duration(vi), duration(vo)],
        )
    }

    /// Advertised contention with the standard local lifetime for each AC.
    pub const fn from_wmm(wmm: WmmParameterSet) -> Self {
        Self::new(wmm, [Self::DEFAULT_MSDU_LIFETIME; 4])
    }

    /// Advertised fields, excluding the local lifetime attributes.
    pub const fn wmm(self) -> WmmParameterSet {
        self.wmm
    }

    /// Complete record for this access category.
    pub const fn access_category(self, category: WmmAccessCategory) -> EdcaAcParameters {
        EdcaAcParameters {
            wmm: self.wmm.access_category(category),
            msdu_lifetime: *category.select(&self.msdu_lifetimes),
        }
    }
}

impl Default for EdcaParameterSet {
    fn default() -> Self {
        Self::STANDARD_DEFAULT
    }
}
