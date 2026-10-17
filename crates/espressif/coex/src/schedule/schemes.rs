//! Recovered coexistence schemes.
//!
//! Every scheme is the complete `coex_schm_<name>` object of esp-coex-lib
//! c758e7b56e0fa22177a0539796e1df59978dc322 (`esp32s31/libcoexist.a` sha256
//! 13b1e1d2a1550400ddb2622648933288aee6a285d3aad454978314c4af685147,
//! `coexist_scheme.o`, one `.rodata.coex_schm_<name>` section each). The
//! vendor image is one phase-count byte, one period byte and four bytes per
//! phase; this module keeps the same values as typed records.

use super::{CoexPhase, CoexScheme};

/// One recovered vendor scheme.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CoexSchemeId {
    /// `coex_schm_all_default`.
    AllDefault,
    /// `coex_schm_ble_default_bt_a2dp_wifi_conn`.
    BleDefaultBtA2dpWifiConn,
    /// `coex_schm_ble_default_bt_a2dp_wifi_connecting`.
    BleDefaultBtA2dpWifiConnecting,
    /// `coex_schm_ble_default_bt_a2dp_wifi_default`.
    BleDefaultBtA2dpWifiDefault,
    /// `coex_schm_ble_default_bt_a2dp_wifi_scan`.
    BleDefaultBtA2dpWifiScan,
    /// `coex_schm_ble_default_bt_default_wifi_conn`.
    BleDefaultBtDefaultWifiConn,
    /// `coex_schm_ble_default_bt_default_wifi_connecting`.
    BleDefaultBtDefaultWifiConnecting,
    /// `coex_schm_ble_default_bt_default_wifi_scan`.
    BleDefaultBtDefaultWifiScan,
    /// `coex_schm_ble_default_bt_idle_wifi_conn`.
    BleDefaultBtIdleWifiConn,
    /// `coex_schm_ble_default_bt_idle_wifi_connecting`.
    BleDefaultBtIdleWifiConnecting,
    /// `coex_schm_ble_default_bt_idle_wifi_default`.
    BleDefaultBtIdleWifiDefault,
    /// `coex_schm_ble_default_bt_idle_wifi_scan`.
    BleDefaultBtIdleWifiScan,
    /// `coex_schm_ble_idle_bt_idle_wifi_default`.
    BleIdleBtIdleWifiDefault,
    /// `coex_schm_ble_mesh_config_bt_a2dp_paused_wifi_conn`.
    BleMeshConfigBtA2dpPausedWifiConn,
    /// `coex_schm_ble_mesh_config_bt_a2dp_paused_wifi_connecting`.
    BleMeshConfigBtA2dpPausedWifiConnecting,
    /// `coex_schm_ble_mesh_config_bt_a2dp_paused_wifi_scan`.
    BleMeshConfigBtA2dpPausedWifiScan,
    /// `coex_schm_ble_mesh_config_bt_a2dp_wifi_conn`.
    BleMeshConfigBtA2dpWifiConn,
    /// `coex_schm_ble_mesh_config_bt_a2dp_wifi_connecting`.
    BleMeshConfigBtA2dpWifiConnecting,
    /// `coex_schm_ble_mesh_config_bt_a2dp_wifi_scan`.
    BleMeshConfigBtA2dpWifiScan,
    /// `coex_schm_ble_mesh_config_bt_conn_wifi_conn`.
    BleMeshConfigBtConnWifiConn,
    /// `coex_schm_ble_mesh_config_bt_conn_wifi_connecting`.
    BleMeshConfigBtConnWifiConnecting,
    /// `coex_schm_ble_mesh_config_bt_conn_wifi_scan`.
    BleMeshConfigBtConnWifiScan,
    /// `coex_schm_ble_mesh_config_bt_default_wifi_conn`.
    BleMeshConfigBtDefaultWifiConn,
    /// `coex_schm_ble_mesh_config_bt_default_wifi_connecting`.
    BleMeshConfigBtDefaultWifiConnecting,
    /// `coex_schm_ble_mesh_config_bt_default_wifi_scan`.
    BleMeshConfigBtDefaultWifiScan,
    /// `coex_schm_ble_mesh_config_bt_piscan_wifi_conn`.
    BleMeshConfigBtPiscanWifiConn,
    /// `coex_schm_ble_mesh_config_bt_piscan_wifi_connecting`.
    BleMeshConfigBtPiscanWifiConnecting,
    /// `coex_schm_ble_mesh_config_bt_piscan_wifi_scan`.
    BleMeshConfigBtPiscanWifiScan,
    /// `coex_schm_ble_mesh_config_bt_sniff_sco_wifi_conn`.
    BleMeshConfigBtSniffScoWifiConn,
    /// `coex_schm_ble_mesh_config_bt_sniff_sco_wifi_connecting`.
    BleMeshConfigBtSniffScoWifiConnecting,
    /// `coex_schm_ble_mesh_config_bt_sniff_sco_wifi_scan`.
    BleMeshConfigBtSniffScoWifiScan,
    /// `coex_schm_ble_mesh_config_wifi_conn`.
    BleMeshConfigWifiConn,
    /// `coex_schm_ble_mesh_config_wifi_connecting`.
    BleMeshConfigWifiConnecting,
    /// `coex_schm_ble_mesh_config_wifi_scan`.
    BleMeshConfigWifiScan,
    /// `coex_schm_ble_mesh_standby_bt_a2dp_paused_wifi_conn`.
    BleMeshStandbyBtA2dpPausedWifiConn,
    /// `coex_schm_ble_mesh_standby_bt_a2dp_paused_wifi_connecting`.
    BleMeshStandbyBtA2dpPausedWifiConnecting,
    /// `coex_schm_ble_mesh_standby_bt_a2dp_paused_wifi_scan`.
    BleMeshStandbyBtA2dpPausedWifiScan,
    /// `coex_schm_ble_mesh_standby_bt_a2dp_wifi_conn`.
    BleMeshStandbyBtA2dpWifiConn,
    /// `coex_schm_ble_mesh_standby_bt_a2dp_wifi_connecting`.
    BleMeshStandbyBtA2dpWifiConnecting,
    /// `coex_schm_ble_mesh_standby_bt_a2dp_wifi_scan`.
    BleMeshStandbyBtA2dpWifiScan,
    /// `coex_schm_ble_mesh_standby_bt_conn_wifi_conn`.
    BleMeshStandbyBtConnWifiConn,
    /// `coex_schm_ble_mesh_standby_bt_conn_wifi_connecting`.
    BleMeshStandbyBtConnWifiConnecting,
    /// `coex_schm_ble_mesh_standby_bt_conn_wifi_scan`.
    BleMeshStandbyBtConnWifiScan,
    /// `coex_schm_ble_mesh_standby_bt_default_wifi_conn`.
    BleMeshStandbyBtDefaultWifiConn,
    /// `coex_schm_ble_mesh_standby_bt_default_wifi_connecting`.
    BleMeshStandbyBtDefaultWifiConnecting,
    /// `coex_schm_ble_mesh_standby_bt_default_wifi_scan`.
    BleMeshStandbyBtDefaultWifiScan,
    /// `coex_schm_ble_mesh_standby_bt_piscan_wifi_conn`.
    BleMeshStandbyBtPiscanWifiConn,
    /// `coex_schm_ble_mesh_standby_bt_piscan_wifi_connecting`.
    BleMeshStandbyBtPiscanWifiConnecting,
    /// `coex_schm_ble_mesh_standby_bt_piscan_wifi_scan`.
    BleMeshStandbyBtPiscanWifiScan,
    /// `coex_schm_ble_mesh_standby_bt_sniff_sco_wifi_conn`.
    BleMeshStandbyBtSniffScoWifiConn,
    /// `coex_schm_ble_mesh_standby_bt_sniff_sco_wifi_connecting`.
    BleMeshStandbyBtSniffScoWifiConnecting,
    /// `coex_schm_ble_mesh_standby_bt_sniff_sco_wifi_scan`.
    BleMeshStandbyBtSniffScoWifiScan,
    /// `coex_schm_ble_mesh_standby_wifi_conn`.
    BleMeshStandbyWifiConn,
    /// `coex_schm_ble_mesh_standby_wifi_connecting`.
    BleMeshStandbyWifiConnecting,
    /// `coex_schm_ble_mesh_standby_wifi_scan`.
    BleMeshStandbyWifiScan,
    /// `coex_schm_ble_mesh_traffic_bt_a2dp_paused_wifi_conn`.
    BleMeshTrafficBtA2dpPausedWifiConn,
    /// `coex_schm_ble_mesh_traffic_bt_a2dp_paused_wifi_connecting`.
    BleMeshTrafficBtA2dpPausedWifiConnecting,
    /// `coex_schm_ble_mesh_traffic_bt_a2dp_paused_wifi_scan`.
    BleMeshTrafficBtA2dpPausedWifiScan,
    /// `coex_schm_ble_mesh_traffic_bt_a2dp_wifi_conn`.
    BleMeshTrafficBtA2dpWifiConn,
    /// `coex_schm_ble_mesh_traffic_bt_a2dp_wifi_connecting`.
    BleMeshTrafficBtA2dpWifiConnecting,
    /// `coex_schm_ble_mesh_traffic_bt_a2dp_wifi_scan`.
    BleMeshTrafficBtA2dpWifiScan,
    /// `coex_schm_ble_mesh_traffic_bt_conn_wifi_conn`.
    BleMeshTrafficBtConnWifiConn,
    /// `coex_schm_ble_mesh_traffic_bt_conn_wifi_connecting`.
    BleMeshTrafficBtConnWifiConnecting,
    /// `coex_schm_ble_mesh_traffic_bt_conn_wifi_scan`.
    BleMeshTrafficBtConnWifiScan,
    /// `coex_schm_ble_mesh_traffic_bt_default_wifi_conn`.
    BleMeshTrafficBtDefaultWifiConn,
    /// `coex_schm_ble_mesh_traffic_bt_default_wifi_connecting`.
    BleMeshTrafficBtDefaultWifiConnecting,
    /// `coex_schm_ble_mesh_traffic_bt_default_wifi_scan`.
    BleMeshTrafficBtDefaultWifiScan,
    /// `coex_schm_ble_mesh_traffic_bt_piscan_wifi_conn`.
    BleMeshTrafficBtPiscanWifiConn,
    /// `coex_schm_ble_mesh_traffic_bt_piscan_wifi_connecting`.
    BleMeshTrafficBtPiscanWifiConnecting,
    /// `coex_schm_ble_mesh_traffic_bt_piscan_wifi_scan`.
    BleMeshTrafficBtPiscanWifiScan,
    /// `coex_schm_ble_mesh_traffic_bt_sniff_sco_wifi_conn`.
    BleMeshTrafficBtSniffScoWifiConn,
    /// `coex_schm_ble_mesh_traffic_bt_sniff_sco_wifi_connecting`.
    BleMeshTrafficBtSniffScoWifiConnecting,
    /// `coex_schm_ble_mesh_traffic_bt_sniff_sco_wifi_scan`.
    BleMeshTrafficBtSniffScoWifiScan,
    /// `coex_schm_ble_mesh_traffic_wifi_conn`.
    BleMeshTrafficWifiConn,
    /// `coex_schm_ble_mesh_traffic_wifi_connecting`.
    BleMeshTrafficWifiConnecting,
    /// `coex_schm_ble_mesh_traffic_wifi_scan`.
    BleMeshTrafficWifiScan,
    /// `coex_schm_bt_a2dp_paused_wifi_conn`.
    BtA2dpPausedWifiConn,
    /// `coex_schm_bt_a2dp_paused_wifi_connecting`.
    BtA2dpPausedWifiConnecting,
    /// `coex_schm_bt_a2dp_paused_wifi_scan`.
    BtA2dpPausedWifiScan,
    /// `coex_schm_bt_a2dp_wifi_conn`.
    BtA2dpWifiConn,
    /// `coex_schm_bt_a2dp_wifi_connecting`.
    BtA2dpWifiConnecting,
    /// `coex_schm_bt_a2dp_wifi_scan`.
    BtA2dpWifiScan,
    /// `coex_schm_bt_conn_wifi_conn`.
    BtConnWifiConn,
    /// `coex_schm_bt_conn_wifi_connecting`.
    BtConnWifiConnecting,
    /// `coex_schm_bt_conn_wifi_scan`.
    BtConnWifiScan,
    /// `coex_schm_bt_default_wifi_conn`.
    BtDefaultWifiConn,
    /// `coex_schm_bt_default_wifi_connecting`.
    BtDefaultWifiConnecting,
    /// `coex_schm_bt_default_wifi_scan`.
    BtDefaultWifiScan,
    /// `coex_schm_bt_idle_wifi_conn`.
    BtIdleWifiConn,
    /// `coex_schm_bt_idle_wifi_connecting`.
    BtIdleWifiConnecting,
    /// `coex_schm_bt_idle_wifi_scan`.
    BtIdleWifiScan,
    /// `coex_schm_bt_inq_wifi_conn`.
    BtInqWifiConn,
    /// `coex_schm_bt_inq_wifi_connecting`.
    BtInqWifiConnecting,
    /// `coex_schm_bt_inq_wifi_scan`.
    BtInqWifiScan,
    /// `coex_schm_bt_page_wifi_conn`.
    BtPageWifiConn,
    /// `coex_schm_bt_page_wifi_connecting`.
    BtPageWifiConnecting,
    /// `coex_schm_bt_page_wifi_scan`.
    BtPageWifiScan,
    /// `coex_schm_bt_piscan_wifi_conn`.
    BtPiscanWifiConn,
    /// `coex_schm_bt_piscan_wifi_connecting`.
    BtPiscanWifiConnecting,
    /// `coex_schm_bt_piscan_wifi_scan`.
    BtPiscanWifiScan,
    /// `coex_schm_bt_sniff_sco_wifi_conn`.
    BtSniffScoWifiConn,
    /// `coex_schm_bt_sniff_sco_wifi_connecting`.
    BtSniffScoWifiConnecting,
    /// `coex_schm_bt_sniff_sco_wifi_scan`.
    BtSniffScoWifiScan,
    /// `coex_schm_external_coex_wifi_connecting`.
    ExternalCoexWifiConnecting,
    /// `coex_schm_external_coex_wifi_default`.
    ExternalCoexWifiDefault,
    /// `coex_schm_external_coex_wifi_default_rxonly`.
    ExternalCoexWifiDefaultRxonly,
    /// `coex_schm_external_coex_wifi_scan`.
    ExternalCoexWifiScan,
}

