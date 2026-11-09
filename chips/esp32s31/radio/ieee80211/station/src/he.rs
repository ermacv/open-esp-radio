//! What an HE20 station advertises and the power elements of its HE20
//! Association Request.

use oer_protocols_ieee80211_contracts_port::Channel;
use oer_protocols_ieee80211_core_roles_sta::association::StaHePower;
use oer_protocols_ieee80211_wire::{
    he::{
        He20LocalCapabilities, HeMacCapabilities, HeMcsNssSupport, HePhyCapabilities,
        he20_capability_ie,
    },
    ht::{HtAmpduParameters, HtCapabilityInfo, HtLocalCapabilities, ht_capability_ie},
    station::{
        HeUlMuPowerCapability, HeUlMuPowerCapabilityError, StaPowerCapability,
        StaPowerCapabilityError,
    },
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
pub(crate) const HE20: [u8; 24] = he20_capability_ie(He20LocalCapabilities {
    mac: HeMacCapabilities::from_bytes([0; 6])
        .with_trigger_frame_mac_padding(HeMacCapabilities::TRIGGER_FRAME_MAC_PADDING_16_US),
    phy: HePhyCapabilities::from_bytes([0; 11])
        .with_nominal_padding(HePhyCapabilities::NOMINAL_PADDING_16_US),
    receive: HeMcsNssSupport::Mcs0To9,
    transmit: HeMcsNssSupport::Mcs0To9,
});
// No Extended Capabilities element beside HE20: this station claims none
// of its capabilities. The vendor's HE station path sets Event, Multiple
// BSSID and TWT Requester (`libnet80211.a[ieee80211_output.o]::
// ieee80211_add_extcap` +0x6c..0x13c), none of which this station
// implements, and an element with every bit clear is not sent: as
// wpa_supplicant trims an Extended Capabilities element's zero tail and
// skips an empty one (implementation precedent, not a requirement of the
// standard).
pub(crate) const HE20_EXTENDED: Option<[u8; 14]> = None;
/// Why the PHY's powers give no HE power elements.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum HePowerError {
    /// The MCS 0 power lies below the MAC's minimum.
    Capability(StaPowerCapabilityError),
    /// A rate of HE-MCS 1 to 9 has more power than HE-MCS 0.
    UlMu(HeUlMuPowerCapabilityError),
}

/// The power elements of an HE20 Association Request: the Power Capability
/// (`minimum_dbm` and the HE-MCS 0 power) and the HE UL-MU power capability
/// (HE-MCS 1 to 9 against 0), from `rate_power`, the power code each MAC
/// rate code of 16 to 25 (HE-MCS 0 to 9) transmits at.
///
/// SOURCE(esp32s31): complete pinned `libnet80211.a[ieee80211_he.o]::
/// ieee80211_add_power_cap` (`hal_get_tx_min_pwr`, `hal_get_tx_pwr(16, 1)`)
/// and `ieee80211_add_ulmu_pwrcap` (`phy_get_max_pwr` of rates 16..=25).
pub(crate) fn he_power(
    minimum_dbm: i8,
    rate_power: impl Fn(u8) -> i8,
) -> Result<StaHePower, HePowerError> {
    let rates: [i8; 10] =
        core::array::from_fn(|index| rate_power(16_u8.saturating_add(index as u8)));
    let [mcs0, ..] = rates;
    Ok(StaHePower {
        power_capability: StaPowerCapability::new(minimum_dbm, mcs0)
            .map_err(HePowerError::Capability)?,
        ul_mu_power: HeUlMuPowerCapability::from_rate_power_indices(rates)
            .map_err(HePowerError::UlMu)?,
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::disallowed_methods)]
mod tests;
