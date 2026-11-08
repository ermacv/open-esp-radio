//! The interface-zero HE20 association: its BSS before authentication, its
//! installation after a successful Association Response and its removal.

use oer_portable_mmio::{Bits, Index};

use crate::radio::ieee80211::registers::{
    MacAssociationId, MacHeBssColor, MacHeDefaultPacketExtensionDuration,
    MacHePacketPaddingDuration, MacMinimumMpduStartSpacing, MacPlacement, WifiRadioRegisters, bits,
};

/// Hardware-visible subset of one parsed HE20 peer.
///
/// SOURCE: complete pinned `libnet80211.a[ieee80211_he.o]`
/// `ieee80211_parse_hecap`/`ieee80211_parse_heopr` callers and complete
/// `libpp.a[hal_mac_ctl.o]` leaves. The protocol crate owns parsing;
/// this type keeps the chip-specific register transform inside the HAL.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MacHe20PeerConfig {
    pub packet_padding_duration: MacHePacketPaddingDuration,
    pub default_packet_extension_duration: MacHeDefaultPacketExtensionDuration,
    /// The HE Operation's Max Co-Hosted BSSID Indicator of a co-hosted BSS;
    /// `None` when it announces none.
    pub max_co_hosted_bssid_indicator: Option<u8>,
    pub extended_range_single_user_disabled: bool,
}

/// One interface-zero HE20 association: the peer's parsed state, the
/// station's association identifier and the access point's minimum MPDU
/// start spacing.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MacHe20Association {
    pub peer: MacHe20PeerConfig,
    pub association_id: MacAssociationId,
    pub minimum_mpdu_start_spacing: MacMinimumMpduStartSpacing,
}

impl<M: MacPlacement> WifiRadioRegisters<M> {
    /// Enable interface-zero HE BSSID matching.
    ///
    /// SOURCE: complete pinned `libpp.a[hal_mac_ctl.o]::hal_he_bssid_init`
    /// (interface zero: two fresh-read RMWs of `0x2010_4020`), tail-calling
    /// the complete `hal_he_set_power_save(0)` (two RMWs of `0x2010_40a0`).
    fn initialize_interface_zero_he_bssid(&mut self) {
        let control = &mut self.mac.wifi_mac_he_init_suffix;
        control.modify_multi_bssid_control(|w| {
            w.set_he_bssid_enable(true);
        });
        control.modify_multi_bssid_control(|w| {
            w.set_bssid_select(Bits::truncate(
                oer_chips_esp32s31_pac::wifi_mac::multi_bssid_control::BssidSelect::Interface0
                    .bits(),
            ));
        });

        let power_save = &mut self.mac.wifi_mac_rx_power_save;
        power_save.modify_rx_power_save_control(|w| {
            w.set_intra_ppdu_ps_enable(true);
        });
        power_save.modify_rx_power_save_control(|w| {
            w.set_intra_ps_check_bss_color_enable(true);
        });
    }

    /// Clear the received-color bitmap.
    ///
    /// SOURCE: complete pinned `libpp.a[hal_mac_ctl.o]::hal_mac_color_clr_bitmap`
    /// (size `0xe`): one RMW setting bit 0 of `0x2010_4048`.
    fn clear_he_color_bitmap(&mut self) {
        self.mac
            .wifi_mac_he_init_prefix
            .modify_rx_field_control(|w| {
                w.set_color_bitmap_clear(true);
            });
    }

    /// Publish interface zero's BSS color: enabled with `color` and
    /// `partial`, or disabled with a zero color.
    ///
    /// SOURCE: complete pinned `libpp.a[hal_mac_ctl.o]::hal_he_set_bss_color`
    /// (size `0xc2`), interface zero: three fresh-read RMWs of `0x2010_4020`,
    /// the enable bit 27, then the color bits 26:21 (zero when disabled),
    /// then the partial bit 28.
    fn set_interface_zero_he_bss_color(&mut self, enabled: bool, color: u32, partial: bool) {
        let control = &mut self.mac.wifi_mac_he_init_suffix;
        control.modify_multi_bssid_control(|w| {
            w.set_bss_color_enable(enabled);
        });
        let color = if enabled { color } else { 0 };
        control.modify_multi_bssid_control(|w| {
            w.set_bss_color(bits(color));
        });
        control.modify_multi_bssid_control(|w| {
            w.set_partial_bss_color_enable(partial);
        });
    }

