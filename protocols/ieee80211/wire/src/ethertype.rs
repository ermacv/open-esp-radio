//! EtherType values the IEEE 802.11 side itself names after an RFC 1042
//! LLC/SNAP header (IEEE 802.11-2020 5.1.4 and 9.3.2.1). Each value is the
//! big-endian number of its registry entry; the values of the network side
//! (IP, ARP, VLAN) belong to `oer-network-wire`.

/// EAP over LAN: IEEE 802.1X port access entity frames, which carry the
/// RSN 4-way and group key handshakes.
///
/// SOURCE: IEEE Std 802.1X-2020, 11.1.4 (PAE EtherType); IEEE Std
/// 802.11-2020, 12.7.2 (EAPOL-Key frames).
pub const ETHER_TYPE_EAPOL: u16 = 0x888e;
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
    }
}
