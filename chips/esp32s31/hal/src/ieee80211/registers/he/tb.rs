//! Typed ownership of HE trigger-based control and diagnostics.

use crate::ieee80211::registers::{MacPlacement, WifiRadioRegisters, byte, halfword};

/// Monotonic hardware counters for HE Trigger reception and TB responses.
///
/// Counter wrap is hardware-defined; callers should compare snapshots with
/// wrapping subtraction.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MacHeTbStatistics {
    /// Trigger frames accepted by the MAC.
    pub rx_trigger_count: u16,
    /// Trigger-based transmission opportunities entered by the MAC.
    pub transmission_count: u16,
    /// Trigger responses for which the MAC appended a QoS Null frame.
    pub qos_null_count: u16,
}

/// Best-effort snapshot of the last HE trigger-based TX calculation.
///
/// The four hardware words are read independently and are not documented as a
/// latched transaction. Take snapshots only for diagnostics, or compare two
/// snapshots around a known Trigger event.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MacHeTbTxDiagnostics {
    pub tx_time: u16,
    pub symbol_count: u16,
    pub pre_fec_padding_phy: u8,
    pub psdu_length: u32,
    pub minimum_subframe_length: u16,
    pub packet_extension_time: u8,
    pub tx_20_packet_count: u8,
    pub qos_null_append_count: u8,
    pub trigger_type: u8,
    pub uplink_length: u16,
    pub gi_and_ltf: u8,
    pub tid_limit: u8,
    pub association_id: u16,
    pub ru_allocation: u8,
    pub uplink_mcs: u8,
    pub basic_preferred_ac: u8,
    pub basic_spacing_factor: u8,
    pub uplink_packet_extension: u8,
}

impl<M: MacPlacement> WifiRadioRegisters<M> {
    /// Apply the two HE Operating Mode Control UL-MU disable flags.
    ///
    /// SOURCE: complete pinned `libnet80211.a[ieee80211_he.o]`
    /// `wifi_htc_omc_txcb` and `libpp.a[hal_mac_ctl.o]`
    /// `hal_he_set_ul_mu`. The callback extracts `UL MU Disable` and
    /// `UL MU Data Disable` from HT-Control bits 11 and 17. The HAL publishes
    /// them to bits 31 and 30 of `0x2010_4c80` through two independent
    /// fresh-read RMWs, preserved below.
    pub fn set_he_uplink_multi_user_disable(
        &mut self,
        ul_mu_disabled: bool,
        ul_mu_data_disabled: bool,
    ) {
        let control = &mut self.mac.wifi_mac_he_init_suffix;
        control.modify_he_default_control(|w| {
            w.set_ul_mu_disable(ul_mu_disabled);
        });
        control.modify_he_default_control(|w| {
            w.set_ul_mu_data_disable(ul_mu_data_disabled);
        });
    }

    /// Read the instruction-exact HE Trigger/TB hardware counters.
    ///
    /// SOURCE: complete pinned `libpp.a[hal_mac_ctl.o]`
    /// `hal_he_get_rx_trigger_cnt`, `hal_he_get_tb_times_cnt` and
    /// `hal_he_get_tb_qosnull_cnt`.
    pub fn he_trigger_based_statistics(&self) -> MacHeTbStatistics {
        let statistics = &self.mac.wifi_mac_he_tb_statistics;
        let transmission = statistics.read_tb_transmission();
        MacHeTbStatistics {
            rx_trigger_count: statistics.read_rx_trigger().count(),
            transmission_count: transmission.count(),
            qos_null_count: transmission.qos_null_count(),
        }
    }

    /// Read the last HE trigger-based TX calculation decoded by the blob.
    ///
    /// SOURCE: complete pinned `libpp.a[hal_debug.o]`
    /// `dbg_read_axtb_diag` and its `WDEVAXTBDIAG0..3` format strings.
    pub fn he_trigger_based_tx_diagnostics(&self) -> MacHeTbTxDiagnostics {
        let diagnostics = &self.mac.wifi_mac_he_tb_diagnostics;
        let timing = diagnostics.read_timing();
        let psdu = diagnostics.read_psdu();
        let trigger = diagnostics.read_he_tb_diagnostics_trigger();
        let user = diagnostics.read_user();

        MacHeTbTxDiagnostics {
            tx_time: timing.tx_time(),
            symbol_count: halfword(timing.symbol_count()),
            pre_fec_padding_phy: byte(timing.pre_fec_padding_phy()),
            psdu_length: psdu.psdu_length().get(),
            minimum_subframe_length: halfword(psdu.minimum_subframe_length()),
            packet_extension_time: byte(psdu.packet_extension_time()),
            tx_20_packet_count: byte(trigger.tx_20_packet_count()),
            qos_null_append_count: byte(trigger.qos_null_append_count()),
            trigger_type: byte(trigger.trigger_type()),
            uplink_length: halfword(trigger.uplink_length()),
            gi_and_ltf: byte(trigger.gi_and_ltf()),
            tid_limit: byte(trigger.tid_limit()),
            association_id: halfword(user.association_id()),
            ru_allocation: byte(user.ru_allocation()),
            uplink_mcs: byte(user.uplink_mcs()),
            basic_preferred_ac: byte(user.basic_preferred_ac()),
            basic_spacing_factor: byte(user.basic_spacing_factor()),
            uplink_packet_extension: byte(user.uplink_packet_extension()),
        }
    }
}

#[cfg(test)]
mod tests;