impl CoexSchemeId {
    /// Every recovered scheme, in vendor symbol order.
    pub const ALL: [Self; 107] = [
        Self::AllDefault,
        Self::BleDefaultBtA2dpWifiConn,
        Self::BleDefaultBtA2dpWifiConnecting,
        Self::BleDefaultBtA2dpWifiDefault,
        Self::BleDefaultBtA2dpWifiScan,
        Self::BleDefaultBtDefaultWifiConn,
        Self::BleDefaultBtDefaultWifiConnecting,
        Self::BleDefaultBtDefaultWifiScan,
        Self::BleDefaultBtIdleWifiConn,
        Self::BleDefaultBtIdleWifiConnecting,
        Self::BleDefaultBtIdleWifiDefault,
        Self::BleDefaultBtIdleWifiScan,
        Self::BleIdleBtIdleWifiDefault,
        Self::BleMeshConfigBtA2dpPausedWifiConn,
        Self::BleMeshConfigBtA2dpPausedWifiConnecting,
        Self::BleMeshConfigBtA2dpPausedWifiScan,
        Self::BleMeshConfigBtA2dpWifiConn,
        Self::BleMeshConfigBtA2dpWifiConnecting,
        Self::BleMeshConfigBtA2dpWifiScan,
        Self::BleMeshConfigBtConnWifiConn,
        Self::BleMeshConfigBtConnWifiConnecting,
        Self::BleMeshConfigBtConnWifiScan,
        Self::BleMeshConfigBtDefaultWifiConn,
        Self::BleMeshConfigBtDefaultWifiConnecting,
        Self::BleMeshConfigBtDefaultWifiScan,
        Self::BleMeshConfigBtPiscanWifiConn,
        Self::BleMeshConfigBtPiscanWifiConnecting,
        Self::BleMeshConfigBtPiscanWifiScan,
        Self::BleMeshConfigBtSniffScoWifiConn,
        Self::BleMeshConfigBtSniffScoWifiConnecting,
        Self::BleMeshConfigBtSniffScoWifiScan,
        Self::BleMeshConfigWifiConn,
        Self::BleMeshConfigWifiConnecting,
        Self::BleMeshConfigWifiScan,
        Self::BleMeshStandbyBtA2dpPausedWifiConn,
        Self::BleMeshStandbyBtA2dpPausedWifiConnecting,
        Self::BleMeshStandbyBtA2dpPausedWifiScan,
        Self::BleMeshStandbyBtA2dpWifiConn,
        Self::BleMeshStandbyBtA2dpWifiConnecting,
        Self::BleMeshStandbyBtA2dpWifiScan,
        Self::BleMeshStandbyBtConnWifiConn,
        Self::BleMeshStandbyBtConnWifiConnecting,
        Self::BleMeshStandbyBtConnWifiScan,
        Self::BleMeshStandbyBtDefaultWifiConn,
        Self::BleMeshStandbyBtDefaultWifiConnecting,
        Self::BleMeshStandbyBtDefaultWifiScan,
        Self::BleMeshStandbyBtPiscanWifiConn,
        Self::BleMeshStandbyBtPiscanWifiConnecting,
        Self::BleMeshStandbyBtPiscanWifiScan,
        Self::BleMeshStandbyBtSniffScoWifiConn,
        Self::BleMeshStandbyBtSniffScoWifiConnecting,
        Self::BleMeshStandbyBtSniffScoWifiScan,
        Self::BleMeshStandbyWifiConn,
        Self::BleMeshStandbyWifiConnecting,
        Self::BleMeshStandbyWifiScan,
        Self::BleMeshTrafficBtA2dpPausedWifiConn,
        Self::BleMeshTrafficBtA2dpPausedWifiConnecting,
        Self::BleMeshTrafficBtA2dpPausedWifiScan,
        Self::BleMeshTrafficBtA2dpWifiConn,
        Self::BleMeshTrafficBtA2dpWifiConnecting,
        Self::BleMeshTrafficBtA2dpWifiScan,
        Self::BleMeshTrafficBtConnWifiConn,
        Self::BleMeshTrafficBtConnWifiConnecting,
        Self::BleMeshTrafficBtConnWifiScan,
        Self::BleMeshTrafficBtDefaultWifiConn,
        Self::BleMeshTrafficBtDefaultWifiConnecting,
        Self::BleMeshTrafficBtDefaultWifiScan,
        Self::BleMeshTrafficBtPiscanWifiConn,
        Self::BleMeshTrafficBtPiscanWifiConnecting,
        Self::BleMeshTrafficBtPiscanWifiScan,
        Self::BleMeshTrafficBtSniffScoWifiConn,
        Self::BleMeshTrafficBtSniffScoWifiConnecting,
        Self::BleMeshTrafficBtSniffScoWifiScan,
        Self::BleMeshTrafficWifiConn,
        Self::BleMeshTrafficWifiConnecting,
        Self::BleMeshTrafficWifiScan,
        Self::BtA2dpPausedWifiConn,
        Self::BtA2dpPausedWifiConnecting,
        Self::BtA2dpPausedWifiScan,
        Self::BtA2dpWifiConn,
        Self::BtA2dpWifiConnecting,
        Self::BtA2dpWifiScan,
        Self::BtConnWifiConn,
        Self::BtConnWifiConnecting,
        Self::BtConnWifiScan,
        Self::BtDefaultWifiConn,
        Self::BtDefaultWifiConnecting,
        Self::BtDefaultWifiScan,
        Self::BtIdleWifiConn,
        Self::BtIdleWifiConnecting,
        Self::BtIdleWifiScan,
        Self::BtInqWifiConn,
        Self::BtInqWifiConnecting,
        Self::BtInqWifiScan,
        Self::BtPageWifiConn,
        Self::BtPageWifiConnecting,
        Self::BtPageWifiScan,
        Self::BtPiscanWifiConn,
        Self::BtPiscanWifiConnecting,
        Self::BtPiscanWifiScan,
        Self::BtSniffScoWifiConn,
        Self::BtSniffScoWifiConnecting,
        Self::BtSniffScoWifiScan,
        Self::ExternalCoexWifiConnecting,
        Self::ExternalCoexWifiDefault,
        Self::ExternalCoexWifiDefaultRxonly,
        Self::ExternalCoexWifiScan,
    ];