    /// The co-hosted BSS of an HE Operation that announces one with its Max
    /// Co-Hosted BSSID Indicator, or none.
    ///
    /// SOURCE: complete pinned `libpp.a[hal_mac_ctl.o]::hal_he_set_co_hosted_bss`
    /// (size `0x7a`): nothing while MULTI_BSSID_ENABLE (bit 8 of
    /// `0x2010_4020`) is set; else, co-hosted, set CO_HOSTED_ENABLE (bit 18)
    /// and replace MULTI_BSSID_MASK (bits 16:9) by `(-1 << indicator) & 0xff`
    /// (`sll` takes the shift modulo 32); not co-hosted, clear bit 18 and
    /// set every mask bit. Two fresh-read RMWs either way, as the cold
    /// init's `hal_he_set_co_hosted_bss(0, 0)` keeps its guard read.
    fn set_co_hosted_bss(&mut self, max_co_hosted_bssid_indicator: Option<u8>) {
        let control = &mut self.mac.wifi_mac_he_init_suffix;
        if control.read_multi_bssid_control().multi_bssid_enable() {
            return;
        }
        match max_co_hosted_bssid_indicator {
            Some(indicator) => {
                let mask = u32::MAX.wrapping_shl(u32::from(indicator)).to_le_bytes()[0];
                control.modify_multi_bssid_control(|w| {
                    w.set_co_hosted_enable(true);
                });
                control.modify_multi_bssid_control(|w| {
                    w.set_multi_bssid_mask(mask);
                });
            }
            None => {
                control.modify_multi_bssid_control(|w| {
                    w.set_co_hosted_enable(false);
                });
                control.modify_multi_bssid_control(|w| {
                    w.set_multi_bssid_mask(0xff);
                });
            }
        }
    }

    /// Every constellation's packet padding.
    ///
    /// SOURCE: complete pinned `libpp.a[hal_mac_ctl.o]::hal_he_set_packet_padding_us`
    /// (size `0x70`) replaces the five five-bit durations of `0x2010_4c90`
    /// in five fresh-read RMWs, BPSK first.
    fn set_he_packet_padding(&mut self, duration: MacHePacketPaddingDuration) {
        let duration = bits(duration.get());
        let init = &mut self.mac.wifi_mac_he_init_suffix;
        init.modify_he_packet_padding(|w| {
            w.set_bpsk_duration(duration);
        });
        init.modify_he_packet_padding(|w| {
            w.set_qpsk_duration(duration);
        });
        init.modify_he_packet_padding(|w| {
            w.set_qam16_duration(duration);
        });
        init.modify_he_packet_padding(|w| {
            w.set_qam64_duration(duration);
        });
        init.modify_he_packet_padding(|w| {
            w.set_qam256_duration(duration);
        });
    }

    /// Begin an HE BSS's connection before authentication: HE BSSID
    /// matching for interface zero, the color bitmap cleared and the BSS
    /// color the scan heard.
    ///
    /// SOURCE: complete pinned `libnet80211.a[wl_cnx.o]::cnx_connect_to_bss`
    /// HE path +0x12a..0x150: `hal_he_bssid_init(0)`,
    /// `hal_mac_color_clr_bitmap()`, then `hal_he_set_bss_color(0, enabled,
    /// color, partial)` from the scanned HE Operation's BSS Color
    /// Information.
    pub fn connect_he20_bss(&mut self, color: MacHeBssColor, enabled: bool, partial: bool) {
        self.initialize_interface_zero_he_bssid();
        self.clear_he_color_bitmap();
        self.set_interface_zero_he_bss_color(enabled, color.get(), partial);
    }

