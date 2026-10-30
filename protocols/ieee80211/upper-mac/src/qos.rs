//! Access-category admission of the MAC data service: how admission control
//! downgrades an access category and which user priority a downgraded frame
//! takes. The priority and category values are `oer_ieee80211_wire`'s.

use oer_ieee80211_wire::extensions::wmm::WmmParameterSet;
use oer_ieee80211_wire::qos::{WmmAccessCategory, WmmUserPriority};

/// The access categories a station without admissions may not send QoS data
/// in: those whose admission control is mandatory.
///
/// The station negotiates no admission (no ADDTS), so an access category
/// whose ACM subfield is set is unavailable to its QoS data, which takes the
/// next lower available category.
///
/// SOURCE: IEEE Std 802.11-2020, EDCA admission control: the ACM subfield of
/// an access category's AC Parameter Record; WMM Specification, admission
/// control (an unadmitted station uses a lower access category).
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct AcAdmission {
    /// Restricted categories in ACI order: BE, BK, VI, VO.
    restricted: [bool; 4],
}

impl AcAdmission {
    /// No access category requires an admission.
    pub const OPEN: Self = Self {
        restricted: [false; 4],
    };

    /// The restrictions of the EDCA parameters a station contends with.
    pub const fn from_edca(edca: WmmParameterSet) -> Self {
        Self {
            restricted: [
                edca.access_category(WmmAccessCategory::BestEffort)
                    .admission_control_mandatory,
                edca.access_category(WmmAccessCategory::Background)
                    .admission_control_mandatory,
                edca.access_category(WmmAccessCategory::Video)
                    .admission_control_mandatory,
                edca.access_category(WmmAccessCategory::Voice)
                    .admission_control_mandatory,
            ],
        }
    }

    /// Whether `category` is unavailable without an admission.
    pub const fn restricts(self, category: WmmAccessCategory) -> bool {
        *category.select(&self.restricted)
    }

    /// The user priority a frame requesting `requested` takes: the requested
    /// one when its access category is available, else the canonical
    /// priority of the next lower available category; `None` when no lower
    /// category is available either.
    pub const fn admitted_user_priority(
        self,
        requested: WmmUserPriority,
    ) -> Option<WmmUserPriority> {
        let mut category = requested.access_category();
        if !self.restricts(category) {
            return Some(requested);
        }
        loop {
            category = match downgrade(category) {
                Some(lower) => lower,
                None => return None,
            };
            if !self.restricts(category) {
                return Some(canonical_user_priority(category));
            }
        }
    }
}

/// Next lower access category used when the requested AC requires an
/// admission which the station does not own.
pub const fn downgrade(category: WmmAccessCategory) -> Option<WmmAccessCategory> {
    match category {
        WmmAccessCategory::Voice => Some(WmmAccessCategory::Video),
        WmmAccessCategory::Video => Some(WmmAccessCategory::BestEffort),
        WmmAccessCategory::BestEffort => Some(WmmAccessCategory::Background),
        WmmAccessCategory::Background => None,
    }
}

/// Canonical user priority used when ACM policy downgrades into `category`.
pub const fn canonical_user_priority(category: WmmAccessCategory) -> WmmUserPriority {
    match category {
        WmmAccessCategory::Voice => WmmUserPriority::UP7,
        // The recovered station classifier enters VI at UP5.
        WmmAccessCategory::Video => WmmUserPriority::UP5,
        WmmAccessCategory::BestEffort => WmmUserPriority::UP0,
        WmmAccessCategory::Background => WmmUserPriority::UP1,
    }
}

#[cfg(test)]
mod tests;