    /// The vendor symbol suffix after `coex_schm_`.
    pub const fn name(self) -> &'static str {
        match self {
            Self::AllDefault => "all_default",
            Self::BleDefaultBtA2dpWifiConn => "ble_default_bt_a2dp_wifi_conn",
            Self::BleDefaultBtA2dpWifiConnecting => "ble_default_bt_a2dp_wifi_connecting",
            Self::BleDefaultBtA2dpWifiDefault => "ble_default_bt_a2dp_wifi_default",
            Self::BleDefaultBtA2dpWifiScan => "ble_default_bt_a2dp_wifi_scan",
            Self::BleDefaultBtDefaultWifiConn => "ble_default_bt_default_wifi_conn",
            Self::BleDefaultBtDefaultWifiConnecting => "ble_default_bt_default_wifi_connecting",
            Self::BleDefaultBtDefaultWifiScan => "ble_default_bt_default_wifi_scan",
            Self::BleDefaultBtIdleWifiConn => "ble_default_bt_idle_wifi_conn",
            Self::BleDefaultBtIdleWifiConnecting => "ble_default_bt_idle_wifi_connecting",
            Self::BleDefaultBtIdleWifiDefault => "ble_default_bt_idle_wifi_default",
            Self::BleDefaultBtIdleWifiScan => "ble_default_bt_idle_wifi_scan",
            Self::BleIdleBtIdleWifiDefault => "ble_idle_bt_idle_wifi_default",
            Self::BleMeshConfigBtA2dpPausedWifiConn => "ble_mesh_config_bt_a2dp_paused_wifi_conn",
            Self::BleMeshConfigBtA2dpPausedWifiConnecting => {
                "ble_mesh_config_bt_a2dp_paused_wifi_connecting"
            }
            Self::BleMeshConfigBtA2dpPausedWifiScan => "ble_mesh_config_bt_a2dp_paused_wifi_scan",
            Self::BleMeshConfigBtA2dpWifiConn => "ble_mesh_config_bt_a2dp_wifi_conn",
            Self::BleMeshConfigBtA2dpWifiConnecting => "ble_mesh_config_bt_a2dp_wifi_connecting",
            Self::BleMeshConfigBtA2dpWifiScan => "ble_mesh_config_bt_a2dp_wifi_scan",
            Self::BleMeshConfigBtConnWifiConn => "ble_mesh_config_bt_conn_wifi_conn",
            Self::BleMeshConfigBtConnWifiConnecting => "ble_mesh_config_bt_conn_wifi_connecting",
            Self::BleMeshConfigBtConnWifiScan => "ble_mesh_config_bt_conn_wifi_scan",
            Self::BleMeshConfigBtDefaultWifiConn => "ble_mesh_config_bt_default_wifi_conn",
            Self::BleMeshConfigBtDefaultWifiConnecting => {
                "ble_mesh_config_bt_default_wifi_connecting"
            }
            Self::BleMeshConfigBtDefaultWifiScan => "ble_mesh_config_bt_default_wifi_scan",
            Self::BleMeshConfigBtPiscanWifiConn => "ble_mesh_config_bt_piscan_wifi_conn",
            Self::BleMeshConfigBtPiscanWifiConnecting => {
                "ble_mesh_config_bt_piscan_wifi_connecting"
            }
            Self::BleMeshConfigBtPiscanWifiScan => "ble_mesh_config_bt_piscan_wifi_scan",
            Self::BleMeshConfigBtSniffScoWifiConn => "ble_mesh_config_bt_sniff_sco_wifi_conn",
            Self::BleMeshConfigBtSniffScoWifiConnecting => {
                "ble_mesh_config_bt_sniff_sco_wifi_connecting"
            }
            Self::BleMeshConfigBtSniffScoWifiScan => "ble_mesh_config_bt_sniff_sco_wifi_scan",
            Self::BleMeshConfigWifiConn => "ble_mesh_config_wifi_conn",
            Self::BleMeshConfigWifiConnecting => "ble_mesh_config_wifi_connecting",
            Self::BleMeshConfigWifiScan => "ble_mesh_config_wifi_scan",
            Self::BleMeshStandbyBtA2dpPausedWifiConn => "ble_mesh_standby_bt_a2dp_paused_wifi_conn",
            Self::BleMeshStandbyBtA2dpPausedWifiConnecting => {
                "ble_mesh_standby_bt_a2dp_paused_wifi_connecting"
            }
            Self::BleMeshStandbyBtA2dpPausedWifiScan => "ble_mesh_standby_bt_a2dp_paused_wifi_scan",
            Self::BleMeshStandbyBtA2dpWifiConn => "ble_mesh_standby_bt_a2dp_wifi_conn",
            Self::BleMeshStandbyBtA2dpWifiConnecting => "ble_mesh_standby_bt_a2dp_wifi_connecting",
            Self::BleMeshStandbyBtA2dpWifiScan => "ble_mesh_standby_bt_a2dp_wifi_scan",
            Self::BleMeshStandbyBtConnWifiConn => "ble_mesh_standby_bt_conn_wifi_conn",
            Self::BleMeshStandbyBtConnWifiConnecting => "ble_mesh_standby_bt_conn_wifi_connecting",
            Self::BleMeshStandbyBtConnWifiScan => "ble_mesh_standby_bt_conn_wifi_scan",
            Self::BleMeshStandbyBtDefaultWifiConn => "ble_mesh_standby_bt_default_wifi_conn",
            Self::BleMeshStandbyBtDefaultWifiConnecting => {
                "ble_mesh_standby_bt_default_wifi_connecting"
            }
            Self::BleMeshStandbyBtDefaultWifiScan => "ble_mesh_standby_bt_default_wifi_scan",
            Self::BleMeshStandbyBtPiscanWifiConn => "ble_mesh_standby_bt_piscan_wifi_conn",
            Self::BleMeshStandbyBtPiscanWifiConnecting => {
                "ble_mesh_standby_bt_piscan_wifi_connecting"
            }
            Self::BleMeshStandbyBtPiscanWifiScan => "ble_mesh_standby_bt_piscan_wifi_scan",
            Self::BleMeshStandbyBtSniffScoWifiConn => "ble_mesh_standby_bt_sniff_sco_wifi_conn",
            Self::BleMeshStandbyBtSniffScoWifiConnecting => {
                "ble_mesh_standby_bt_sniff_sco_wifi_connecting"
            }
            Self::BleMeshStandbyBtSniffScoWifiScan => "ble_mesh_standby_bt_sniff_sco_wifi_scan",
            Self::BleMeshStandbyWifiConn => "ble_mesh_standby_wifi_conn",
            Self::BleMeshStandbyWifiConnecting => "ble_mesh_standby_wifi_connecting",
            Self::BleMeshStandbyWifiScan => "ble_mesh_standby_wifi_scan",
            Self::BleMeshTrafficBtA2dpPausedWifiConn => "ble_mesh_traffic_bt_a2dp_paused_wifi_conn",
            Self::BleMeshTrafficBtA2dpPausedWifiConnecting => {
                "ble_mesh_traffic_bt_a2dp_paused_wifi_connecting"
            }
            Self::BleMeshTrafficBtA2dpPausedWifiScan => "ble_mesh_traffic_bt_a2dp_paused_wifi_scan",
            Self::BleMeshTrafficBtA2dpWifiConn => "ble_mesh_traffic_bt_a2dp_wifi_conn",
            Self::BleMeshTrafficBtA2dpWifiConnecting => "ble_mesh_traffic_bt_a2dp_wifi_connecting",
            Self::BleMeshTrafficBtA2dpWifiScan => "ble_mesh_traffic_bt_a2dp_wifi_scan",
            Self::BleMeshTrafficBtConnWifiConn => "ble_mesh_traffic_bt_conn_wifi_conn",
            Self::BleMeshTrafficBtConnWifiConnecting => "ble_mesh_traffic_bt_conn_wifi_connecting",
            Self::BleMeshTrafficBtConnWifiScan => "ble_mesh_traffic_bt_conn_wifi_scan",
            Self::BleMeshTrafficBtDefaultWifiConn => "ble_mesh_traffic_bt_default_wifi_conn",
            Self::BleMeshTrafficBtDefaultWifiConnecting => {
                "ble_mesh_traffic_bt_default_wifi_connecting"
            }
            Self::BleMeshTrafficBtDefaultWifiScan => "ble_mesh_traffic_bt_default_wifi_scan",
            Self::BleMeshTrafficBtPiscanWifiConn => "ble_mesh_traffic_bt_piscan_wifi_conn",
            Self::BleMeshTrafficBtPiscanWifiConnecting => {
                "ble_mesh_traffic_bt_piscan_wifi_connecting"
            }
            Self::BleMeshTrafficBtPiscanWifiScan => "ble_mesh_traffic_bt_piscan_wifi_scan",
            Self::BleMeshTrafficBtSniffScoWifiConn => "ble_mesh_traffic_bt_sniff_sco_wifi_conn",
            Self::BleMeshTrafficBtSniffScoWifiConnecting => {
                "ble_mesh_traffic_bt_sniff_sco_wifi_connecting"
            }
            Self::BleMeshTrafficBtSniffScoWifiScan => "ble_mesh_traffic_bt_sniff_sco_wifi_scan",
            Self::BleMeshTrafficWifiConn => "ble_mesh_traffic_wifi_conn",
            Self::BleMeshTrafficWifiConnecting => "ble_mesh_traffic_wifi_connecting",
            Self::BleMeshTrafficWifiScan => "ble_mesh_traffic_wifi_scan",
            Self::BtA2dpPausedWifiConn => "bt_a2dp_paused_wifi_conn",
            Self::BtA2dpPausedWifiConnecting => "bt_a2dp_paused_wifi_connecting",
            Self::BtA2dpPausedWifiScan => "bt_a2dp_paused_wifi_scan",
            Self::BtA2dpWifiConn => "bt_a2dp_wifi_conn",
            Self::BtA2dpWifiConnecting => "bt_a2dp_wifi_connecting",
            Self::BtA2dpWifiScan => "bt_a2dp_wifi_scan",
            Self::BtConnWifiConn => "bt_conn_wifi_conn",
            Self::BtConnWifiConnecting => "bt_conn_wifi_connecting",
            Self::BtConnWifiScan => "bt_conn_wifi_scan",
            Self::BtDefaultWifiConn => "bt_default_wifi_conn",
            Self::BtDefaultWifiConnecting => "bt_default_wifi_connecting",
            Self::BtDefaultWifiScan => "bt_default_wifi_scan",
            Self::BtIdleWifiConn => "bt_idle_wifi_conn",
            Self::BtIdleWifiConnecting => "bt_idle_wifi_connecting",
            Self::BtIdleWifiScan => "bt_idle_wifi_scan",
            Self::BtInqWifiConn => "bt_inq_wifi_conn",
            Self::BtInqWifiConnecting => "bt_inq_wifi_connecting",
            Self::BtInqWifiScan => "bt_inq_wifi_scan",
            Self::BtPageWifiConn => "bt_page_wifi_conn",
            Self::BtPageWifiConnecting => "bt_page_wifi_connecting",
            Self::BtPageWifiScan => "bt_page_wifi_scan",
            Self::BtPiscanWifiConn => "bt_piscan_wifi_conn",
            Self::BtPiscanWifiConnecting => "bt_piscan_wifi_connecting",
            Self::BtPiscanWifiScan => "bt_piscan_wifi_scan",
            Self::BtSniffScoWifiConn => "bt_sniff_sco_wifi_conn",
            Self::BtSniffScoWifiConnecting => "bt_sniff_sco_wifi_connecting",
            Self::BtSniffScoWifiScan => "bt_sniff_sco_wifi_scan",
            Self::ExternalCoexWifiConnecting => "external_coex_wifi_connecting",
            Self::ExternalCoexWifiDefault => "external_coex_wifi_default",
            Self::ExternalCoexWifiDefaultRxonly => "external_coex_wifi_default_rxonly",
            Self::ExternalCoexWifiScan => "external_coex_wifi_scan",
        }
    }

    /// The recovered period and phases.
    pub const fn scheme(self) -> &'static CoexScheme {
        match self {
            Self::AllDefault => &ALL_DEFAULT,
            Self::BleDefaultBtA2dpWifiConn => &BLE_DEFAULT_BT_A2DP_WIFI_CONN,
            Self::BleDefaultBtA2dpWifiConnecting => &BLE_DEFAULT_BT_A2DP_WIFI_CONNECTING,
            Self::BleDefaultBtA2dpWifiDefault => &BLE_DEFAULT_BT_A2DP_WIFI_DEFAULT,
            Self::BleDefaultBtA2dpWifiScan => &BLE_DEFAULT_BT_A2DP_WIFI_SCAN,
            Self::BleDefaultBtDefaultWifiConn => &BLE_DEFAULT_BT_DEFAULT_WIFI_CONN,
            Self::BleDefaultBtDefaultWifiConnecting => &BLE_DEFAULT_BT_DEFAULT_WIFI_CONNECTING,
            Self::BleDefaultBtDefaultWifiScan => &BLE_DEFAULT_BT_DEFAULT_WIFI_SCAN,
            Self::BleDefaultBtIdleWifiConn => &BLE_DEFAULT_BT_IDLE_WIFI_CONN,
            Self::BleDefaultBtIdleWifiConnecting => &BLE_DEFAULT_BT_IDLE_WIFI_CONNECTING,
            Self::BleDefaultBtIdleWifiDefault => &BLE_DEFAULT_BT_IDLE_WIFI_DEFAULT,
            Self::BleDefaultBtIdleWifiScan => &BLE_DEFAULT_BT_IDLE_WIFI_SCAN,
            Self::BleIdleBtIdleWifiDefault => &BLE_IDLE_BT_IDLE_WIFI_DEFAULT,
            Self::BleMeshConfigBtA2dpPausedWifiConn => &BLE_MESH_CONFIG_BT_A2DP_PAUSED_WIFI_CONN,
            Self::BleMeshConfigBtA2dpPausedWifiConnecting => {
                &BLE_MESH_CONFIG_BT_A2DP_PAUSED_WIFI_CONNECTING
            }
            Self::BleMeshConfigBtA2dpPausedWifiScan => &BLE_MESH_CONFIG_BT_A2DP_PAUSED_WIFI_SCAN,
            Self::BleMeshConfigBtA2dpWifiConn => &BLE_MESH_CONFIG_BT_A2DP_WIFI_CONN,
            Self::BleMeshConfigBtA2dpWifiConnecting => &BLE_MESH_CONFIG_BT_A2DP_WIFI_CONNECTING,
            Self::BleMeshConfigBtA2dpWifiScan => &BLE_MESH_CONFIG_BT_A2DP_WIFI_SCAN,
            Self::BleMeshConfigBtConnWifiConn => &BLE_MESH_CONFIG_BT_CONN_WIFI_CONN,
            Self::BleMeshConfigBtConnWifiConnecting => &BLE_MESH_CONFIG_BT_CONN_WIFI_CONNECTING,
            Self::BleMeshConfigBtConnWifiScan => &BLE_MESH_CONFIG_BT_CONN_WIFI_SCAN,
            Self::BleMeshConfigBtDefaultWifiConn => &BLE_MESH_CONFIG_BT_DEFAULT_WIFI_CONN,
            Self::BleMeshConfigBtDefaultWifiConnecting => {
                &BLE_MESH_CONFIG_BT_DEFAULT_WIFI_CONNECTING
            }
            Self::BleMeshConfigBtDefaultWifiScan => &BLE_MESH_CONFIG_BT_DEFAULT_WIFI_SCAN,
            Self::BleMeshConfigBtPiscanWifiConn => &BLE_MESH_CONFIG_BT_PISCAN_WIFI_CONN,
            Self::BleMeshConfigBtPiscanWifiConnecting => &BLE_MESH_CONFIG_BT_PISCAN_WIFI_CONNECTING,
            Self::BleMeshConfigBtPiscanWifiScan => &BLE_MESH_CONFIG_BT_PISCAN_WIFI_SCAN,
            Self::BleMeshConfigBtSniffScoWifiConn => &BLE_MESH_CONFIG_BT_SNIFF_SCO_WIFI_CONN,
            Self::BleMeshConfigBtSniffScoWifiConnecting => {
                &BLE_MESH_CONFIG_BT_SNIFF_SCO_WIFI_CONNECTING
            }
            Self::BleMeshConfigBtSniffScoWifiScan => &BLE_MESH_CONFIG_BT_SNIFF_SCO_WIFI_SCAN,
            Self::BleMeshConfigWifiConn => &BLE_MESH_CONFIG_WIFI_CONN,
            Self::BleMeshConfigWifiConnecting => &BLE_MESH_CONFIG_WIFI_CONNECTING,
            Self::BleMeshConfigWifiScan => &BLE_MESH_CONFIG_WIFI_SCAN,
            Self::BleMeshStandbyBtA2dpPausedWifiConn => &BLE_MESH_STANDBY_BT_A2DP_PAUSED_WIFI_CONN,
            Self::BleMeshStandbyBtA2dpPausedWifiConnecting => {
                &BLE_MESH_STANDBY_BT_A2DP_PAUSED_WIFI_CONNECTING
            }
            Self::BleMeshStandbyBtA2dpPausedWifiScan => &BLE_MESH_STANDBY_BT_A2DP_PAUSED_WIFI_SCAN,
            Self::BleMeshStandbyBtA2dpWifiConn => &BLE_MESH_STANDBY_BT_A2DP_WIFI_CONN,
            Self::BleMeshStandbyBtA2dpWifiConnecting => &BLE_MESH_STANDBY_BT_A2DP_WIFI_CONNECTING,
            Self::BleMeshStandbyBtA2dpWifiScan => &BLE_MESH_STANDBY_BT_A2DP_WIFI_SCAN,
            Self::BleMeshStandbyBtConnWifiConn => &BLE_MESH_STANDBY_BT_CONN_WIFI_CONN,
            Self::BleMeshStandbyBtConnWifiConnecting => &BLE_MESH_STANDBY_BT_CONN_WIFI_CONNECTING,
            Self::BleMeshStandbyBtConnWifiScan => &BLE_MESH_STANDBY_BT_CONN_WIFI_SCAN,
            Self::BleMeshStandbyBtDefaultWifiConn => &BLE_MESH_STANDBY_BT_DEFAULT_WIFI_CONN,
            Self::BleMeshStandbyBtDefaultWifiConnecting => {
                &BLE_MESH_STANDBY_BT_DEFAULT_WIFI_CONNECTING
            }
            Self::BleMeshStandbyBtDefaultWifiScan => &BLE_MESH_STANDBY_BT_DEFAULT_WIFI_SCAN,
            Self::BleMeshStandbyBtPiscanWifiConn => &BLE_MESH_STANDBY_BT_PISCAN_WIFI_CONN,
            Self::BleMeshStandbyBtPiscanWifiConnecting => {
                &BLE_MESH_STANDBY_BT_PISCAN_WIFI_CONNECTING
            }
            Self::BleMeshStandbyBtPiscanWifiScan => &BLE_MESH_STANDBY_BT_PISCAN_WIFI_SCAN,
            Self::BleMeshStandbyBtSniffScoWifiConn => &BLE_MESH_STANDBY_BT_SNIFF_SCO_WIFI_CONN,
            Self::BleMeshStandbyBtSniffScoWifiConnecting => {
                &BLE_MESH_STANDBY_BT_SNIFF_SCO_WIFI_CONNECTING
            }
            Self::BleMeshStandbyBtSniffScoWifiScan => &BLE_MESH_STANDBY_BT_SNIFF_SCO_WIFI_SCAN,
            Self::BleMeshStandbyWifiConn => &BLE_MESH_STANDBY_WIFI_CONN,
            Self::BleMeshStandbyWifiConnecting => &BLE_MESH_STANDBY_WIFI_CONNECTING,
            Self::BleMeshStandbyWifiScan => &BLE_MESH_STANDBY_WIFI_SCAN,
            Self::BleMeshTrafficBtA2dpPausedWifiConn => &BLE_MESH_TRAFFIC_BT_A2DP_PAUSED_WIFI_CONN,
            Self::BleMeshTrafficBtA2dpPausedWifiConnecting => {
                &BLE_MESH_TRAFFIC_BT_A2DP_PAUSED_WIFI_CONNECTING
            }
            Self::BleMeshTrafficBtA2dpPausedWifiScan => &BLE_MESH_TRAFFIC_BT_A2DP_PAUSED_WIFI_SCAN,
            Self::BleMeshTrafficBtA2dpWifiConn => &BLE_MESH_TRAFFIC_BT_A2DP_WIFI_CONN,
            Self::BleMeshTrafficBtA2dpWifiConnecting => &BLE_MESH_TRAFFIC_BT_A2DP_WIFI_CONNECTING,
            Self::BleMeshTrafficBtA2dpWifiScan => &BLE_MESH_TRAFFIC_BT_A2DP_WIFI_SCAN,
            Self::BleMeshTrafficBtConnWifiConn => &BLE_MESH_TRAFFIC_BT_CONN_WIFI_CONN,
            Self::BleMeshTrafficBtConnWifiConnecting => &BLE_MESH_TRAFFIC_BT_CONN_WIFI_CONNECTING,
            Self::BleMeshTrafficBtConnWifiScan => &BLE_MESH_TRAFFIC_BT_CONN_WIFI_SCAN,
            Self::BleMeshTrafficBtDefaultWifiConn => &BLE_MESH_TRAFFIC_BT_DEFAULT_WIFI_CONN,
            Self::BleMeshTrafficBtDefaultWifiConnecting => {
                &BLE_MESH_TRAFFIC_BT_DEFAULT_WIFI_CONNECTING
            }
            Self::BleMeshTrafficBtDefaultWifiScan => &BLE_MESH_TRAFFIC_BT_DEFAULT_WIFI_SCAN,
            Self::BleMeshTrafficBtPiscanWifiConn => &BLE_MESH_TRAFFIC_BT_PISCAN_WIFI_CONN,
            Self::BleMeshTrafficBtPiscanWifiConnecting => {
                &BLE_MESH_TRAFFIC_BT_PISCAN_WIFI_CONNECTING
            }
            Self::BleMeshTrafficBtPiscanWifiScan => &BLE_MESH_TRAFFIC_BT_PISCAN_WIFI_SCAN,
            Self::BleMeshTrafficBtSniffScoWifiConn => &BLE_MESH_TRAFFIC_BT_SNIFF_SCO_WIFI_CONN,
            Self::BleMeshTrafficBtSniffScoWifiConnecting => {
                &BLE_MESH_TRAFFIC_BT_SNIFF_SCO_WIFI_CONNECTING
            }
            Self::BleMeshTrafficBtSniffScoWifiScan => &BLE_MESH_TRAFFIC_BT_SNIFF_SCO_WIFI_SCAN,
            Self::BleMeshTrafficWifiConn => &BLE_MESH_TRAFFIC_WIFI_CONN,
            Self::BleMeshTrafficWifiConnecting => &BLE_MESH_TRAFFIC_WIFI_CONNECTING,
            Self::BleMeshTrafficWifiScan => &BLE_MESH_TRAFFIC_WIFI_SCAN,
            Self::BtA2dpPausedWifiConn => &BT_A2DP_PAUSED_WIFI_CONN,
            Self::BtA2dpPausedWifiConnecting => &BT_A2DP_PAUSED_WIFI_CONNECTING,
            Self::BtA2dpPausedWifiScan => &BT_A2DP_PAUSED_WIFI_SCAN,
            Self::BtA2dpWifiConn => &BT_A2DP_WIFI_CONN,
            Self::BtA2dpWifiConnecting => &BT_A2DP_WIFI_CONNECTING,
            Self::BtA2dpWifiScan => &BT_A2DP_WIFI_SCAN,
            Self::BtConnWifiConn => &BT_CONN_WIFI_CONN,
            Self::BtConnWifiConnecting => &BT_CONN_WIFI_CONNECTING,
            Self::BtConnWifiScan => &BT_CONN_WIFI_SCAN,
            Self::BtDefaultWifiConn => &BT_DEFAULT_WIFI_CONN,
            Self::BtDefaultWifiConnecting => &BT_DEFAULT_WIFI_CONNECTING,
            Self::BtDefaultWifiScan => &BT_DEFAULT_WIFI_SCAN,
            Self::BtIdleWifiConn => &BT_IDLE_WIFI_CONN,
            Self::BtIdleWifiConnecting => &BT_IDLE_WIFI_CONNECTING,
            Self::BtIdleWifiScan => &BT_IDLE_WIFI_SCAN,
            Self::BtInqWifiConn => &BT_INQ_WIFI_CONN,
            Self::BtInqWifiConnecting => &BT_INQ_WIFI_CONNECTING,
            Self::BtInqWifiScan => &BT_INQ_WIFI_SCAN,
            Self::BtPageWifiConn => &BT_PAGE_WIFI_CONN,
            Self::BtPageWifiConnecting => &BT_PAGE_WIFI_CONNECTING,
            Self::BtPageWifiScan => &BT_PAGE_WIFI_SCAN,
            Self::BtPiscanWifiConn => &BT_PISCAN_WIFI_CONN,
            Self::BtPiscanWifiConnecting => &BT_PISCAN_WIFI_CONNECTING,
            Self::BtPiscanWifiScan => &BT_PISCAN_WIFI_SCAN,
            Self::BtSniffScoWifiConn => &BT_SNIFF_SCO_WIFI_CONN,
            Self::BtSniffScoWifiConnecting => &BT_SNIFF_SCO_WIFI_CONNECTING,
            Self::BtSniffScoWifiScan => &BT_SNIFF_SCO_WIFI_SCAN,
            Self::ExternalCoexWifiConnecting => &EXTERNAL_COEX_WIFI_CONNECTING,
            Self::ExternalCoexWifiDefault => &EXTERNAL_COEX_WIFI_DEFAULT,
            Self::ExternalCoexWifiDefaultRxonly => &EXTERNAL_COEX_WIFI_DEFAULT_RXONLY,
            Self::ExternalCoexWifiScan => &EXTERNAL_COEX_WIFI_SCAN,
        }
    }
}

