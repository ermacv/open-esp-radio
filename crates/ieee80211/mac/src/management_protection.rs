//! Robust management frames and the SA Query procedure.
//!
//! An association that protects its management frames protects only its
//! robust ones: Deauthentication, Disassociation and Action frames of a
//! robust category. The SA Query Action frame lets a station confirm that its
//! access point still holds the association before it believes an
//! unprotected disconnect.
//!
//! SOURCE(esp32s31): complete pinned `libnet80211[ieee80211.o]::ieee80211_is_robust_mgmt_frm` and `libnet80211[ieee80211_sta.o]::ieee80211_is_action_category_robust`; IEEE 802.11-2016 9.6.10 (SA Query).

/// Management subtype byte (Frame Control byte zero) of Disassociation.
const DISASSOCIATION: u8 = 0xa0;
/// Management subtype byte of Deauthentication.
const DEAUTHENTICATION: u8 = 0xc0;
/// Management subtype byte of Action.
const ACTION: u8 = 0xd0;
/// Highest category the vendor classifies by table.
const LAST_TABLED_CATEGORY: u8 = 22;
/// Categories up to 22 the vendor treats as not robust: Public (4), HT (7),
/// Unprotected WNM (11), Self-protected (15), 20, 21 and 22.
const UNPROTECTED_CATEGORIES: u32 =
    (1 << 4) | (1 << 7) | (1 << 11) | (1 << 15) | (1 << 20) | (1 << 21) | (1 << 22);
/// Vendor-specific, the only category above 22 that is not robust.
const VENDOR_SPECIFIC_CATEGORY: u8 = 127;

/// SA Query Action category.
pub const SA_QUERY_CATEGORY: u8 = 8;
const SA_QUERY_REQUEST: u8 = 0;
const SA_QUERY_RESPONSE: u8 = 1;
/// SA Query Action body: category, action and transaction identifier.
pub const SA_QUERY_BODY_LEN: usize = 4;

/// Whether an Action frame of `category` is robust.
pub const fn is_robust_action_category(category: u8) -> bool {
    if category <= 3 {
        true
    } else if category <= LAST_TABLED_CATEGORY {
        UNPROTECTED_CATEGORIES >> category & 1 == 0
    } else {
        category != VENDOR_SPECIFIC_CATEGORY
    }
}

/// Whether a management frame is robust, from Frame Control byte zero and,
/// for an Action frame, its category.
pub const fn is_robust_management_frame(frame_control_type: u8, action_category: u8) -> bool {
    match frame_control_type {
        DISASSOCIATION | DEAUTHENTICATION => true,
        ACTION => is_robust_action_category(action_category),
        _ => false,
    }
}

/// One SA Query Action body.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SaQuery {
    Request { transaction: [u8; 2] },
    Response { transaction: [u8; 2] },
}

impl SaQuery {
    /// Parse an Action body; `None` for any other Action.
    pub fn parse(body: &[u8]) -> Option<Self> {
        let [SA_QUERY_CATEGORY, action, first, second, ..] = *body else {
            return None;
        };
        let transaction = [first, second];
        match action {
            SA_QUERY_REQUEST => Some(Self::Request { transaction }),
            SA_QUERY_RESPONSE => Some(Self::Response { transaction }),
            _ => None,
        }
    }

    pub const fn encode(self) -> [u8; SA_QUERY_BODY_LEN] {
        let (action, transaction) = match self {
            Self::Request { transaction } => (SA_QUERY_REQUEST, transaction),
            Self::Response { transaction } => (SA_QUERY_RESPONSE, transaction),
        };
        [SA_QUERY_CATEGORY, action, transaction[0], transaction[1]]
    }
}

#[cfg(test)]
mod tests;