    /// Install the interface-zero HE20 association after its Association
    /// Response, in the vendor's order: the capabilities' packet padding
    /// (`ieee80211_parse_hecap`), the operation's default PE, co-hosted BSS,
    /// HE RTS threshold and ER-SU (`ieee80211_parse_heopr` +0xc4..0x27a; its
    /// BSS color is written only to a connected station), then the node's
    /// setup (`libnet80211.a[wl_cnx.o]::ic_set_sta` +0x13c..0x170: MMSS and
    /// AID, packet padding, BSR initialization). The BSS's color and HE
    /// BSSID matching are [`Self::connect_he20_bss`]'s, before
    /// authentication.
    ///
    /// The HE RTS threshold is always disabled: the transmitter protects
    /// every individually addressed HE PPDU while the access point
    /// advertises a threshold, and the vendor's threshold table
    /// (`ic_set_he_rts_threshold_bytes_tab`) is not ported. The broadcast
    /// RU index is zero, an access point outside a Multiple BSSID set
    /// (`ic_set_sta` passes node byte 899; the station never enables
    /// Multiple BSSID, so `hal_he_set_mmss_and_aid` passes 0 for the low
    /// value).
    pub fn install_he20_association(&mut self, association: MacHe20Association) {
        let MacHe20Association {
            peer,
            association_id,
            minimum_mpdu_start_spacing,
        } = association;
        self.set_he_packet_padding(peer.packet_padding_duration);
        // SOURCE: complete pinned `hal_he_set_default_pe` (size `0x14`):
        // one RMW of bits 2:0 of `0x2010_4c80`.
        self.mac
            .wifi_mac_he_init_suffix
            .modify_he_default_control(|w| {
                w.set_default_pe_duration(bits(peer.default_packet_extension_duration.get()));
            });
        self.set_co_hosted_bss(peer.max_co_hosted_bssid_indicator);
        // SOURCE: complete pinned `hal_he_disable_rts_threshold`
        // (size `0x32`): bit 17 of each ordinary queue's HE RTS word, queue
        // 0 first; `ieee80211_parse_heopr` +0x19e..0x1a4 calls it for a
        // threshold of 0 or 1023, and `ic_set_sta` +0x2ca for a node without
        // HE.
        crate::radio::ieee80211::registers::tx::queue::disable_he_rts_threshold(
            &mut self.mac.wifi_mac_tx_queue_vector,
        );
        // SOURCE: complete pinned `hal_he_set_ersu` (size `0x22`): clear
        // bit 10 of `0x2010_4c7c` when ER-SU is disabled, else set it and
        // tail-call `hal_he_set_ersu_ack_rate`, whose one store writes all
        // four baseline ACK-rate bytes to 0x80 (every bit of the word).
        let init = &mut self.mac.wifi_mac_he_init_suffix;
        init.modify_ersu_and_vht_control(|w| {
            w.set_auto_ack_allow_ersu(!peer.extended_range_single_user_disabled);
        });
        if !peer.extended_range_single_user_disabled {
            init.modify_ersu_ack_rate(|w| {
                w.set_rate_0(
                    oer_chips_esp32s31_pac::wifi_mac::ersu_ack_rate::Rate0::Ordinary.bits() as u8,
                )
                .set_rate_1(
                    oer_chips_esp32s31_pac::wifi_mac::ersu_ack_rate::Rate1::Ordinary.bits() as u8,
                )
                .set_rate_2(
                    oer_chips_esp32s31_pac::wifi_mac::ersu_ack_rate::Rate2::Ordinary.bits() as u8,
                )
                .set_rate_3(
                    oer_chips_esp32s31_pac::wifi_mac::ersu_ack_rate::Rate3::Ordinary.bits() as u8,
                );
            });
        }

        // SOURCE: complete pinned `hal_he_set_mmss_and_aid` (size `0x82`),
        // interface zero: the MMSS RMW, the AID RMW of `0x2010_4004`, the AID
        // RMW of `0x2010_4038` bits 10:0, then the tail call
        // `hal_he_set_bcast_ru(index, low, 0)` with `low` zero while
        // MULTI_BSSID_ENABLE is clear. Complete `hal_he_set_bcast_ru`
        // (size `0x62`): `0x2010_4038` enable bit 22, then bits 21:11;
        // `0x2010_403c` bit 11, bits 10:0, bit 23, bits 22:12.
        let association: Bits<11> = bits(association_id.get());
        let policy = &mut self.mac.wifi_mac_bssid_policy;
        policy.modify_bssid_high(Index::constant::<0>(), |w| {
            w.set_minimum_mpdu_start_spacing(bits(minimum_mpdu_start_spacing.get()));
        });
        policy.modify_bssid_high(Index::constant::<0>(), |w| {
            w.set_association_id(association);
        });
        let init = &mut self.mac.wifi_mac_he_init_suffix;
        init.modify_broadcast_ru_low(|w| {
            w.set_association_id(association);
        });
        init.modify_broadcast_ru_low(|w| {
            w.set_enable(true);
        });
        init.modify_broadcast_ru_low(|w| {
            w.set_value(Bits::constant::<0>());
        });
        init.modify_broadcast_ru_high(|w| {
            w.set_low_enable(true);
        });
        init.modify_broadcast_ru_high(|w| {
            w.set_low_value(Bits::constant::<0>());
        });
        init.modify_broadcast_ru_high(|w| {
            w.set_high_enable(true);
        });
        init.modify_broadcast_ru_high(|w| {
            w.set_high_value(Bits::constant::<0>());
        });
        self.set_he_packet_padding(peer.packet_padding_duration);
        self.initialize_he_buffer_status_report();
    }

    /// Remove the interface-zero HE association: HE BSSID matching off, the
    /// color bitmap cleared and the BSS color disabled and zero.
    ///
    /// SOURCE: complete pinned `libnet80211.a[wl_cnx.o]::cnx_connect_to_bss`
    /// non-HE path +0x1b4..0x1c4: `hal_he_bssid_deinit(0)` (size `0x1e`, one
    /// RMW clearing bit 31 of `0x2010_4020`), `hal_mac_color_clr_bitmap`,
    /// then `hal_he_set_bss_color(0, 0, 0, 0)`. The vendor leaves the AID,
    /// the broadcast RU, BSR and padding state of an earlier association in
    /// place; this does too.
    pub fn clear_he20_association(&mut self) {
        self.mac
            .wifi_mac_he_init_suffix
            .modify_multi_bssid_control(|w| {
                w.set_he_bssid_enable(false);
            });
        self.clear_he_color_bitmap();
        self.set_interface_zero_he_bss_color(false, 0, false);
    }
}

#[cfg(test)]
mod tests;
