//! The complete post-power `hal_he_init` suffix and the HE BSR, ER-SU and
//! MPDU-length-table leaves beside it.

use oer_chips_esp32s31_pac::wifi_mac::PhyFrequencyChannelOracleWifiMac;
use oer_portable_mmio::{Bits, Placement};

use crate::radio::ieee80211::registers::{
    MacPlacement, WifiRadioRegisters, byte, element, halfword,
};

/// The entries of the hardware TX MPDU-length link table.
const MPDU_LENGTH_LINK_COUNT: usize = oer_chips_esp32s31_pac::wifi_mac::he_scratch::COUNT;
// The init clears every link word by one `clear_he_mpdu_length_links`
// step each, written out below.
const _: () = {
    if MPDU_LENGTH_LINK_COUNT != 120 {
        panic!("the HE init clears each of the 120 MPDU-length link words");
    }
};

/// One hardware TX MPDU-length link-table entry.
///
/// SOURCE: complete `libpp[hal_debug.o]::dbg_read_tx_mplen`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MacHeTxMpduLengthLink {
    pub mpdu_length: u16,
    pub next_link: u8,
}

impl<M: MacPlacement> WifiRadioRegisters<M> {
    /// Clear software CTS requests during cold initialization, retaining RTS.
    ///
    /// SOURCE: `hal_he_init` offsets 0xb6..0xec, four ordered queue RMWs.
    pub(crate) fn initialize_mac_software_cts(&mut self) {
        // Physical protection words are traversed high-to-low.
        for physical in (0..4).rev() {
            self.mac
                .wifi_mac_tx_queue_control
                .modify_protection(element(physical), |w| {
                    w.set_software_cts(false);
                });
        }
    }

    /// Sample one of the 120 linked TX MPDU-length entries.
    pub fn he_tx_mpdu_length_link(&self, index: u8) -> Option<MacHeTxMpduLengthLink> {
        let index =
            oer_portable_mmio::Index::<MPDU_LENGTH_LINK_COUNT>::new(usize::from(index)).ok()?;
        let value = self.mac.wifi_mac_he_init_suffix.read_he_scratch(index);
        Some(MacHeTxMpduLengthLink {
            mpdu_length: halfword(value.mpdu_length()),
            next_link: byte(value.next_link()),
        })
    }

    /// Apply the complete HE Buffer Status Report initialization leaf.
    ///
    /// SOURCE: complete pinned `libpp.a[hal_mac_ctl.o]`
    /// `hal_he_bsr_init`, size `0x50`. All eight fresh-read RMW edges and
    /// their order are preserved. This leaf is reached later by the vendor
    /// STA-node setup, not by `hal_he_init`; omitting that second lifecycle
    /// phase produced `0x2010_4c7c = 0x1082_3c00` instead of the vendor auth
    /// image whose BSR portion is `0x0000_81c1`.
    pub fn initialize_he_buffer_status_report(&mut self) {
        let init = &mut self.mac.wifi_mac_he_init_suffix;
        init.modify_ersu_and_vht_control(|w| {
            w.set_bsr_enable(true);
        });
        init.modify_he_default_control(|w| {
            w.set_bsr_update_enable(true);
        });
        init.modify_ersu_and_vht_control(|w| {
            w.set_ignore_valid_edca(true);
        });
        init.modify_ersu_and_vht_control(|w| {
            w.set_ignore_valid_tb(true);
        });
        init.modify_ersu_and_vht_control(|w| {
            w.set_qos_data_update_bsr(true);
        });
        init.modify_ersu_and_vht_control(|w| {
            w.set_preac_no_resource_enable_bsr(false);
        });
        init.modify_ersu_and_vht_control(|w| {
            w.set_ac_empty_single_bsr(false);
        });
        init.modify_ersu_and_vht_control(|w| {
            w.set_bsr_aci_high_comparison_method(Bits::truncate(oer_chips_esp32s31_pac::wifi_mac::ersu_and_vht_control::BsrAciHighComparisonMethod::Method1.bits()));
        });
    }

    /// Apply the raw ER-SU-Disable bit parsed from the BSS's HE Operation.
    ///
    /// SOURCE: complete pinned `libpp.a[hal_mac_ctl.o]`
    /// `libpp[hal_mac_ctl.o]::hal_he_set_ersu` and
    /// `libnet80211.a[ieee80211_he.o]`
    /// `ieee80211_parse_heopr`, whose format string names complete-IE byte
    /// five bit zero `ER-SU-Disable`. A true argument clears the blob-named
    /// `AUTO_ACK_ALLOW_ERSU` bit; false sets it. The permitted leaf also
    /// rewrites ACK-rate bytes already initialized by the cold HE suffix.
    pub fn set_he_extended_range_single_user_disabled(&mut self, disabled: bool) {
        self.mac
            .wifi_mac_he_init_suffix
            .modify_ersu_and_vht_control(|w| {
                w.set_auto_ack_allow_ersu(!disabled);
            });
    }

