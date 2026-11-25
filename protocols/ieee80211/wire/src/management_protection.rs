//! Robust management frames and the SA Query procedure.
//!
//! An association that protects its management frames protects only its
//! robust ones: Deauthentication, Disassociation and Action frames of a
//! robust category. The SA Query Action frame lets a station confirm that its
//! access point still holds the association before it believes an
//! unprotected disconnect.
//!
//! SOURCE(esp32s31): complete pinned `libnet80211[ieee80211.o]::ieee80211_is_robust_mgmt_frm` and `libnet80211[ieee80211_sta.o]::ieee80211_is_action_category_robust`; IEEE 802.11-2016 9.6.10 (SA Query).
//!
//! Received bytes reach this module; nothing here panics on them.

use crate::action::{ActionCategory, SaQueryAction};

/// Management subtype byte (Frame Control byte zero) of Disassociation.
const DISASSOCIATION: u8 = 0xa0;
/// Management subtype byte of Deauthentication.
const DEAUTHENTICATION: u8 = 0xc0;
/// Management subtype byte of Action.
const ACTION: u8 = 0xd0;
/// SA Query Action body: category, action and transaction identifier.
pub const SA_QUERY_BODY_LEN: usize = 4;

/// Whether a management frame is robust, from Frame Control byte zero and,
/// for an Action frame, its category.
pub const fn is_robust_management_frame(
    frame_control_type: u8,
    action_category: ActionCategory,
) -> bool {
    match frame_control_type {
        DISASSOCIATION | DEAUTHENTICATION => true,
        ACTION => action_category.is_robust(),
        _ => false,
    }
}

oer_portable_wire::bitfield! {
    /// What matches an SA Query's Request with its Response.
    ///
    /// SOURCE: IEEE Std 802.11-2016, 9.6.10 (SA Query).
    ///
    /// A number is no identifier until it is one's field:
    ///
    /// ```compile_fail,E0308
    /// use oer_protocols_ieee80211_wire::management_protection::SaQueryTransaction;
    /// let transaction: SaQueryTransaction = [0, 1];
    /// ```
    pub struct SaQueryTransaction[2] (little) {
        pub identifier, with_identifier: {u16} = 0..16;
    }
}

/// One SA Query Action body.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SaQuery {
    Request { transaction: SaQueryTransaction },
    Response { transaction: SaQueryTransaction },
}

impl SaQuery {
    /// Parse an Action body; `None` for any other Action.
    pub fn parse(body: &[u8]) -> Option<Self> {
        let [category, action, first, second, ..] = *body else {
            return None;
        };
        if ActionCategory::new(category) != ActionCategory::SA_QUERY {
            return None;
        }
        let transaction = SaQueryTransaction::from_bytes([first, second]);
        match SaQueryAction::new(action) {
            SaQueryAction::REQUEST => Some(Self::Request { transaction }),
            SaQueryAction::RESPONSE => Some(Self::Response { transaction }),
            _ => None,
        }
    }

    pub const fn encode(self) -> [u8; SA_QUERY_BODY_LEN] {
        let (action, transaction) = match self {
            Self::Request { transaction } => (SaQueryAction::REQUEST, transaction),
            Self::Response { transaction } => (SaQueryAction::RESPONSE, transaction),
        };
        let [first, second] = transaction.to_bytes();
        [
            ActionCategory::SA_QUERY.value(),
            action.value(),
            first,
            second,
        ]
    }
}

#[cfg(test)]
mod tests;