const ALL_DEFAULT: CoexScheme = CoexScheme::new(
    1,
    &[
        CoexPhase::new(40, 0x03, [0x00, 0x00]),
        CoexPhase::new(60, 0x00, [0x00, 0x00]),
    ],
);
const BLE_DEFAULT_BT_A2DP_WIFI_CONN: CoexScheme = CoexScheme::new(
    1,
    &[
        CoexPhase::new(30, 0x03, [0x00, 0x00]),
        CoexPhase::new(70, 0x00, [0x00, 0x00]),
    ],
);
const BLE_DEFAULT_BT_A2DP_WIFI_CONNECTING: CoexScheme = CoexScheme::new(
    33,
    &[
        CoexPhase::new(30, 0x02, [0x00, 0x00]),
        CoexPhase::new(70, 0x00, [0x00, 0x00]),
    ],
);
const BLE_DEFAULT_BT_A2DP_WIFI_DEFAULT: CoexScheme = CoexScheme::new(
    1,
    &[
        CoexPhase::new(30, 0x03, [0x00, 0x00]),
        CoexPhase::new(70, 0x00, [0x00, 0x00]),
    ],
);
const BLE_DEFAULT_BT_A2DP_WIFI_SCAN: CoexScheme = CoexScheme::new(
    34,
    &[
        CoexPhase::new(30, 0x02, [0x00, 0x00]),
        CoexPhase::new(70, 0x00, [0x00, 0x00]),
    ],
);
const BLE_DEFAULT_BT_DEFAULT_WIFI_CONN: CoexScheme = CoexScheme::new(
    1,
    &[
        CoexPhase::new(40, 0x03, [0x00, 0x00]),
        CoexPhase::new(60, 0x00, [0x00, 0x00]),
    ],
);
const BLE_DEFAULT_BT_DEFAULT_WIFI_CONNECTING: CoexScheme = CoexScheme::new(
    25,
    &[
        CoexPhase::new(40, 0x02, [0x00, 0x00]),
        CoexPhase::new(60, 0x00, [0x00, 0x00]),
    ],
);
const BLE_DEFAULT_BT_DEFAULT_WIFI_SCAN: CoexScheme = CoexScheme::new(
    30,
    &[
        CoexPhase::new(40, 0x02, [0x00, 0x00]),
        CoexPhase::new(60, 0x00, [0x00, 0x00]),
    ],
);
const BLE_DEFAULT_BT_IDLE_WIFI_CONN: CoexScheme = CoexScheme::new(
    1,
    &[
        CoexPhase::new(50, 0x03, [0x00, 0x00]),
        CoexPhase::new(50, 0x00, [0x00, 0x00]),
    ],
);
const BLE_DEFAULT_BT_IDLE_WIFI_CONNECTING: CoexScheme = CoexScheme::new(
    20,
    &[
        CoexPhase::new(50, 0x02, [0x00, 0x00]),
        CoexPhase::new(50, 0x00, [0x00, 0x00]),
    ],
);
const BLE_DEFAULT_BT_IDLE_WIFI_DEFAULT: CoexScheme = CoexScheme::new(
    1,
    &[
        CoexPhase::new(50, 0x03, [0x00, 0x00]),
        CoexPhase::new(50, 0x00, [0x00, 0x00]),
    ],
);
const BLE_DEFAULT_BT_IDLE_WIFI_SCAN: CoexScheme = CoexScheme::new(
    24,
    &[
        CoexPhase::new(50, 0x02, [0x00, 0x00]),
        CoexPhase::new(50, 0x00, [0x00, 0x00]),
    ],
);
const BLE_IDLE_BT_IDLE_WIFI_DEFAULT: CoexScheme =
    CoexScheme::new(1, &[CoexPhase::new(100, 0x00, [0x00, 0x00])]);
