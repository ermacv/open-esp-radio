//! ESP32-S31 TX metadata derived from portable traffic and completion intent.
//!
//! Queue numbers, descriptor priority bytes, packet types and callback bits
//! belong to the chip representation. IEEE 802.11 framing retains typed WMM
//! categories without depending on this encoding.

use oer_ieee80211_mac::{
    data::{DataEncapPlan, DataInterfaceRole, IEEE80211_QOS_DATA_HEADER_LEN},
    qos::{WmmAccessCategory, WmmUserPriority},
};

const ETHER_TYPE_EAPOL: u16 = 0x888e;
const CALLBACK_STA_EAPOL: u32 = 1 << 3;
const CALLBACK_AP_POWER_SAVE: u32 = 1 << 12;

/// Metadata accompanying one encoded ordinary STA/AP data MPDU.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DataTxMetadata {
    pub queue_class: u8,
    pub packet_type: u8,
}

impl DataTxMetadata {
    /// Lower a portable encapsulation plan into the existing S31 packet profile.
    pub const fn from_encapsulation(plan: &DataEncapPlan) -> Self {
        let queue_class = queue_for_category(plan.access_category);
        let packet_class = if plan.header_len as usize == IEEE80211_QOS_DATA_HEADER_LEN {
            queue_class
        } else {
            0
        };
        Self {
            queue_class,
            packet_type: 10 + packet_class,
        }
    }
}

/// Lower a standard WMM category to S31's VO=0, VI=1, BE=2, BK=3 queue order.
pub const fn queue_for_category(category: WmmAccessCategory) -> u8 {
    match category {
        WmmAccessCategory::Voice => 0,
        WmmAccessCategory::Video => 1,
        WmmAccessCategory::BestEffort => 2,
        WmmAccessCategory::Background => 3,
    }
}

/// Validate an ordinary user priority and select its S31 queue.
pub const fn queue_class(priority: u8) -> Option<u8> {
    match WmmUserPriority::new(priority) {
        Some(priority) => Some(queue_for_category(priority.access_category())),
        None => None,
    }
}

pub const fn descriptor_priority_byte(priority: u8) -> Option<u8> {
    match queue_class(priority) {
        Some(class) => Some((class << 4) | priority),
        None => None,
    }
}

pub const fn completion_callback_mask(role: DataInterfaceRole, ether_type: u16) -> u32 {
    match role {
        DataInterfaceRole::Station if ether_type == ETHER_TYPE_EAPOL => CALLBACK_STA_EAPOL,
        DataInterfaceRole::Station => 0,
        DataInterfaceRole::AccessPoint => CALLBACK_AP_POWER_SAVE,
    }
}

#[cfg(test)]
mod tests;
