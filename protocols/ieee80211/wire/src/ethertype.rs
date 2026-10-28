//! EtherType values that IEEE 802.11 data frames carry after their LLC/SNAP
//! header (IEEE 802.11-2020 5.1.4 and 9.3.2.1: an RFC 1042 SNAP header
//! names the EtherType of the MSDU) and that Ethernet frames carry in their
//! Type field. Each value is the big-endian number of its registry entry.

/// Internet Protocol version 4.
///
/// SOURCE: IEEE Registration Authority EtherType registry; RFC 894.
pub const ETHER_TYPE_IPV4: u16 = 0x0800;
/// Address Resolution Protocol.
///
/// SOURCE: IEEE Registration Authority EtherType registry; RFC 826.
pub const ETHER_TYPE_ARP: u16 = 0x0806;
/// Customer VLAN tag (C-TAG).
///
/// SOURCE: IEEE Std 802.1Q-2022, 9.5 (Tag Protocol Identification), Table 9-1.
pub const ETHER_TYPE_VLAN: u16 = 0x8100;
/// Internet Protocol version 6.
///
/// SOURCE: IEEE Registration Authority EtherType registry; RFC 8200.
pub const ETHER_TYPE_IPV6: u16 = 0x86dd;
/// EAP over LAN: IEEE 802.1X port access entity frames, which carry the
/// RSN 4-way and group key handshakes.
///
/// SOURCE: IEEE Std 802.1X-2020, 11.1.4 (PAE EtherType); IEEE Std
/// 802.11-2020, 12.7.2 (EAPOL-Key frames).
pub const ETHER_TYPE_EAPOL: u16 = 0x888e;
/// Service VLAN tag (S-TAG).
///
/// SOURCE: IEEE Std 802.1Q-2022, 9.5 (Tag Protocol Identification), Table 9-1.
pub const ETHER_TYPE_PROVIDER_VLAN: u16 = 0x88a8;
/// WLAN Authentication and Privacy Infrastructure.
///
/// SOURCE: IEEE Registration Authority EtherType registry (88-B4).
pub const ETHER_TYPE_WAPI: u16 = 0x88b4;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn eapol_is_the_registered_pae_ethertype_on_air() {
        assert_eq!(ETHER_TYPE_EAPOL.to_be_bytes(), [0x88, 0x8e]);
        assert_eq!(ETHER_TYPE_IPV4.to_be_bytes(), [0x08, 0x00]);
    }
}