const BLE_MESH_CONFIG_BT_A2DP_PAUSED_WIFI_CONN: CoexScheme = CoexScheme::new(
    1,
    &[
        CoexPhase::new(25, 0x03, [0x00, 0x00]),
        CoexPhase::new(20, 0x00, [0x20, 0x04]),
        CoexPhase::new(55, 0x00, [0x02, 0x40]),
    ],
);
const BLE_MESH_CONFIG_BT_A2DP_PAUSED_WIFI_CONNECTING: CoexScheme = CoexScheme::new(
    10,
    &[
        CoexPhase::new(25, 0x02, [0x00, 0x00]),
        CoexPhase::new(20, 0x00, [0x20, 0x04]),
        CoexPhase::new(55, 0x00, [0x02, 0x40]),
    ],
);
const BLE_MESH_CONFIG_BT_A2DP_PAUSED_WIFI_SCAN: CoexScheme = CoexScheme::new(
    12,
    &[
        CoexPhase::new(25, 0x02, [0x00, 0x00]),
        CoexPhase::new(20, 0x00, [0x20, 0x04]),
        CoexPhase::new(55, 0x00, [0x02, 0x40]),
    ],
);
const BLE_MESH_CONFIG_BT_A2DP_WIFI_CONN: CoexScheme = CoexScheme::new(
    1,
    &[
        CoexPhase::new(25, 0x03, [0x00, 0x00]),
        CoexPhase::new(20, 0x00, [0x20, 0x04]),
        CoexPhase::new(55, 0x00, [0x02, 0x40]),
    ],
);
const BLE_MESH_CONFIG_BT_A2DP_WIFI_CONNECTING: CoexScheme = CoexScheme::new(
    10,
    &[
        CoexPhase::new(25, 0x02, [0x00, 0x00]),
        CoexPhase::new(20, 0x00, [0x20, 0x04]),
        CoexPhase::new(55, 0x00, [0x02, 0x40]),
    ],
);
const BLE_MESH_CONFIG_BT_A2DP_WIFI_SCAN: CoexScheme = CoexScheme::new(
    12,
    &[
        CoexPhase::new(25, 0x02, [0x00, 0x00]),
        CoexPhase::new(20, 0x00, [0x20, 0x04]),
        CoexPhase::new(55, 0x00, [0x02, 0x40]),
    ],
);
const BLE_MESH_CONFIG_BT_CONN_WIFI_CONN: CoexScheme = CoexScheme::new(
    1,
    &[
        CoexPhase::new(25, 0x03, [0x00, 0x00]),
        CoexPhase::new(20, 0x00, [0x20, 0x04]),
        CoexPhase::new(55, 0x00, [0x02, 0x40]),
    ],
);
const BLE_MESH_CONFIG_BT_CONN_WIFI_CONNECTING: CoexScheme = CoexScheme::new(
    10,
    &[
        CoexPhase::new(25, 0x02, [0x00, 0x00]),
        CoexPhase::new(20, 0x00, [0x20, 0x04]),
        CoexPhase::new(55, 0x00, [0x02, 0x40]),
    ],
);
const BLE_MESH_CONFIG_BT_CONN_WIFI_SCAN: CoexScheme = CoexScheme::new(
    12,
    &[
        CoexPhase::new(25, 0x02, [0x00, 0x00]),
        CoexPhase::new(20, 0x00, [0x20, 0x04]),
        CoexPhase::new(55, 0x00, [0x02, 0x40]),
    ],
);
const BLE_MESH_CONFIG_BT_DEFAULT_WIFI_CONN: CoexScheme = CoexScheme::new(
    1,
    &[
        CoexPhase::new(25, 0x03, [0x00, 0x00]),
        CoexPhase::new(20, 0x00, [0x20, 0x04]),
        CoexPhase::new(55, 0x00, [0x02, 0x40]),
    ],
);
const BLE_MESH_CONFIG_BT_DEFAULT_WIFI_CONNECTING: CoexScheme = CoexScheme::new(
    10,
    &[
        CoexPhase::new(25, 0x02, [0x00, 0x00]),
        CoexPhase::new(20, 0x00, [0x20, 0x04]),
        CoexPhase::new(55, 0x00, [0x02, 0x40]),
    ],
);
const BLE_MESH_CONFIG_BT_DEFAULT_WIFI_SCAN: CoexScheme = CoexScheme::new(
    12,
    &[
        CoexPhase::new(25, 0x02, [0x00, 0x00]),
        CoexPhase::new(20, 0x00, [0x20, 0x04]),
        CoexPhase::new(55, 0x00, [0x02, 0x40]),
    ],
);
const BLE_MESH_CONFIG_BT_PISCAN_WIFI_CONN: CoexScheme = CoexScheme::new(
    1,
    &[
        CoexPhase::new(45, 0x03, [0x20, 0x02]),
        CoexPhase::new(55, 0x00, [0x02, 0x20]),
    ],
);
const BLE_MESH_CONFIG_BT_PISCAN_WIFI_CONNECTING: CoexScheme = CoexScheme::new(
    10,
    &[
        CoexPhase::new(45, 0x02, [0x20, 0x02]),
        CoexPhase::new(55, 0x00, [0x02, 0x20]),
    ],
);
const BLE_MESH_CONFIG_BT_PISCAN_WIFI_SCAN: CoexScheme = CoexScheme::new(
    12,
    &[
        CoexPhase::new(45, 0x02, [0x20, 0x02]),
        CoexPhase::new(55, 0x00, [0x02, 0x20]),
    ],
);
const BLE_MESH_CONFIG_BT_SNIFF_SCO_WIFI_CONN: CoexScheme = CoexScheme::new(
    1,
    &[
        CoexPhase::new(45, 0x03, [0x20, 0x00]),
        CoexPhase::new(55, 0x00, [0x02, 0x00]),
    ],
);
const BLE_MESH_CONFIG_BT_SNIFF_SCO_WIFI_CONNECTING: CoexScheme = CoexScheme::new(
    10,
    &[
        CoexPhase::new(45, 0x02, [0x20, 0x00]),
        CoexPhase::new(55, 0x00, [0x02, 0x00]),
    ],
);
const BLE_MESH_CONFIG_BT_SNIFF_SCO_WIFI_SCAN: CoexScheme = CoexScheme::new(
    12,
    &[
        CoexPhase::new(45, 0x02, [0x20, 0x00]),
        CoexPhase::new(55, 0x00, [0x02, 0x00]),
    ],
);
const BLE_MESH_CONFIG_WIFI_CONN: CoexScheme = CoexScheme::new(
    1,
    &[
        CoexPhase::new(45, 0x03, [0x00, 0x00]),
        CoexPhase::new(55, 0x00, [0x00, 0x00]),
    ],
);
const BLE_MESH_CONFIG_WIFI_CONNECTING: CoexScheme = CoexScheme::new(
    10,
    &[
        CoexPhase::new(45, 0x02, [0x00, 0x00]),
        CoexPhase::new(55, 0x00, [0x00, 0x00]),
    ],
);
const BLE_MESH_CONFIG_WIFI_SCAN: CoexScheme = CoexScheme::new(
    12,
    &[
        CoexPhase::new(40, 0x02, [0x00, 0x00]),
        CoexPhase::new(60, 0x00, [0x00, 0x00]),
    ],
);
const BLE_MESH_STANDBY_BT_A2DP_PAUSED_WIFI_CONN: CoexScheme = CoexScheme::new(
    2,
    &[
        CoexPhase::new(58, 0x03, [0x00, 0x00]),
        CoexPhase::new(30, 0x00, [0x20, 0x04]),
        CoexPhase::new(12, 0x00, [0x02, 0x40]),
    ],
);
const BLE_MESH_STANDBY_BT_A2DP_PAUSED_WIFI_CONNECTING: CoexScheme = CoexScheme::new(
    20,
    &[
        CoexPhase::new(45, 0x02, [0x00, 0x00]),
        CoexPhase::new(30, 0x00, [0x20, 0x04]),
        CoexPhase::new(25, 0x00, [0x02, 0x40]),
    ],
);
const BLE_MESH_STANDBY_BT_A2DP_PAUSED_WIFI_SCAN: CoexScheme = CoexScheme::new(
    30,
    &[
        CoexPhase::new(40, 0x02, [0x00, 0x00]),
        CoexPhase::new(30, 0x00, [0x20, 0x04]),
        CoexPhase::new(30, 0x00, [0x02, 0x40]),
    ],
);
const BLE_MESH_STANDBY_BT_A2DP_WIFI_CONN: CoexScheme = CoexScheme::new(
    2,
    &[
        CoexPhase::new(28, 0x03, [0x00, 0x00]),
        CoexPhase::new(60, 0x00, [0x20, 0x04]),
        CoexPhase::new(12, 0x00, [0x02, 0x40]),
    ],
);
const BLE_MESH_STANDBY_BT_A2DP_WIFI_CONNECTING: CoexScheme = CoexScheme::new(
    30,
    &[
        CoexPhase::new(30, 0x02, [0x00, 0x00]),
        CoexPhase::new(25, 0x00, [0x20, 0x04]),
        CoexPhase::new(10, 0x00, [0x02, 0x40]),
        CoexPhase::new(25, 0x00, [0x20, 0x04]),
        CoexPhase::new(10, 0x00, [0x02, 0x40]),
    ],
);
const BLE_MESH_STANDBY_BT_A2DP_WIFI_SCAN: CoexScheme = CoexScheme::new(
    25,
    &[
        CoexPhase::new(40, 0x02, [0x00, 0x00]),
        CoexPhase::new(25, 0x00, [0x20, 0x04]),
        CoexPhase::new(5, 0x00, [0x02, 0x40]),
        CoexPhase::new(25, 0x00, [0x20, 0x04]),
        CoexPhase::new(5, 0x00, [0x02, 0x40]),
    ],
);
const BLE_MESH_STANDBY_BT_CONN_WIFI_CONN: CoexScheme = CoexScheme::new(
    2,
    &[
        CoexPhase::new(63, 0x03, [0x00, 0x00]),
        CoexPhase::new(25, 0x00, [0x20, 0x04]),
        CoexPhase::new(12, 0x00, [0x02, 0x40]),
    ],
);
const BLE_MESH_STANDBY_BT_CONN_WIFI_CONNECTING: CoexScheme = CoexScheme::new(
    20,
    &[
        CoexPhase::new(45, 0x02, [0x00, 0x00]),
        CoexPhase::new(30, 0x00, [0x20, 0x04]),
        CoexPhase::new(25, 0x00, [0x02, 0x40]),
    ],
);
const BLE_MESH_STANDBY_BT_CONN_WIFI_SCAN: CoexScheme = CoexScheme::new(
    30,
    &[
        CoexPhase::new(40, 0x02, [0x00, 0x00]),
        CoexPhase::new(30, 0x00, [0x20, 0x04]),
        CoexPhase::new(30, 0x00, [0x02, 0x40]),
    ],
);
const BLE_MESH_STANDBY_BT_DEFAULT_WIFI_CONN: CoexScheme = CoexScheme::new(
    2,
    &[
        CoexPhase::new(38, 0x03, [0x00, 0x00]),
        CoexPhase::new(50, 0x00, [0x20, 0x04]),
        CoexPhase::new(12, 0x00, [0x02, 0x40]),
    ],
);
const BLE_MESH_STANDBY_BT_DEFAULT_WIFI_CONNECTING: CoexScheme = CoexScheme::new(
    30,
    &[
        CoexPhase::new(34, 0x02, [0x00, 0x00]),
        CoexPhase::new(23, 0x00, [0x20, 0x04]),
        CoexPhase::new(10, 0x00, [0x02, 0x40]),
        CoexPhase::new(23, 0x00, [0x20, 0x04]),
        CoexPhase::new(10, 0x00, [0x02, 0x40]),
    ],
);
const BLE_MESH_STANDBY_BT_DEFAULT_WIFI_SCAN: CoexScheme = CoexScheme::new(
    30,
    &[
        CoexPhase::new(40, 0x02, [0x00, 0x00]),
        CoexPhase::new(25, 0x00, [0x20, 0x04]),
        CoexPhase::new(5, 0x00, [0x02, 0x40]),
        CoexPhase::new(25, 0x00, [0x20, 0x04]),
        CoexPhase::new(5, 0x00, [0x02, 0x40]),
    ],
);
const BLE_MESH_STANDBY_BT_PISCAN_WIFI_CONN: CoexScheme = CoexScheme::new(
    2,
    &[
        CoexPhase::new(88, 0x03, [0x20, 0x20]),
        CoexPhase::new(12, 0x00, [0x02, 0x20]),
    ],
);
const BLE_MESH_STANDBY_BT_PISCAN_WIFI_CONNECTING: CoexScheme = CoexScheme::new(
    10,
    &[
        CoexPhase::new(70, 0x02, [0x20, 0x02]),
        CoexPhase::new(30, 0x00, [0x02, 0x20]),
    ],
);
const BLE_MESH_STANDBY_BT_PISCAN_WIFI_SCAN: CoexScheme = CoexScheme::new(
    17,
    &[
        CoexPhase::new(70, 0x02, [0x20, 0x02]),
        CoexPhase::new(30, 0x00, [0x02, 0x20]),
    ],
);
const BLE_MESH_STANDBY_BT_SNIFF_SCO_WIFI_CONN: CoexScheme = CoexScheme::new(
    2,
    &[
        CoexPhase::new(88, 0x03, [0x20, 0x00]),
        CoexPhase::new(12, 0x00, [0x02, 0x00]),
    ],
);
const BLE_MESH_STANDBY_BT_SNIFF_SCO_WIFI_CONNECTING: CoexScheme = CoexScheme::new(
    10,
    &[
        CoexPhase::new(70, 0x02, [0x20, 0x00]),
        CoexPhase::new(30, 0x00, [0x02, 0x00]),
    ],
);
const BLE_MESH_STANDBY_BT_SNIFF_SCO_WIFI_SCAN: CoexScheme = CoexScheme::new(
    20,
    &[
        CoexPhase::new(60, 0x02, [0x20, 0x00]),
        CoexPhase::new(40, 0x00, [0x02, 0x00]),
    ],
);
const BLE_MESH_STANDBY_WIFI_CONN: CoexScheme = CoexScheme::new(
    2,
    &[
        CoexPhase::new(80, 0x03, [0x00, 0x00]),
        CoexPhase::new(20, 0x00, [0x00, 0x00]),
    ],
);
const BLE_MESH_STANDBY_WIFI_CONNECTING: CoexScheme = CoexScheme::new(
    10,
    &[
        CoexPhase::new(70, 0x02, [0x00, 0x00]),
        CoexPhase::new(30, 0x00, [0x00, 0x00]),
    ],
);
const BLE_MESH_STANDBY_WIFI_SCAN: CoexScheme = CoexScheme::new(
    20,
    &[
        CoexPhase::new(60, 0x02, [0x00, 0x00]),
        CoexPhase::new(40, 0x00, [0x00, 0x00]),
    ],
);
const BLE_MESH_TRAFFIC_BT_A2DP_PAUSED_WIFI_CONN: CoexScheme = CoexScheme::new(
    1,
    &[
        CoexPhase::new(40, 0x03, [0x00, 0x00]),
        CoexPhase::new(30, 0x00, [0x20, 0x04]),
        CoexPhase::new(30, 0x00, [0x02, 0x40]),
    ],
);
const BLE_MESH_TRAFFIC_BT_A2DP_PAUSED_WIFI_CONNECTING: CoexScheme = CoexScheme::new(
    25,
    &[
        CoexPhase::new(40, 0x02, [0x00, 0x00]),
        CoexPhase::new(30, 0x00, [0x20, 0x04]),
        CoexPhase::new(30, 0x00, [0x02, 0x40]),
    ],
);
const BLE_MESH_TRAFFIC_BT_A2DP_PAUSED_WIFI_SCAN: CoexScheme = CoexScheme::new(
    30,
    &[
        CoexPhase::new(40, 0x02, [0x00, 0x00]),
        CoexPhase::new(30, 0x00, [0x20, 0x04]),
        CoexPhase::new(30, 0x00, [0x02, 0x40]),
    ],
);
const BLE_MESH_TRAFFIC_BT_A2DP_WIFI_CONN: CoexScheme = CoexScheme::new(
    1,
    &[
        CoexPhase::new(20, 0x03, [0x00, 0x00]),
        CoexPhase::new(50, 0x00, [0x20, 0x04]),
        CoexPhase::new(30, 0x00, [0x02, 0x40]),
    ],
);
const BLE_MESH_TRAFFIC_BT_A2DP_WIFI_CONNECTING: CoexScheme = CoexScheme::new(
    30,
    &[
        CoexPhase::new(30, 0x02, [0x00, 0x00]),
        CoexPhase::new(25, 0x00, [0x20, 0x04]),
        CoexPhase::new(10, 0x00, [0x02, 0x40]),
        CoexPhase::new(25, 0x00, [0x20, 0x04]),
        CoexPhase::new(10, 0x00, [0x02, 0x40]),
    ],
);
const BLE_MESH_TRAFFIC_BT_A2DP_WIFI_SCAN: CoexScheme = CoexScheme::new(
    34,
    &[
        CoexPhase::new(30, 0x02, [0x00, 0x00]),
        CoexPhase::new(25, 0x00, [0x20, 0x04]),
        CoexPhase::new(10, 0x00, [0x02, 0x40]),
        CoexPhase::new(25, 0x00, [0x20, 0x04]),
        CoexPhase::new(10, 0x00, [0x02, 0x40]),
    ],
);
const BLE_MESH_TRAFFIC_BT_CONN_WIFI_CONN: CoexScheme = CoexScheme::new(
    1,
    &[
        CoexPhase::new(40, 0x03, [0x00, 0x00]),
        CoexPhase::new(30, 0x00, [0x20, 0x04]),
        CoexPhase::new(30, 0x00, [0x02, 0x40]),
    ],
);
const BLE_MESH_TRAFFIC_BT_CONN_WIFI_CONNECTING: CoexScheme = CoexScheme::new(
    25,
    &[
        CoexPhase::new(40, 0x02, [0x00, 0x00]),
        CoexPhase::new(30, 0x00, [0x20, 0x04]),
        CoexPhase::new(30, 0x00, [0x02, 0x40]),
    ],
);
const BLE_MESH_TRAFFIC_BT_CONN_WIFI_SCAN: CoexScheme = CoexScheme::new(
    30,
    &[
        CoexPhase::new(40, 0x02, [0x00, 0x00]),
        CoexPhase::new(30, 0x00, [0x20, 0x04]),
        CoexPhase::new(30, 0x00, [0x02, 0x40]),
    ],
);
const BLE_MESH_TRAFFIC_BT_DEFAULT_WIFI_CONN: CoexScheme = CoexScheme::new(
    1,
    &[
        CoexPhase::new(40, 0x03, [0x00, 0x00]),
        CoexPhase::new(30, 0x00, [0x20, 0x04]),
        CoexPhase::new(30, 0x00, [0x02, 0x40]),
    ],
);
const BLE_MESH_TRAFFIC_BT_DEFAULT_WIFI_CONNECTING: CoexScheme = CoexScheme::new(
    30,
    &[
        CoexPhase::new(34, 0x02, [0x00, 0x00]),
        CoexPhase::new(23, 0x00, [0x20, 0x04]),
        CoexPhase::new(10, 0x00, [0x02, 0x40]),
        CoexPhase::new(23, 0x00, [0x20, 0x04]),
        CoexPhase::new(10, 0x00, [0x02, 0x40]),
    ],
);
const BLE_MESH_TRAFFIC_BT_DEFAULT_WIFI_SCAN: CoexScheme = CoexScheme::new(
    30,
    &[
        CoexPhase::new(40, 0x02, [0x00, 0x00]),
        CoexPhase::new(20, 0x00, [0x20, 0x04]),
        CoexPhase::new(10, 0x00, [0x02, 0x40]),
        CoexPhase::new(20, 0x00, [0x20, 0x04]),
        CoexPhase::new(10, 0x00, [0x02, 0x40]),
    ],
);
const BLE_MESH_TRAFFIC_BT_PISCAN_WIFI_CONN: CoexScheme = CoexScheme::new(
    1,
    &[
        CoexPhase::new(70, 0x03, [0x20, 0x02]),
        CoexPhase::new(30, 0x00, [0x02, 0x20]),
    ],
);
const BLE_MESH_TRAFFIC_BT_PISCAN_WIFI_CONNECTING: CoexScheme = CoexScheme::new(
    10,
    &[
        CoexPhase::new(70, 0x02, [0x20, 0x02]),
        CoexPhase::new(30, 0x00, [0x02, 0x20]),
    ],
);
const BLE_MESH_TRAFFIC_BT_PISCAN_WIFI_SCAN: CoexScheme = CoexScheme::new(
    20,
    &[
        CoexPhase::new(60, 0x02, [0x20, 0x02]),
        CoexPhase::new(40, 0x00, [0x02, 0x20]),
    ],
);
const BLE_MESH_TRAFFIC_BT_SNIFF_SCO_WIFI_CONN: CoexScheme = CoexScheme::new(
    1,
    &[
        CoexPhase::new(70, 0x03, [0x20, 0x00]),
        CoexPhase::new(30, 0x00, [0x02, 0x00]),
    ],
);
const BLE_MESH_TRAFFIC_BT_SNIFF_SCO_WIFI_CONNECTING: CoexScheme = CoexScheme::new(
    10,
    &[
        CoexPhase::new(70, 0x02, [0x20, 0x00]),
        CoexPhase::new(30, 0x00, [0x02, 0x00]),
    ],
);
const BLE_MESH_TRAFFIC_BT_SNIFF_SCO_WIFI_SCAN: CoexScheme = CoexScheme::new(
    24,
    &[
        CoexPhase::new(50, 0x02, [0x20, 0x00]),
        CoexPhase::new(50, 0x00, [0x02, 0x00]),
    ],
);
const BLE_MESH_TRAFFIC_WIFI_CONN: CoexScheme = CoexScheme::new(
    1,
    &[
        CoexPhase::new(60, 0x03, [0x00, 0x00]),
        CoexPhase::new(40, 0x00, [0x00, 0x00]),
    ],
);
const BLE_MESH_TRAFFIC_WIFI_CONNECTING: CoexScheme = CoexScheme::new(
    10,
    &[
        CoexPhase::new(70, 0x02, [0x00, 0x00]),
        CoexPhase::new(30, 0x00, [0x00, 0x00]),
    ],
);
const BLE_MESH_TRAFFIC_WIFI_SCAN: CoexScheme = CoexScheme::new(
    24,
    &[
        CoexPhase::new(50, 0x02, [0x00, 0x00]),
        CoexPhase::new(50, 0x00, [0x00, 0x00]),
    ],
);
const BT_A2DP_PAUSED_WIFI_CONN: CoexScheme = CoexScheme::new(
    1,
    &[
        CoexPhase::new(70, 0x03, [0x00, 0x00]),
        CoexPhase::new(30, 0x00, [0x00, 0x04]),
    ],
);
const BT_A2DP_PAUSED_WIFI_CONNECTING: CoexScheme = CoexScheme::new(
    10,
    &[
        CoexPhase::new(70, 0x02, [0x00, 0x00]),
        CoexPhase::new(30, 0x00, [0x00, 0x04]),
    ],
);
const BT_A2DP_PAUSED_WIFI_SCAN: CoexScheme = CoexScheme::new(
    17,
    &[
        CoexPhase::new(70, 0x02, [0x00, 0x00]),
        CoexPhase::new(30, 0x00, [0x00, 0x04]),
    ],
);
const BT_A2DP_WIFI_CONN: CoexScheme = CoexScheme::new(
    1,
    &[
        CoexPhase::new(40, 0x03, [0x00, 0x00]),
        CoexPhase::new(60, 0x00, [0x00, 0x04]),
    ],
);
const BT_A2DP_WIFI_CONNECTING: CoexScheme = CoexScheme::new(
    25,
    &[
        CoexPhase::new(40, 0x02, [0x00, 0x00]),
        CoexPhase::new(60, 0x00, [0x00, 0x00]),
    ],
);
const BT_A2DP_WIFI_SCAN: CoexScheme = CoexScheme::new(
    20,
    &[
        CoexPhase::new(50, 0x02, [0x00, 0x00]),
        CoexPhase::new(50, 0x00, [0x00, 0x00]),
    ],
);
const BT_CONN_WIFI_CONN: CoexScheme = CoexScheme::new(
    1,
    &[
        CoexPhase::new(50, 0x03, [0x00, 0x00]),
        CoexPhase::new(50, 0x00, [0x00, 0x04]),
    ],
);
const BT_CONN_WIFI_CONNECTING: CoexScheme = CoexScheme::new(
    10,
    &[
        CoexPhase::new(70, 0x02, [0x00, 0x00]),
        CoexPhase::new(30, 0x00, [0x00, 0x00]),
    ],
);
const BT_CONN_WIFI_SCAN: CoexScheme = CoexScheme::new(
    17,
    &[
        CoexPhase::new(70, 0x02, [0x00, 0x00]),
        CoexPhase::new(30, 0x00, [0x00, 0x00]),
    ],
);
const BT_DEFAULT_WIFI_CONN: CoexScheme = CoexScheme::new(
    1,
    &[
        CoexPhase::new(50, 0x03, [0x00, 0x00]),
        CoexPhase::new(50, 0x00, [0x00, 0x00]),
    ],
);
const BT_DEFAULT_WIFI_CONNECTING: CoexScheme = CoexScheme::new(
    20,
    &[
        CoexPhase::new(50, 0x02, [0x00, 0x00]),
        CoexPhase::new(50, 0x00, [0x00, 0x00]),
    ],
);
const BT_DEFAULT_WIFI_SCAN: CoexScheme = CoexScheme::new(
    24,
    &[
        CoexPhase::new(50, 0x02, [0x00, 0x00]),
        CoexPhase::new(50, 0x00, [0x00, 0x00]),
    ],
);
const BT_IDLE_WIFI_CONN: CoexScheme =
    CoexScheme::new(1, &[CoexPhase::new(100, 0x02, [0x00, 0x00])]);
