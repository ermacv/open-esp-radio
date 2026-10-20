//! Stateless policy recovered from the ESP32-S31 net80211 TX classifier.
//!
//! Raw ESF, descriptor, and node layouts remain in the target adapter. This
//! module contains only finite decisions which can be verified on the host.

pub const ETHER_TYPE_IPV4: u16 = 0x0008;
pub const ETHER_TYPE_ARP: u16 = 0x0608;
pub const ETHER_TYPE_EAPOL: u16 = 0x8e88;
pub const ETHER_TYPE_WAPI: u16 = 0xb488;
pub const ETHER_TYPE_IPV6: u16 = 0xdd86;
pub const ETHER_TYPE_VENDOR_PRIORITY: u16 = 0xeeee;

pub const fn uses_fixed_per_packet_rate(
    protocol: u16,
    sta_arp: bool,
    udp_ports: Option<(u16, u16)>,
) -> bool {
    if protocol == ETHER_TYPE_EAPOL || protocol == ETHER_TYPE_WAPI {
        return true;
    }
    if protocol == ETHER_TYPE_ARP {
        return sta_arp;
    }
    if protocol != ETHER_TYPE_IPV4 {
        return false;
    }
    match udp_ports {
        Some((source, destination)) => {
            source == 53 || source == 67 || source == 68 || destination == 53
        }
        None => false,
    }
}

pub const fn user_priority(protocol: u16, ipv4_tos: Option<u8>, ipv6_prefix: Option<u32>) -> u32 {
    match protocol {
        ETHER_TYPE_IPV4 => match ipv4_tos {
            Some(tos) => crate::qos::DsField::from_bytes([tos]).class_selector() as u32,
            None => 0,
        },
        ETHER_TYPE_IPV6 => match ipv6_prefix {
            Some(prefix) => {
                let [first, second, ..] = prefix.to_be_bytes();
                let header = crate::qos::Ipv6HeaderPrefix::from_bytes([first, second]);
                crate::qos::DsField::from_bytes([header.traffic_class()]).class_selector() as u32
            }
            None => 0,
        },
        ETHER_TYPE_VENDOR_PRIORITY => 5,
        _ => 0,
    }
}

pub const fn apply_wmm_admission(mut priority: u32, admission_required: [bool; 3]) -> u32 {
    let mut access_category = match priority {
        0 | 3 => 2,
        1 | 2 => return priority,
        4 | 5 => 1,
        6 | 7 => 0,
        _ => return 7,
    };
    // Exact transition pairs reconstructed from pinned
    // libnet80211.a[ieee80211_output.o]. A transition always increases the
    // access-category index and category 3 is terminal.
    let [voice, video, best_effort] = admission_required;
    if access_category == 0 {
        if !voice {
            return priority;
        }
        priority = 5;
        access_category = 1;
    }
    if access_category == 1 {
        if !video {
            return priority;
        }
        priority = 0;
    }
    if !best_effort {
        return priority;
    }
    1
}

#[cfg(test)]
mod tests;
