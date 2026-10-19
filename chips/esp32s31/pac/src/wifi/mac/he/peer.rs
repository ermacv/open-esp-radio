//! Generated-PAC ownership for the finite HE20 peer/association leaves.

#![forbid(unsafe_code)]

use crate::{
    MacAssociationId, MacHeBssColor, MacHeDefaultPacketExtensionDuration,
    MacHePacketPaddingDuration, MacMinimumMpduStartSpacing, WifiRadioRegisters,
};

/// Hardware-visible subset of one parsed HE20 peer.
///
/// SOURCE: complete pinned `libnet80211.a[ieee80211_he.o]`
/// `ieee80211_parse_hecap`/`ieee80211_parse_heopr` callers and complete
/// `libpp.a[hal_mac_ctl.o]` leaves. The protocol crate owns parsing;
/// this type keeps the chip-specific register transform inside the PAC.
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

impl WifiRadioRegisters {
    /// Enable interface-zero HE BSSID matching before installing peer state.
    ///
    /// SOURCE: complete `libpp.a[hal_mac_ctl.o]::
    /// hal_he_bssid_init`, size `0x4c`, tail-calling the complete
    /// `hal_he_set_power_save`, size `0x36`. The associated-STA path in
    /// `libnet80211.a[wl_cnx.o]::cnx_connect_to_bss`, size `0x2b6`,
    /// calls this exact interface-zero leaf before clearing the color bitmap
    /// and programming the parsed HE Operation BSS color.
    fn initialize_interface_zero_he_bssid(&mut self) {
        let control = self
            .peripherals
            .wifi_mac
            .wifi_mac_he_init_suffix
            .multi_bssid_control();
        // Keep the four fresh-read RMW edges distinct and in blob order.
        control.modify(|_, w| w.he_bssid_enable().set_bit());
        control.modify(|_, w| w.bssid_select().set(0));

        let power_save = self.peripherals.wifi_mac.wifi_mac_rx_power_save.control();
        power_save.modify(|_, w| w.intra_ppdu_ps_enable().set_bit());
        power_save.modify(|_, w| w.intra_ps_check_bss_color_enable().set_bit());
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
        let init = &self.peripherals.wifi_mac.wifi_mac_he_init_suffix;
        if config.bss_color.get() != 0 || !config.bss_color_enabled {
            let register = init.multi_bssid_control();
            register.modify(|_, w| {
                w.bss_color()
                    .set(if config.bss_color_enabled {
                        config.bss_color.get() as u8
                    } else {
                        0
                    })
                    .bss_color_enable()
                    .bit(config.bss_color_enabled)
                    .partial_bss_color_enable()
                    .bit(config.partial_bss_color)
            });
        }

        self.peripherals
            .wifi_mac
            .wifi_mac_he_init_prefix
            .rx_field_control()
            .modify(|_, w| w.color_bitmap_clear().set_bit());
        init.he_default_control().modify(|_, w| {
            w.default_pe_duration()
                .set(config.default_packet_extension_duration.get() as u8)
        });

        let duration = config.packet_padding_duration.get() as u8;
        init.he_packet_padding().modify(|_, w| {
            w.bpsk_duration()
                .set(duration)
                .qpsk_duration()
                .set(duration)
                .qam16_duration()
                .set(duration)
                .qam64_duration()
                .set(duration)
                .qam256_duration()
                .set(duration)
        });

        let queues = &self.peripherals.wifi_mac.wifi_mac_tx_queue_vector;
        crate::wifi::mac::tx::queue::disable_he_rts_threshold(queues);

        init.ersu_and_vht_control().modify(|_, w| {
            w.auto_ack_allow_ersu()
                .bit(!config.extended_range_single_user_disabled)
        });
        if !config.extended_range_single_user_disabled {
            // The complete ER-SU-permitted leaf writes all four baseline
            // ACK-rate bytes to 0x80.
            crate::svd::zero_based_field_write::ersu_ack_rate_baseline(
                init, 0x80, 0x80, 0x80, 0x80,
            );
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
        self.peripherals
            .wifi_mac
            .wifi_mac_bssid_policy
            .bssid_high(0)
            .modify(|_, w| {
                w.minimum_mpdu_start_spacing()
                    .set(minimum_mpdu_start_spacing.get() as u8)
                    .association_id()
                    .set(association_id.get() as u16)
            });

        let init = &self.peripherals.wifi_mac.wifi_mac_he_init_suffix;
        let broadcast_low = init.broadcast_ru_low();
        broadcast_low.modify(|_, w| {
            w.association_id()
                .set(association_id.get() as u16)
                .enable()
                .set_bit()
                .value()
                .set(u16::from(bssid_index))
        });

        let broadcast_high = init.broadcast_ru_high();
        broadcast_high.modify(|_, w| {
            w.low_enable()
                .set_bit()
                .low_value()
                .set(0)
                .high_enable()
                .set_bit()
                .high_value()
                .set(0)
        });
    }
}