const BT_IDLE_WIFI_CONNECTING: CoexScheme =
    CoexScheme::new(10, &[CoexPhase::new(100, 0x02, [0x00, 0x00])]);
const BT_IDLE_WIFI_SCAN: CoexScheme =
    CoexScheme::new(12, &[CoexPhase::new(100, 0x02, [0x00, 0x00])]);
const BT_INQ_WIFI_CONN: CoexScheme = CoexScheme::new(
    1,
    &[
        CoexPhase::new(40, 0x02, [0x00, 0x00]),
        CoexPhase::new(60, 0x00, [0x00, 0x00]),
    ],
);
const BT_INQ_WIFI_CONNECTING: CoexScheme = CoexScheme::new(
    20,
    &[
        CoexPhase::new(50, 0x02, [0x00, 0x00]),
        CoexPhase::new(50, 0x00, [0x00, 0x00]),
    ],
);
const BT_INQ_WIFI_SCAN: CoexScheme = CoexScheme::new(
    30,
    &[
        CoexPhase::new(40, 0x02, [0x00, 0x00]),
        CoexPhase::new(60, 0x00, [0x00, 0x00]),
    ],
);
const BT_PAGE_WIFI_CONN: CoexScheme = CoexScheme::new(
    1,
    &[
        CoexPhase::new(30, 0x02, [0x00, 0x00]),
        CoexPhase::new(70, 0x00, [0x00, 0x00]),
    ],
);
const BT_PAGE_WIFI_CONNECTING: CoexScheme = CoexScheme::new(
    20,
    &[
        CoexPhase::new(50, 0x02, [0x00, 0x00]),
        CoexPhase::new(50, 0x00, [0x00, 0x00]),
    ],
);
const BT_PAGE_WIFI_SCAN: CoexScheme = CoexScheme::new(
    40,
    &[
        CoexPhase::new(30, 0x02, [0x00, 0x00]),
        CoexPhase::new(70, 0x00, [0x00, 0x00]),
    ],
);
const BT_PISCAN_WIFI_CONN: CoexScheme =
    CoexScheme::new(1, &[CoexPhase::new(100, 0x02, [0x00, 0x00])]);