    /// Apply the complete hardware-visible `hal_he_init` suffix.
    ///
    /// SOURCE: complete pinned `libpp.a[hal_mac_ctl.o]` parent and
    /// reached leaves recorded as `BLOB_LIBPP_HAL_HE_INIT_SUFFIX`. The order
    /// below is 163 writes/RMWs plus both conditional multi-BSSID guard reads.
    /// It starts after the separate `dbg_read_tx_power` traversal. `channel`
    /// is the shared PHY's channel TX-offset word, which the leaf updates
    /// twice.
    pub fn initialize_mac_he_suffix<C: Placement>(
        &mut self,
        channel: &mut PhyFrequencyChannelOracleWifiMac<C>,
    ) {
        let init = &mut self.mac.wifi_mac_he_init_suffix;

        // Complete hal_he_set_ersu(0), followed by its complete
        // hal_he_set_ersu_ack_rate(0) child. The four bytes are deliberately
        // four fresh-read RMWs rather than one full-word write.
        init.modify_ersu_and_vht_control(|w| {
            w.set_auto_ack_allow_ersu(true);
        });
        init.modify_ersu_ack_rate(|w| {
            w.set_rate_0(
                oer_chips_esp32s31_pac::wifi_mac::ersu_ack_rate::Rate0::Ordinary.bits() as u8,
            );
        });
        init.modify_ersu_ack_rate(|w| {
            w.set_rate_1(
                oer_chips_esp32s31_pac::wifi_mac::ersu_ack_rate::Rate1::Ordinary.bits() as u8,
            );
        });
        init.modify_ersu_ack_rate(|w| {
            w.set_rate_2(
                oer_chips_esp32s31_pac::wifi_mac::ersu_ack_rate::Rate2::Ordinary.bits() as u8,
            );
        });
        init.modify_ersu_ack_rate(|w| {
            w.set_rate_3(
                oer_chips_esp32s31_pac::wifi_mac::ersu_ack_rate::Rate3::Ordinary.bits() as u8,
            );
        });

        // Complete hal_set_tx_min_pwr(-11), then the parent field update.
        channel.modify_channel_tx_offset_control(|w| {
            w.set_minimum_power_low(Bits::constant::<5>())
                .set_minimum_power_high(Bits::constant::<3>());
        });
        init.modify_he_default_control(|w| {
            w.set_mpdu_length_offset(Bits::constant::<0x17c>());
        });

        // SOURCE: BLOB_LIBPP_HAL_HE_INIT_PIN_20261108, `hal_he_init`
        // +0xa0..0xb2: a whole zero word to each of the 120 MPDU-length link
        // words, in ascending order (`clear_he_mpdu_length_links`). The older
        // oracle (BLOB_LIBPP_HAL_HE_INIT_SUFFIX) kept bits 31:21.
        init.clear_he_mpdu_length_links_he_scratch0();
        init.clear_he_mpdu_length_links_he_scratch1();
        init.clear_he_mpdu_length_links_he_scratch2();
        init.clear_he_mpdu_length_links_he_scratch3();
        init.clear_he_mpdu_length_links_he_scratch4();
        init.clear_he_mpdu_length_links_he_scratch5();
        init.clear_he_mpdu_length_links_he_scratch6();
        init.clear_he_mpdu_length_links_he_scratch7();
        init.clear_he_mpdu_length_links_he_scratch8();
        init.clear_he_mpdu_length_links_he_scratch9();
        init.clear_he_mpdu_length_links_he_scratch10();
        init.clear_he_mpdu_length_links_he_scratch11();
        init.clear_he_mpdu_length_links_he_scratch12();
        init.clear_he_mpdu_length_links_he_scratch13();
        init.clear_he_mpdu_length_links_he_scratch14();
        init.clear_he_mpdu_length_links_he_scratch15();
        init.clear_he_mpdu_length_links_he_scratch16();
        init.clear_he_mpdu_length_links_he_scratch17();
        init.clear_he_mpdu_length_links_he_scratch18();
        init.clear_he_mpdu_length_links_he_scratch19();
        init.clear_he_mpdu_length_links_he_scratch20();
        init.clear_he_mpdu_length_links_he_scratch21();
        init.clear_he_mpdu_length_links_he_scratch22();
        init.clear_he_mpdu_length_links_he_scratch23();
        init.clear_he_mpdu_length_links_he_scratch24();
        init.clear_he_mpdu_length_links_he_scratch25();
        init.clear_he_mpdu_length_links_he_scratch26();
        init.clear_he_mpdu_length_links_he_scratch27();
        init.clear_he_mpdu_length_links_he_scratch28();
        init.clear_he_mpdu_length_links_he_scratch29();
        init.clear_he_mpdu_length_links_he_scratch30();
        init.clear_he_mpdu_length_links_he_scratch31();
        init.clear_he_mpdu_length_links_he_scratch32();
        init.clear_he_mpdu_length_links_he_scratch33();
        init.clear_he_mpdu_length_links_he_scratch34();
        init.clear_he_mpdu_length_links_he_scratch35();
        init.clear_he_mpdu_length_links_he_scratch36();
        init.clear_he_mpdu_length_links_he_scratch37();
        init.clear_he_mpdu_length_links_he_scratch38();
        init.clear_he_mpdu_length_links_he_scratch39();
        init.clear_he_mpdu_length_links_he_scratch40();
        init.clear_he_mpdu_length_links_he_scratch41();
        init.clear_he_mpdu_length_links_he_scratch42();
        init.clear_he_mpdu_length_links_he_scratch43();
        init.clear_he_mpdu_length_links_he_scratch44();
        init.clear_he_mpdu_length_links_he_scratch45();
        init.clear_he_mpdu_length_links_he_scratch46();
        init.clear_he_mpdu_length_links_he_scratch47();
        init.clear_he_mpdu_length_links_he_scratch48();
        init.clear_he_mpdu_length_links_he_scratch49();
        init.clear_he_mpdu_length_links_he_scratch50();
        init.clear_he_mpdu_length_links_he_scratch51();
        init.clear_he_mpdu_length_links_he_scratch52();
        init.clear_he_mpdu_length_links_he_scratch53();
        init.clear_he_mpdu_length_links_he_scratch54();
        init.clear_he_mpdu_length_links_he_scratch55();
        init.clear_he_mpdu_length_links_he_scratch56();
        init.clear_he_mpdu_length_links_he_scratch57();
        init.clear_he_mpdu_length_links_he_scratch58();
        init.clear_he_mpdu_length_links_he_scratch59();
        init.clear_he_mpdu_length_links_he_scratch60();
        init.clear_he_mpdu_length_links_he_scratch61();
        init.clear_he_mpdu_length_links_he_scratch62();
        init.clear_he_mpdu_length_links_he_scratch63();
        init.clear_he_mpdu_length_links_he_scratch64();
        init.clear_he_mpdu_length_links_he_scratch65();
        init.clear_he_mpdu_length_links_he_scratch66();
        init.clear_he_mpdu_length_links_he_scratch67();
        init.clear_he_mpdu_length_links_he_scratch68();
        init.clear_he_mpdu_length_links_he_scratch69();
        init.clear_he_mpdu_length_links_he_scratch70();
        init.clear_he_mpdu_length_links_he_scratch71();
        init.clear_he_mpdu_length_links_he_scratch72();
        init.clear_he_mpdu_length_links_he_scratch73();
        init.clear_he_mpdu_length_links_he_scratch74();
        init.clear_he_mpdu_length_links_he_scratch75();
        init.clear_he_mpdu_length_links_he_scratch76();
        init.clear_he_mpdu_length_links_he_scratch77();
        init.clear_he_mpdu_length_links_he_scratch78();
        init.clear_he_mpdu_length_links_he_scratch79();
        init.clear_he_mpdu_length_links_he_scratch80();
        init.clear_he_mpdu_length_links_he_scratch81();
        init.clear_he_mpdu_length_links_he_scratch82();
        init.clear_he_mpdu_length_links_he_scratch83();
        init.clear_he_mpdu_length_links_he_scratch84();
        init.clear_he_mpdu_length_links_he_scratch85();
        init.clear_he_mpdu_length_links_he_scratch86();
        init.clear_he_mpdu_length_links_he_scratch87();
        init.clear_he_mpdu_length_links_he_scratch88();
        init.clear_he_mpdu_length_links_he_scratch89();
        init.clear_he_mpdu_length_links_he_scratch90();
        init.clear_he_mpdu_length_links_he_scratch91();
        init.clear_he_mpdu_length_links_he_scratch92();
        init.clear_he_mpdu_length_links_he_scratch93();
        init.clear_he_mpdu_length_links_he_scratch94();
        init.clear_he_mpdu_length_links_he_scratch95();
        init.clear_he_mpdu_length_links_he_scratch96();
        init.clear_he_mpdu_length_links_he_scratch97();
        init.clear_he_mpdu_length_links_he_scratch98();
        init.clear_he_mpdu_length_links_he_scratch99();
        init.clear_he_mpdu_length_links_he_scratch100();
        init.clear_he_mpdu_length_links_he_scratch101();
        init.clear_he_mpdu_length_links_he_scratch102();
        init.clear_he_mpdu_length_links_he_scratch103();
        init.clear_he_mpdu_length_links_he_scratch104();
        init.clear_he_mpdu_length_links_he_scratch105();
        init.clear_he_mpdu_length_links_he_scratch106();
        init.clear_he_mpdu_length_links_he_scratch107();
        init.clear_he_mpdu_length_links_he_scratch108();
        init.clear_he_mpdu_length_links_he_scratch109();
        init.clear_he_mpdu_length_links_he_scratch110();
        init.clear_he_mpdu_length_links_he_scratch111();
        init.clear_he_mpdu_length_links_he_scratch112();
        init.clear_he_mpdu_length_links_he_scratch113();
        init.clear_he_mpdu_length_links_he_scratch114();
        init.clear_he_mpdu_length_links_he_scratch115();
        init.clear_he_mpdu_length_links_he_scratch116();
        init.clear_he_mpdu_length_links_he_scratch117();
        init.clear_he_mpdu_length_links_he_scratch118();
        init.clear_he_mpdu_length_links_he_scratch119();

        self.initialize_mac_software_cts();

        self.mac.wifi_mac_txrx_prefix.modify_mode_control(|w| {
            w.set_bit_2_opaque(true);
        });
        let init = &mut self.mac.wifi_mac_he_init_suffix;
        init.modify_he_init_suffix_shared_enable_control(|w| {
            w.set_bit_31_opaque(true);
        });
        init.modify_he_init_suffix_feature_edges(|w| {
            w.set_enable_1(true);
        });
        init.modify_he_init_suffix_feature_edges(|w| {
            w.set_enable_0(true);
        });
        init.modify_tx_mode_control(|w| {
            w.set_beacon_filter_frame_crc_state(Bits::constant::<1>());
        });

        // Complete hal_he_set_bcast_ru(0x7fd, 0, 0): six independent RMWs.
        init.modify_broadcast_ru_low(|w| {
            w.set_enable(true);
        });
        init.modify_broadcast_ru_low(|w| {
            w.set_value(Bits::constant::<0x7fd>());
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

        // Complete hal_he_set_uora_parameter with packed argument byte 0x2b.
        init.modify_uora_control(|w| {
            w.set_low_window(Bits::constant::<7>());
        });
        init.modify_uora_control(|w| {
            w.set_high_window(Bits::constant::<31>());
        });

        channel.modify_channel_tx_offset_control(|w| {
            w.set_bit_17_opaque(true);
        });
        init.modify_dump_complete_hesigb(|w| {
            w.set_enable(false);
        });
        // SOURCE: BLOB_MAC_VHT_BW_SIGNALING_20261103. The current pinned
        // leaf changes the named enable and preserves the opaque bit 22.
        init.modify_ersu_and_vht_control(|w| {
            w.set_vht_bw_signaling_rts(false);
        });

        // Two parent RMWs precede the complete clear leaf.
        init.modify_multi_bssid_control(|w| {
            w.set_multi_bssid_enable(false);
        });
        init.modify_multi_bssid_control(|w| {
            w.set_co_hosted_enable(false);
        });

        // Complete hal_he_clr_multi_bssid. Preserve its explicit guard read
        // and its repeated ENABLE update even though the parent just cleared
        // the same bit.
        if !init.read_multi_bssid_control().co_hosted_enable() {
            init.modify_multi_bssid_control(|w| {
                w.set_multi_bssid_enable(false);
            });
            init.modify_multi_bssid_control(|w| {
                w.set_multi_bssid_mask(0xff);
            });
            init.modify_multi_bssid_control(|w| {
                w.set_bssid_byte_5(0);
            });
            init.modify_multi_bssid_high(|w| {
                w.set_bssid_index(0);
            });
            for physical in (0..8).rev() {
                init.modify_queue_control(element(physical), |w| {
                    w.set_qos_null_to_translated_bss(false);
                });
            }
        }

        // Complete hal_he_set_co_hosted_bss(0, 0), including its independent
        // guard read and the repeated hosted-mask OR.
        if !init.read_multi_bssid_control().multi_bssid_enable() {
            init.modify_multi_bssid_control(|w| {
                w.set_co_hosted_enable(false);
            });
            init.modify_multi_bssid_control(|w| {
                w.set_multi_bssid_mask(0xff);
            });
        }
    }
}

#[cfg(test)]
mod tests;
