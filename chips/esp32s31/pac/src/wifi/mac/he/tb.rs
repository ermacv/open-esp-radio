//! Typed ownership of HE trigger-based control and diagnostics.

use crate::WifiRadioRegisters;

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

impl WifiRadioRegisters {
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
        let control = self
            .peripherals
            .wifi_mac
            .wifi_mac_he_init_suffix
            .he_default_control();
        control.modify(|_, w| w.ul_mu_disable().bit(ul_mu_disabled));
        control.modify(|_, w| w.ul_mu_data_disable().bit(ul_mu_data_disabled));
    }

    /// Read the instruction-exact HE Trigger/TB hardware counters.
    ///
    /// SOURCE: complete pinned `libpp.a[hal_mac_ctl.o]`
    /// `hal_he_get_rx_trigger_cnt`, `hal_he_get_tb_times_cnt` and
    /// `hal_he_get_tb_qosnull_cnt`.
    pub fn he_trigger_based_statistics(&self) -> MacHeTbStatistics {
        let statistics = &self.peripherals.wifi_mac.wifi_mac_he_tb_statistics;
        let transmission = statistics.tb_transmission().read();
        MacHeTbStatistics {
            rx_trigger_count: statistics.rx_trigger().read().count().bits(),
            transmission_count: transmission.count().bits(),
            qos_null_count: transmission.qos_null_count().bits(),
        }
    }

    /// Read the last HE trigger-based TX calculation decoded by the blob.
    ///
    /// SOURCE: complete pinned `libpp.a[hal_debug.o]`
    /// `dbg_read_axtb_diag` and its `WDEVAXTBDIAG0..3` format strings.
    pub fn he_trigger_based_tx_diagnostics(&self) -> MacHeTbTxDiagnostics {
        let diagnostics = &self.peripherals.wifi_mac.wifi_mac_he_tb_diagnostics;
        let timing = diagnostics.timing().read();
        let psdu = diagnostics.psdu().read();
        let trigger = diagnostics.trigger().read();
        let user = diagnostics.user().read();

        MacHeTbTxDiagnostics {
            tx_time: timing.tx_time().bits(),
            symbol_count: timing.symbol_count().bits(),
            pre_fec_padding_phy: timing.pre_fec_padding_phy().bits(),
            psdu_length: psdu.psdu_length().bits(),
            minimum_subframe_length: psdu.minimum_subframe_length().bits(),
            packet_extension_time: psdu.packet_extension_time().bits(),
            tx_20_packet_count: trigger.tx_20_packet_count().bits(),
            qos_null_append_count: trigger.qos_null_append_count().bits(),
            trigger_type: trigger.trigger_type().bits(),
            uplink_length: trigger.uplink_length().bits(),
            gi_and_ltf: trigger.gi_and_ltf().bits(),
            tid_limit: trigger.tid_limit().bits(),
            association_id: user.association_id().bits(),
            ru_allocation: user.ru_allocation().bits(),
            uplink_mcs: user.uplink_mcs().bits(),
            basic_preferred_ac: user.basic_preferred_ac().bits(),
            basic_spacing_factor: user.basic_spacing_factor().bits(),
            uplink_packet_extension: user.uplink_packet_extension().bits(),
        }
    }
}
