//! The finite HE20 peer/association leaves.

use oer_mmio::Bits;

use crate::ieee80211::registers::{
    MacAssociationId, MacHeBssColor, MacHeDefaultPacketExtensionDuration,
    MacHePacketPaddingDuration, MacMinimumMpduStartSpacing, MacPlacement, ORDINARY_QUEUE_COUNT,
    WifiRadioRegisters, bits, element, ordinary_bank,
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
    pub bss_color: MacHeBssColor,
    pub bss_color_enabled: bool,
    pub partial_bss_color: bool,
    pub extended_range_single_user_disabled: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MacHe20PeerError {
    UnsupportedRtsThreshold,
}

impl<M: MacPlacement> WifiRadioRegisters<M> {
    /// Enable interface-zero HE BSSID matching before installing peer state.
    ///
    /// SOURCE: complete `libpp.a[hal_mac_ctl.o]::
    /// hal_he_bssid_init`, size `0x4c`, tail-calling the complete
    /// `hal_he_set_power_save`, size `0x36`. The associated-STA path in
    /// `libnet80211.a[wl_cnx.o]::cnx_connect_to_bss`, size `0x2b6`,
    /// calls this exact interface-zero leaf before clearing the color bitmap
    /// and programming the parsed HE Operation BSS color.
    fn initialize_interface_zero_he_bssid(&mut self) {
        let control = &mut self.mac.wifi_mac_he_init_suffix;
        // Keep the four fresh-read RMW edges distinct and in blob order.
        control.modify_multi_bssid_control(|w| {
            w.set_he_bssid_enable(true);
        });
        control.modify_multi_bssid_control(|w| {
            w.set_bssid_select(Bits::truncate(
                oer_esp32s31_pac::wifi_mac::multi_bssid_control::BssidSelect::Interface0.bits(),
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

    /// Disable the HE RTS threshold in logical queue order, retaining the
    /// byte threshold and the software RTS request.
    ///
    /// SOURCE: complete `hal_he_disable_rts_threshold`.
    fn disable_he_rts_threshold(&mut self) {
        for queue in 0..ORDINARY_QUEUE_COUNT {
            let bank = ordinary_bank(queue).expect("every ordinary queue has a bank");
            self.mac
                .wifi_mac_tx_queue_vector
                .modify_he_rts_control(bank, |w| {
                    w.set_he_rts_disabled(true);
                });
        }
    }

    /// Program the finite HE20 receive-side state reached by the pinned peer
    /// capability and operation parsers.
    ///
    /// A finite RTS threshold fails closed: its vendor table builder contains
    /// a separate floating-point loop that has not been promoted yet.
    pub fn program_he20_peer(
        &mut self,
        config: MacHe20PeerConfig,
        rts_threshold: Option<u16>,
    ) -> Result<(), MacHe20PeerError> {
        if rts_threshold.is_some() {
            return Err(MacHe20PeerError::UnsupportedRtsThreshold);
        }

        self.initialize_interface_zero_he_bssid();
        if config.bss_color.get() != 0 || !config.bss_color_enabled {
            let color = if config.bss_color_enabled {
                config.bss_color.get()
            } else {
                0
            };
            self.mac
                .wifi_mac_he_init_suffix
                .modify_multi_bssid_control(|w| {
                    w.set_bss_color(bits(color))
                        .set_bss_color_enable(config.bss_color_enabled)
                        .set_partial_bss_color_enable(config.partial_bss_color);
                });
        }

        self.mac
            .wifi_mac_he_init_prefix
            .modify_rx_field_control(|w| {
                w.set_color_bitmap_clear(true);
            });
        let init = &mut self.mac.wifi_mac_he_init_suffix;
        init.modify_he_default_control(|w| {
            w.set_default_pe_duration(bits(config.default_packet_extension_duration.get()));
        });

        let duration = bits(config.packet_padding_duration.get());
        init.modify_he_packet_padding(|w| {
            w.set_bpsk_duration(duration)
                .set_qpsk_duration(duration)
                .set_qam16_duration(duration)
                .set_qam64_duration(duration)
                .set_qam256_duration(duration);
        });

        self.disable_he_rts_threshold();

        let init = &mut self.mac.wifi_mac_he_init_suffix;
        init.modify_ersu_and_vht_control(|w| {
            w.set_auto_ack_allow_ersu(!config.extended_range_single_user_disabled);
        });
        if !config.extended_range_single_user_disabled {
            // The complete ER-SU-permitted leaf writes all four baseline
            // ACK-rate bytes to 0x80 in one store; the four bytes are every
            // bit of the word.
            init.modify_ersu_ack_rate(|w| {
                w.set_rate_0(
                    oer_esp32s31_pac::wifi_mac::ersu_ack_rate::Rate0::Ordinary.bits() as u8,
                )
                .set_rate_1(oer_esp32s31_pac::wifi_mac::ersu_ack_rate::Rate1::Ordinary.bits() as u8)
                .set_rate_2(oer_esp32s31_pac::wifi_mac::ersu_ack_rate::Rate2::Ordinary.bits() as u8)
                .set_rate_3(
                    oer_esp32s31_pac::wifi_mac::ersu_ack_rate::Rate3::Ordinary.bits() as u8,
                );
            });
        }
        Ok(())
    }

    /// Install interface-zero HE association state after a successful
    /// Association Response.
    pub fn program_he20_association(
        &mut self,
        association_id: MacAssociationId,
        minimum_mpdu_start_spacing: MacMinimumMpduStartSpacing,
        bssid_index: u8,
    ) {
        let association: Bits<11> = bits(association_id.get());
        self.mac
            .wifi_mac_bssid_policy
            .modify_bssid_high(element(0), |w| {
                w.set_minimum_mpdu_start_spacing(bits(minimum_mpdu_start_spacing.get()))
                    .set_association_id(association);
            });

        let init = &mut self.mac.wifi_mac_he_init_suffix;
        init.modify_broadcast_ru_low(|w| {
            w.set_association_id(association)
                .set_enable(true)
                .set_value(bits(u32::from(bssid_index)));
        });

        init.modify_broadcast_ru_high(|w| {
            w.set_low_enable(true)
                .set_low_value(Bits::constant::<0>())
                .set_high_enable(true)
                .set_high_value(Bits::constant::<0>());
        });
    }
}

#[cfg(test)]
mod tests;
