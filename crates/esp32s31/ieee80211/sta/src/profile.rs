//! ESP32-S31 station capability and channel-selection profile.
//!
//! These claims retain the existing source-owned runtime limits and reviewed
//! evidence. The portable IEEE 802.11 encoder consumes this explicit profile;
//! it does not choose hardware capabilities or S31 channel encodings.

use {
    oer_ieee80211_mac::scan::HtSecondaryChannel, oer_ieee80211_mac::scan::ScanRecord,
    oer_ieee80211_mac::station::AssociationCapabilities,
    oer_ieee80211_mac::station::association::PhyMode,
    oer_ieee80211_mac::station::association::Preference,
};

// One-stream HT20 with short guard interval. Channel-width, STBC, LDPC,
// large A-MSDU and 40-MHz claims remain disabled until their matching
// Rust-owned paths are enabled.
//
// SOURCE[PROMOTED_HE20_PEER]: reviewed promoted HT20 capability image,
// originally qualified by the strict ESP32-S31 STA WPA2/ADDBA throughput HIL.
const HT20_CAPABILITY_IE: [u8; 28] = station_ht_capability_ie(0x0020, 0x00);
// One-stream HT40 with short guard intervals for both 20 and 40 MHz and
// spatial multiplexing power save disabled. Although the S31 has one receive
// stream, advertising static SMPS (`0x0062`) made the controlled Linux HT40 AP
// accept authentication but discard the association request. The vendor
// builder preserves the interface's SMPS bits and ordinary ESP STA requests
// use the disabled value (`bits 2..3 = 0b11`), giving `0x006e`.
//
// SOURCE: `libnet80211.a[ieee80211_ht.o]::
// ieee80211_add_htcap_body` reads the base capability at node offset `0x14c`,
// adds Supported Channel Width at `+0x4e`, SGI20 at `+0x8e`, and SGI40 at
// `+0xaa` without replacing the base SMPS bits. IEEE 802.11 HT Capabilities
// Info defines bits 2..3 value `0b11` as SMPS disabled. Selection remains
// gated by the complete AP HT Capabilities/Operation IEs through
// `ScanRecord::ht40_secondary_channel`; hardware CBW support is the complete
// rev0 ROM `phy_bb_bss_cbw40` implementation promoted into the S31 PAC/HAL.
// The independent MCS32 receive bit remains clear until RX behavior has
// runtime and HIL proof. The parser and diagnostics can still observe MCS32
// from a peer without advertising it as a local receive capability.
const HT40_CAPABILITY_IE: [u8; 28] = station_ht_capability_ie(0x006e, 0x00);
// Exact HT20 capability carried beside the HE capability in the complete
// vendor association request. It differs from the deliberately narrow
// standalone HT20 profile above: SMPS is disabled, RX STBC is one stream,
// the receive A-MPDU limit is 65,535 bytes and minimum spacing is 4 us.
//
// SOURCE[HIL_VENDOR_HE20_NDPA_CBF_2026_07_24]: qualified frame 7624.
// SOURCE: complete `libnet80211.a[ieee80211_ht.o]::
// ieee80211_add_htcap_body` produces the same capability fields.
const HE20_HT_CAPABILITY_IE: [u8; 28] = station_ht_capability_ie(0x112c, 0x17);

/// Build the complete vendor-shaped one-stream HT capability base.
///
/// The Supported MCS Set begins at complete-IE byte five. Its byte 12 is the
/// TX MCS parameters field, therefore it is complete-IE byte 17. Keeping this
/// layout in one builder prevents the STA capability images from drifting.
/// The supported MCS set deliberately contains only the runtime-qualified
/// one-stream MCS0 through MCS7 range.
const fn station_ht_capability_ie(capability_info: u16, ampdu_parameters: u8) -> [u8; 28] {
    let mut element = [0_u8; 28];
    element[0] = 45;
    element[1] = 26;
    element[2] = capability_info as u8;
    element[3] = (capability_info >> 8) as u8;
    element[4] = ampdu_parameters;
    element[5] = 0xff;
    element[17] = 0x01;
    element
}
// Exact one-stream HE20 MCS0-9 capability captured from the vendor
// association request and retained as a comparison oracle.
// This must not be relabelled HE40: complete
// `libnet80211.a[ieee80211_he.o]::ieee80211_add_hecap` writes zero to
// complete IE byte nine (the first HE PHY Capabilities byte / Channel Width
// Set) on both its STA branches. The chip's vendor path advertises 40 MHz
// separately through HT Capabilities, as represented by HT40_CAPABILITY_IE.
//
// SOURCE[PROMOTED_HE20_ASSOCIATION]: reviewed promoted HE20 MCS9 capability
// image of the request constructed by pinned `libnet80211.a`.
//
// `libnet80211.a[ieee80211_he.o]::ieee80211_add_hecap` proves that
// byte 11 bit 3 is the S31 `g_phy_cap_rx_stbc` advertisement. Byte 15 bits
// 2..4 advertise triggered SU beamforming feedback, triggered MU partial-
// bandwidth feedback and triggered CQI; byte 18 bit 1 advertises
// non-triggered CQI.
const HE20_VENDOR_MCS9_CAPABILITY_IE: [u8; 24] = [
    255, 22, 35, 0x03, 0x18, 0x9c, 0xca, 0x10, 0x80, 0x00, 0x10, 0x8a, 0x1b, 0x0d, 0xc0, 0x1f,
    0x00, 0x02, 0x82, 0x01, 0xfd, 0xff, 0xfd, 0xff,
];