const BT_PISCAN_WIFI_CONNECTING: CoexScheme =
    CoexScheme::new(10, &[CoexPhase::new(100, 0x02, [0x00, 0x00])]);
const BT_PISCAN_WIFI_SCAN: CoexScheme =
    CoexScheme::new(12, &[CoexPhase::new(100, 0x02, [0x00, 0x00])]);
const BT_SNIFF_SCO_WIFI_CONN: CoexScheme =
    CoexScheme::new(1, &[CoexPhase::new(100, 0x02, [0x00, 0x00])]);
const BT_SNIFF_SCO_WIFI_CONNECTING: CoexScheme =
    CoexScheme::new(10, &[CoexPhase::new(100, 0x02, [0x00, 0x00])]);
const BT_SNIFF_SCO_WIFI_SCAN: CoexScheme =
    CoexScheme::new(12, &[CoexPhase::new(100, 0x02, [0x00, 0x00])]);
const EXTERNAL_COEX_WIFI_CONNECTING: CoexScheme =
    CoexScheme::new(10, &[CoexPhase::new(100, 0x02, [0x00, 0x00])]);
const EXTERNAL_COEX_WIFI_DEFAULT: CoexScheme = CoexScheme::new(
    1,
    &[
        CoexPhase::new(50, 0x03, [0x00, 0x00]),
        CoexPhase::new(50, 0x00, [0x00, 0x00]),
    ],
);
const EXTERNAL_COEX_WIFI_DEFAULT_RXONLY: CoexScheme = CoexScheme::new(
    1,
    &[
        CoexPhase::new(50, 0x02, [0x00, 0x00]),
        CoexPhase::new(50, 0x04, [0x00, 0x00]),
    ],
);
const EXTERNAL_COEX_WIFI_SCAN: CoexScheme =
    CoexScheme::new(12, &[CoexPhase::new(100, 0x02, [0x00, 0x00])]);
