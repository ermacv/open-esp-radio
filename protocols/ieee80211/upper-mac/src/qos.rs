//! Access-category admission of the MAC data service: how admission control
//! downgrades an access category and which user priority a downgraded frame
//! takes. The priority and category values are `oer_ieee80211_wire`'s.

use oer_ieee80211_wire::qos::{WmmAccessCategory, WmmUserPriority};

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