// CAPABILITY: wifi-802-11ax-he-su-mu-beamformee
const fn owned_he20_mcs9_capability_ie() -> [u8; 24] {
    let mut capability = HE20_VENDOR_MCS9_CAPABILITY_IE;
    // HE MAC Capabilities bit 1 is TWT Requester Support. The open driver has
    // no activated hardware TWT wake transaction owner, so it must not
    // inherit that vendor claim.
    capability[3] &= !(1 << 1);
    // Hardware setup and report-rate programming exist, but the reachable
    // software publication boundary rejects every NDPA feedback request and
    // autonomous completion by the hardware has no runtime proof. Clear the
    // SU beamformee bit and its under-80-MHz Max STS field, the two NG16
    // feedback claims, and the SU/MU codebook plus triggered-feedback claims.
    // Keeping dependent beamformee fields after clearing SU beamformee would
    // itself publish a contradictory capability image.
    capability[13] &= !(0x01 | 0x1c);
    capability[14] &= !(0x40 | 0x80);
    capability[15] &= !(0x01 | 0x02 | 0x04 | 0x08);
    // There is also no open triggered or non-triggered CQI report producer:
    // advertising either capability could make an AP schedule a response the
    // STA cannot generate. Clear those two independent claims.
    //
    // SOURCE[HIL_OPEN_HE20_CQI_CAPABILITY_MASK_2026_07_30]: ESP32-S31 rev0
    // associated with FRITZ!Box 7530 FN on channel 1 after these exact two
    // bits were cleared, completed WPA2/CCMP and DHCP, then passed a complete
    // 30-profile LDPC MCS0..9 x GI/LTF A-MPDU matrix with zero failed
    // profiles. Evidence:
    // `/tmp/open-radio-he20-owned-cqi-mask-hil-20260730.log`.
    capability[15] &= !(1 << 4);
    capability[18] &= !(1 << 1);
    capability
}

const HE20_OWNED_MCS9_CAPABILITY_IE: [u8; 24] = owned_he20_mcs9_capability_ie();

// Vendor-shaped Extended Capabilities IE adjacent to the HE/UL-MU capability
// pair. Retain Event, but clear Multiple BSSID because the scan/profile and
// hardware BSSID-index owners implement only BSSID zero. Extended
// Capabilities bit 77 (TWT requester) also remains clear until negotiation and
// wake ownership exist.
//
// SOURCE[HIL_VENDOR_HE20_NDPA_CBF_2026_07_24]: exact frame 7624 bytes.
// SOURCE: complete `libnet80211.a[ieee80211_output.o]::
// ieee80211_add_extcap` emits this 12-byte body for the captured STA state.
const HE20_EXTENDED_CAPABILITY_IE: [u8; 14] =
    [127, 12, 0x80, 0x00, 0x00, 0, 0, 0, 0, 0, 0, 0, 0, 0];
// WMM Information, version one, U-APSD disabled.
//
// SOURCE: the same promoted `sta_link.rs::WMM_INFORMATION_IE`, cross-checked
// against `libnet80211.a` association-request construction.
const WMM_INFORMATION_IE: [u8; 9] = [221, 7, 0x00, 0x50, 0xf2, 0x02, 0x00, 0x01, 0x00];

/// The exact local station profile used by every production association TX.
pub const ASSOCIATION_CAPABILITIES: AssociationCapabilities = AssociationCapabilities {
    ht20: HT20_CAPABILITY_IE,
    ht40: HT40_CAPABILITY_IE,
    he20_ht: HE20_HT_CAPABILITY_IE,
    he20: HE20_OWNED_MCS9_CAPABILITY_IE,
    he20_extended: HE20_EXTENDED_CAPABILITY_IE,
    wmm: WMM_INFORMATION_IE,
};

/// Complete scan-to-channel decision consumed by the PHY channel transition.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Selection {
    pub phy: PhyMode,
    pub primary_channel: u8,
    /// Primary channel number for 20 MHz, center frequency in MHz for HT40.
    pub channel_or_frequency: u16,
    /// Recovered ESP32-S31 CBW encoding: 0=20 MHz, 2=above, 3=below.
    pub cbw: u8,
}

/// Select the strongest open-driver PHY supported by one scanned peer.
///
/// Automatic mode prefers the 150-Mbit/s one-stream HT40 path when the peer's
/// HT Capabilities and HT Operation agree on a usable secondary channel. HE
/// remains 20-MHz-only because the complete ESP32-S31 vendor HE capability
/// builder advertises a zero HE Channel Width Set. Otherwise the selection is
/// HE20 MCS9 or the conservative HT20 fallback, and legacy for an access
/// point without HT Capabilities (`select_association_phy` of
/// `oer-ieee80211-sta`).
///
/// SOURCE: complete `libnet80211.a[ieee80211_he.o]::
/// ieee80211_add_hecap` and the complete HT Capabilities/Operation IEs retained
/// by [`ScanRecord::ht40_secondary_channel`]. The above/below CBW values are
/// independently recovered from rev0 ROM `phy_bb_bss_cbw40`.
pub fn select_association(access_point: &ScanRecord, preference: Preference) -> Selection {
    let phy =
        oer_ieee80211_sta::association::select_association_phy(access_point, preference, true);

    let primary_frequency = 2_407 + u16::from(access_point.channel) * 5;
    let (channel_or_frequency, cbw) = if phy == PhyMode::Ht40 {
        match access_point.ht40_secondary_channel() {
            Some(HtSecondaryChannel::Above) => (primary_frequency + 10, 2),
            Some(HtSecondaryChannel::Below) => (primary_frequency - 10, 3),
            None => (u16::from(access_point.channel), 0),
        }
    } else {
        (u16::from(access_point.channel), 0)
    };
    Selection {
        phy,
        primary_channel: access_point.channel,
        channel_or_frequency,
        cbw,
    }
}

#[cfg(test)]
mod tests;
