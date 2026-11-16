//! What an HE20 station advertises. The power elements of its Association
//! Request are each scanned channel's, computed by the backend
//! (`facts::he_power`).

use oer_protocols_ieee80211_contracts_port::Channel;
use oer_protocols_ieee80211_wire::{
    he::{
        HeLocalCapabilities, HeMacCapabilities, HeMcsNssSupport, HePhyCapabilities,
        he_capability_ie,
    },
    ht::{HtAmpduParameters, HtCapabilityInfo, HtLocalCapabilities, ht_capability_ie},
};

// HT Capabilities beside HE20: HT20, one spatial stream, MCS 0..7, SM power
// save disabled, A-MPDUs of up to 65 535 octets with MPDUs at least 4 us
// apart: the HT40 element's fields on the 20 MHz geometry.
//
// SOURCE: the vendor station's HT Capabilities beside its HE elements
// (`libnet80211.a[ieee80211_ht.o]::ieee80211_add_htcap_body`, frame 7624 of
// HIL_VENDOR_HE20_NDPA_CBF_2026_07_24: capability information 0x112c, A-MPDU
// Parameters 0x17), without its RX STBC and 40 MHz DSSS/CCK claims; the
// encoder sets the 20 MHz short guard interval from the geometry, as in the
// HT20 element.
pub(crate) const HE20_HT: [u8; 28] = ht_capability_ie(
    HtLocalCapabilities::new(
        u16::from_le_bytes(
            HtCapabilityInfo::from_bytes([0; 2])
                .with_sm_power_save(HtCapabilityInfo::SM_POWER_SAVE_DISABLED)
                .to_bytes(),
        ),
        {
            let [parameters] = HtAmpduParameters::from_bytes([0])
                .with_exponent(HtAmpduParameters::EXPONENT_65535_OCTETS)
                .with_spacing(HtAmpduParameters::SPACING_4_US)
                .to_bytes();
            parameters
        },
        0xff,
        1,
    ),
    match Channel::ghz2_4(
        1,
        oer_protocols_ieee80211_wire::channel::ChannelWidth::Mhz20,
    ) {
        Ok(channel) => channel,
        Err(_) => panic!("HE20 HT advertisement geometry"),
    },
);
// HE20: one spatial stream, HE-MCS 0..9 received and transmitted, and no
// optional capability: every HE MAC capability clear but the Trigger Frame
// MAC Padding Duration, 16 us; every HE PHY capability clear but the Nominal
// Packet Padding, 16 us for every constellation. Neither value claims a
// capability: they ask the access point for processing time. LDPC, 1x LTF
// with 0.8 us GI, STBC, DCM, beamformee, device class A, UL MU-MIMO, PPE
// Thresholds and every trigger, TWT, BSR, OM control and +HTC-HE capability
// stay clear: nothing here implements them, and no Trigger frame gets an
// HE TB PPDU answer this station owns.
//
// SOURCE: the vendor's TF MAC padding default `s_mac_cap_padding_duration = 2`
// (`libnet80211.a[ieee80211_he.o]::ieee80211_add_hecap` +0x130/+0x134,
// `esp_wifi_set_tf_padding_duration` admits 0..2) and Nominal Packet Padding
// 16 us (the vendor default element, PHY byte 9 bits 7:6 = 2). Field
// positions: IEEE Std 802.11ax-2021, 9.4.2.248.2 and .3, as Linux cites them.
pub(crate) const HE20: [u8; 24] = he_capability_ie(
    HeLocalCapabilities {
        mac: HeMacCapabilities::from_bytes([0; 6])
            .with_trigger_frame_mac_padding(HeMacCapabilities::TRIGGER_FRAME_MAC_PADDING_16_US),
        phy: HePhyCapabilities::from_bytes([0; 11])
            .with_nominal_padding(HePhyCapabilities::NOMINAL_PADDING_16_US),
        receive: HeMcsNssSupport::Mcs0To9,
        transmit: HeMcsNssSupport::Mcs0To9,
    },
    oer_chips_esp32s31_radio_ieee80211_backend::facts::ESP32S31_HE_CHANNEL_WIDTH_SET,
);
// No Extended Capabilities element beside HE20: this station claims none
// of its capabilities. The vendor's HE station path sets Event, Multiple
// BSSID and TWT Requester (`libnet80211.a[ieee80211_output.o]::
// ieee80211_add_extcap` +0x6c..0x13c), none of which this station
// implements, and an element with every bit clear is not sent: as
// wpa_supplicant trims an Extended Capabilities element's zero tail and
// skips an empty one (implementation precedent, not a requirement of the
// standard).
pub(crate) const HE20_EXTENDED: Option<[u8; 14]> = None;
#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::disallowed_methods)]
mod tests;
