use super::*;
use oer_ieee80211_mac::block_ack::TX_BLOCK_ACK_MAX_TIDS;

/// Runtime-selected rate policy independent of HIL environment variables.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ConnectedStaRateConfig {
    pub high_throughput_enabled: bool,
    pub fallback_legacy_rate: LegacyRate,
    pub fallback_ht_mcs: HtMcs,
    pub fallback_ht_guard_interval: HtGuardInterval,
    pub ht_mcs_override: Option<HtMcs>,
    pub ht_guard_interval_override: Option<HtGuardInterval>,
    pub he_mcs_override: Option<HeMcs>,
    pub he_guard_interval_and_ltf_override:
        Option<oer_esp32s31_ieee80211_mac::rx::HeGuardIntervalAndLtf>,
    pub he_dcm_override: Option<HeDcmRate>,
}

/// Complete value policy for one connected driver epoch.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ConnectedStaConfig {
    pub tx: ConnectedStaTxPolicy,
    pub block_ack: ConnectedStaBlockAckPolicy,
    pub receive: ConnectedStaRxPolicy,
    pub power: StationPowerMode,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ConnectedStaTxPolicy {
    pub rate: ConnectedStaRateConfig,
    pub completion_timeout: oer_time::Duration,
    pub aggregate_frame_limit: u8,
    pub aggregate_he_txop_limit: HeEdcaTxopLimit,
    /// Optional recovered queue/MPLEN/BSR preparation for AP Trigger frames.
    /// A runtime handoff additionally requires the config's explicit software
    /// window; the final HE-TB PHY publication remains fail-closed.
    /// `None` leaves HE-SU behavior unchanged.
    pub he_trigger_based: Option<HeTriggerBasedTxConfig>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ConnectedStaBlockAckPolicy {
    pub tx_block_ack_window: u16,
    pub tx_block_ack_negotiation_timeout: oer_time::Duration,
    pub tx_block_ack_negotiation_attempt_limit: u8,
    pub tid0_amsdu: bool,
    pub rx_block_ack_maximum_window: u16,
    pub request_initial_tx_block_ack: bool,
}

impl ConnectedStaBlockAckPolicy {
    /// The station's TX Block Ack originator: the Espressif TIDs and Dialog
    /// Tokens, its window bounded by the S31 TX queue geometry.
    pub fn tx_block_ack_originator(
        &self,
    ) -> Result<TxBlockAckOriginator<TX_BLOCK_ACK_MAX_TIDS>, ConnectedStaConfigError> {
        if self.tx_block_ack_window > TX_BLOCK_ACK_MAX_WINDOW {
            return Err(ConnectedStaConfigError::TxBlockAckWindow(
                self.tx_block_ack_window,
            ));
        }
        TxBlockAckOriginator::new(
            TxBlockAckOriginatorPolicy {
                tids: &oer_espressif_ieee80211_policy::block_ack::STA_TX_BLOCK_ACK_TIDS,
                first_dialog_token: oer_espressif_ieee80211_policy::block_ack::FIRST_DIALOG_TOKEN,
                next_dialog_token: oer_espressif_ieee80211_policy::block_ack::next_dialog_token,
            },
            TxBlockAckOriginatorConfig {
                window: self.tx_block_ack_window,
                negotiation_timeout: self.tx_block_ack_negotiation_timeout,
                amsdu_tids: u8::from(self.tid0_amsdu),
            },
        )
        .map_err(ConnectedStaConfigError::TxBlockAck)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ConnectedStaRxPolicy {
    pub ingress: RxIngressConfig,
    /// Consecutive misses the hardware beacon monitor counts while the
    /// modem sleeps.
    pub beacon_miss_limit: u8,
    /// How long the link may go without a beacon before the station probes
    /// the access point.
    pub beacon_timeout: oer_time::Duration,
}

/// Configuration failure detected before any connected owner moves.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConnectedStaConfigError {
    SecurityModeMismatch {
        installed: oer_ieee80211_mac::security::LinkProtection,
        material: oer_ieee80211_mac::security::LinkProtection,
    },
    MissingStationInterface,
    InterfaceRole(VifRole),
    InterfaceAddress {
        interface: [u8; 6],
        station: [u8; 6],
    },
    InvalidAssociationId(u16),
    AggregateFrameLimit {
        limit: u8,
        capacity: usize,
    },
    RxBlockAckWindowExceedsStorage {
        window: u16,
        capacity: usize,
    },
    ZeroTxBlockAckNegotiationAttemptLimit,
    HeTriggerTidMismatch {
        configured: u8,
        data_tid: u8,
    },
    PeerDoesNotSupportQos,
    TxBlockAck(TxBlockAckOriginatorError),
    /// The TX Block Ack window exceeds the S31 TX queue geometry.
    TxBlockAckWindow(u16),
    RxBlockAck(RxBlockAckSessionsError),
    BeaconLoss(StaBeaconLossConfigError),
}

/// Complete owner return when connected policy validation fails.
#[derive(Debug)]
pub struct ConnectedStaPrepareFailure {
    pub error: ConnectedStaConfigError,
    pub peer: ConnectedStaPeer,
}

/// Validated driver plan derived from the exact associated peer.
#[derive(Debug, Eq, PartialEq)]
pub struct ConnectedStaPlan {
    pub(super) interface: BoundVirtualInterface,
    pub(super) link: StaConnectedLink,
    pub(super) config: ConnectedStaConfig,
    pub(super) data_tx_rate: TxPhyRate,
    pub(super) data_rate_policy: StaTxRatePolicy,
    pub(super) control_schedule: oer_esp32s31_ieee80211_mac::rate::schedule::RateScheduleRef,
    pub(super) aggregate_tx_rate: TxPhyRate,
    pub(super) ht_duplicate_tx_selection: HtDuplicateTxSelection,
    pub(super) aggregate_rate_policy: StaTxRatePolicy,
    pub(super) rate_control: Option<StaRateControlAssociation>,
    pub(super) beacon_loss: StaBeaconLossConfig,
    pub(super) esp_now_rx: Option<EspNowRxEpoch>,
    pub(super) ccmp_rx_replay: Option<StaCcmpRxReplayRxEndpoint>,
    pub(super) security: oer_ieee80211_mac::security::LinkProtection,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConnectedStaCcmpReplayError {
    RequiresWpa2,
    AlreadyInstalled,
}

/// Plan rejection which returns the exact shared RX endpoint unchanged.
#[derive(Debug, Eq, PartialEq)]
pub struct ConnectedStaCcmpReplayFailure {
    pub error: ConnectedStaCcmpReplayError,
    replay: StaCcmpRxReplayRxEndpoint,
}

impl ConnectedStaCcmpReplayFailure {
    pub fn into_parts(self) -> (ConnectedStaCcmpReplayError, StaCcmpRxReplayRxEndpoint) {
        (self.error, self.replay)
    }
}

/// Why a portable ESP-NOW RX epoch cannot join this connected STA plan.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConnectedStaEspNowRxError {
    StationBinding {
        connected: BoundVirtualInterface,
        esp_now: BoundVirtualInterface,
    },
}

impl ConnectedStaPlan {
    pub const fn link_protection(&self) -> oer_ieee80211_mac::security::LinkProtection {
        self.security
    }

    pub const fn interface(&self) -> BoundVirtualInterface {
        self.interface
    }

    pub const fn link(&self) -> StaConnectedLink {
        self.link
    }

    pub const fn data_tx_rate(&self) -> TxPhyRate {
        self.data_tx_rate
    }

    pub const fn aggregate_tx_rate(&self) -> TxPhyRate {
        self.aggregate_tx_rate
    }

    /// Independent fixed-request outcome; never part of ordinary rate ranking.
    pub const fn ht_duplicate_tx_selection(&self) -> HtDuplicateTxSelection {
        self.ht_duplicate_tx_selection
    }

    pub const fn beacon_loss(&self) -> StaBeaconLossConfig {
        self.beacon_loss
    }

    /// Install peer/duplicate ownership created while this exact STA VIF and
    /// home channel were active.
    pub fn enable_esp_now_rx(
        &mut self,
        epoch: EspNowRxEpoch,
    ) -> Result<(), ConnectedStaEspNowRxError> {
        if epoch.config().station() != self.interface {
            return Err(ConnectedStaEspNowRxError::StationBinding {
                connected: self.interface,
                esp_now: epoch.config().station(),
            });
        }
        self.esp_now_rx = Some(epoch);
        Ok(())
    }

    pub const fn esp_now_rx_enabled(&self) -> bool {
        self.esp_now_rx.is_some()
    }

    pub const fn esp_now_rx_epoch(&self) -> Option<&EspNowRxEpoch> {
        self.esp_now_rx.as_ref()
    }

    /// Remove a not-yet-composed epoch without changing connected Wi-Fi
    /// policy. A running dispatcher revokes its copy during RX shutdown.
    pub fn disable_esp_now_rx(&mut self) -> Option<EspNowRxEpoch> {
        self.esp_now_rx.take()
    }

    /// Move the association's unique PTK/GTK replay epoch into the connected
    /// RX plan before the dispatcher is composed.
    pub fn enable_ccmp_rx_replay(
        &mut self,
        replay: StaCcmpRxReplayRxEndpoint,
    ) -> Result<(), ConnectedStaCcmpReplayFailure> {
        if self.security != oer_ieee80211_mac::security::LinkProtection::Ccmp {
            return Err(ConnectedStaCcmpReplayFailure {
                error: ConnectedStaCcmpReplayError::RequiresWpa2,
                replay,
            });
        }
        if self.ccmp_rx_replay.is_some() {
            return Err(ConnectedStaCcmpReplayFailure {
                error: ConnectedStaCcmpReplayError::AlreadyInstalled,
                replay,
            });
        }
        self.ccmp_rx_replay = Some(replay);
        Ok(())
    }

    pub const fn ccmp_rx_replay_enabled(&self) -> bool {
        self.ccmp_rx_replay.is_some()
    }

    pub(super) fn take_ccmp_rx_replay(&mut self) -> Option<StaCcmpRxReplayRxEndpoint> {
        self.ccmp_rx_replay.take()
    }

    pub const fn rx_config(&self) -> ConnectedRxConfig {
        ConnectedRxConfig {
            station_address: self.link.station_address,
            bssid: self.link.bssid,
            association_id: self.link.association_id,
            ingress: self.config.receive.ingress,
            security: self.security,
            peer_qos: self.link.peer_qos,
            management_protection: self.link.management_protection,
        }
    }

    pub const fn single_mpdu_tx_config(&self) -> SingleMpduTxConfig {
        SingleMpduTxConfig {
            station_address: self.link.station_address,
            bssid: self.link.bssid,
            ssid: self.link.ssid,
            peer_qos: self.link.peer_qos,
            management_protection: self.link.management_protection,
            access_category: WmmAccessCategory::BestEffort,
            control_schedule: self.control_schedule,
            publication_timeout: self.config.tx.completion_timeout,
        }
    }

    pub const fn aggregate_tx_config(&self) -> AggregateTxConfig {
        AggregateTxConfig {
            rate: self.aggregate_tx_rate,
            frame_limit: self.config.tx.aggregate_frame_limit,
            completion_timeout: self.config.tx.completion_timeout,
            he_txop_limit: self.config.tx.aggregate_he_txop_limit,
        }
    }
}
impl ConnectedStaPort {
    /// Return the portable service contract implemented by this production
    /// ESP32-S31 adapter. HMAC policy can inspect this value without importing
    /// PAC, DMA, interrupt or executor types.
    pub const fn capabilities() -> MacServiceCapabilities {
        ESP32S31_MAC_SERVICE_CAPABILITIES
    }

    /// Materialize the station interface selected by the application radio
    /// plan into one concrete connected ESP32-S31 owner graph.
    ///
    /// The plan has already been checked against complete radio/MAC
    /// capabilities. This boundary still fails closed if a caller passes a
    /// plan without a station and returns the associated peer unchanged.
    #[allow(clippy::result_large_err)]
    pub fn prepare_for_wifi_plan_with_storage<
        const AGGREGATE_SLOTS: usize,
        const RX_REORDER_SLOTS: usize,
    >(
        peer: ConnectedStaPeer,
        config: ConnectedStaConfig,
        wifi: WifiPlan,
    ) -> Result<ConnectedStaPlan, ConnectedStaPrepareFailure> {
        Self::prepare_for_wifi_plan_with_storage_and_ht_duplicate_certification::<
            AGGREGATE_SLOTS,
            RX_REORDER_SLOTS,
        >(peer, config, wifi, None)
    }

    /// Prepare a STA plan with an explicit MCS32 certification request.
    ///
    /// The result retains the request's typed rejection for telemetry while
    /// the ordinary MCS0..MCS7/HE schedule remains the publishable plan.
    #[allow(clippy::result_large_err)]
    pub fn prepare_for_wifi_plan_with_storage_and_ht_duplicate_certification<
        const AGGREGATE_SLOTS: usize,
        const RX_REORDER_SLOTS: usize,
    >(
        peer: ConnectedStaPeer,
        config: ConnectedStaConfig,
        wifi: WifiPlan,
        request: Option<HtDuplicateCertificationRequest>,
    ) -> Result<ConnectedStaPlan, ConnectedStaPrepareFailure> {
        let Some(interface) = wifi.station() else {
            return Err(ConnectedStaPrepareFailure {
                error: ConnectedStaConfigError::MissingStationInterface,
                peer,
            });
        };
        Self::prepare_for_interface_with_storage_and_ht_duplicate_certification::<
            AGGREGATE_SLOTS,
            RX_REORDER_SLOTS,
        >(peer, config, interface, request)
    }

    /// Prepare one explicitly identified STA VIF on a hardware channel
    /// context.
    #[allow(clippy::result_large_err)]
    pub fn prepare_for_interface_with_storage<
        const AGGREGATE_SLOTS: usize,
        const RX_REORDER_SLOTS: usize,
    >(
        peer: ConnectedStaPeer,
        config: ConnectedStaConfig,
        interface: BoundVirtualInterface,
    ) -> Result<ConnectedStaPlan, ConnectedStaPrepareFailure> {
        Self::prepare_for_interface_with_storage_and_ht_duplicate_certification::<
            AGGREGATE_SLOTS,
            RX_REORDER_SLOTS,
        >(peer, config, interface, None)
    }

    /// Prepare one STA VIF under the exact installed security contract.
    ///
    /// Legacy non-WMM peers remain valid for Open because its connected TX is
    /// deliberately non-QoS and owns no BlockAck/A-MPDU session. Existing
    /// planner entrypoints retain their WPA2 aggregate contract.
    #[allow(clippy::result_large_err)]
    pub fn prepare_for_interface_with_storage_and_security<
        const AGGREGATE_SLOTS: usize,
        const RX_REORDER_SLOTS: usize,
    >(
        peer: ConnectedStaPeer,
        config: ConnectedStaConfig,
        interface: BoundVirtualInterface,
        security: oer_ieee80211_mac::security::LinkProtection,
    ) -> Result<ConnectedStaPlan, ConnectedStaPrepareFailure> {
        Self::prepare_for_interface_with_storage_security_and_ht_duplicate_certification::<
            AGGREGATE_SLOTS,
            RX_REORDER_SLOTS,
        >(peer, config, interface, security, None)
    }

    /// Prepare one STA VIF and evaluate an independent MCS32 fixed request.
    #[allow(clippy::result_large_err)]
    pub fn prepare_for_interface_with_storage_and_ht_duplicate_certification<
        const AGGREGATE_SLOTS: usize,
        const RX_REORDER_SLOTS: usize,
    >(
        peer: ConnectedStaPeer,
        config: ConnectedStaConfig,
        interface: BoundVirtualInterface,
        request: Option<HtDuplicateCertificationRequest>,
    ) -> Result<ConnectedStaPlan, ConnectedStaPrepareFailure> {
        Self::prepare_for_interface_with_storage_security_and_ht_duplicate_certification::<
            AGGREGATE_SLOTS,
            RX_REORDER_SLOTS,
        >(
            peer,
            config,
            interface,
            oer_ieee80211_mac::security::LinkProtection::Ccmp,
            request,
        )
    }

    #[allow(clippy::result_large_err)]
    fn prepare_for_interface_with_storage_security_and_ht_duplicate_certification<
        const AGGREGATE_SLOTS: usize,
        const RX_REORDER_SLOTS: usize,
    >(
        peer: ConnectedStaPeer,
        mut config: ConnectedStaConfig,
        interface: BoundVirtualInterface,
        security: oer_ieee80211_mac::security::LinkProtection,
        request: Option<HtDuplicateCertificationRequest>,
    ) -> Result<ConnectedStaPlan, ConnectedStaPrepareFailure> {
        // Trigger-based publication assumes the QoS/BA-connected contract.
        // Open owns neither, so erase even a syntactically valid opt-in before
        // it can reach control or TX runtime state.
        if security == oer_ieee80211_mac::security::LinkProtection::Open {
            config.tx.he_trigger_based = None;
        }
        if interface.interface.role != VifRole::Station {
            return Err(ConnectedStaPrepareFailure {
                error: ConnectedStaConfigError::InterfaceRole(interface.interface.role),
                peer,
            });
        }
        if interface.interface.address != peer.link.station_address {
            return Err(ConnectedStaPrepareFailure {
                error: ConnectedStaConfigError::InterfaceAddress {
                    interface: interface.interface.address,
                    station: peer.link.station_address,
                },
                peer,
            });
        }
        if config.tx.aggregate_frame_limit == 0
            || usize::from(config.tx.aggregate_frame_limit) > AGGREGATE_SLOTS
        {
            return Err(ConnectedStaPrepareFailure {
                error: ConnectedStaConfigError::AggregateFrameLimit {
                    limit: config.tx.aggregate_frame_limit,
                    capacity: AGGREGATE_SLOTS,
                },
                peer,
            });
        }
        if usize::from(config.block_ack.rx_block_ack_maximum_window) > RX_REORDER_SLOTS {
            return Err(ConnectedStaPrepareFailure {
                error: ConnectedStaConfigError::RxBlockAckWindowExceedsStorage {
                    window: config.block_ack.rx_block_ack_maximum_window,
                    capacity: RX_REORDER_SLOTS,
                },
                peer,
            });
        }
        if config.block_ack.tx_block_ack_negotiation_attempt_limit == 0 {
            return Err(ConnectedStaPrepareFailure {
                error: ConnectedStaConfigError::ZeroTxBlockAckNegotiationAttemptLimit,
                peer,
            });
        }
        if let Some(trigger) = config.tx.he_trigger_based
            && trigger.tid().value() != 0
        {
            return Err(ConnectedStaPrepareFailure {
                error: ConnectedStaConfigError::HeTriggerTidMismatch {
                    configured: trigger.tid().value(),
                    data_tid: 0,
                },
                peer,
            });
        }
        if security == oer_ieee80211_mac::security::LinkProtection::Ccmp && !peer.link.peer_qos {
            return Err(ConnectedStaPrepareFailure {
                error: ConnectedStaConfigError::PeerDoesNotSupportQos,
                peer,
            });
        }
        if StaAssociationId::new(peer.link.association_id).is_none() {
            return Err(ConnectedStaPrepareFailure {
                error: ConnectedStaConfigError::InvalidAssociationId(peer.link.association_id),
                peer,
            });
        }
        if let Err(error) = config.block_ack.tx_block_ack_originator() {
            return Err(ConnectedStaPrepareFailure { error, peer });
        }
        if let Err(error) = RxBlockAckSessions::<1>::with_maximum_window(
            config.block_ack.rx_block_ack_maximum_window,
        ) {
            return Err(ConnectedStaPrepareFailure {
                error: ConnectedStaConfigError::RxBlockAck(error),
                peer,
            });
        }
        let beacon_loss = match StaBeaconLossConfig::new(
            peer.link.beacon_interval_tu,
            config.receive.beacon_miss_limit,
            config.receive.beacon_timeout,
        ) {
            Ok(beacon_loss) => beacon_loss,
            Err(error) => {
                return Err(ConnectedStaPrepareFailure {
                    error: ConnectedStaConfigError::BeaconLoss(error),
                    peer,
                });
            }
        };
        let ConnectedStaPeer { link, rate_control } = peer;
        let ht_duplicate_tx_selection = select_esp32s31_ht_duplicate_tx(
            request,
            HtDuplicateTxLinkCapabilities::new(
                match link.association_phy {
                    PhyMode::Legacy => None,
                    PhyMode::Ht20 | PhyMode::He20 => Some(HtChannelWidth::Mhz20),
                    PhyMode::Ht40 => Some(HtChannelWidth::Mhz40),
                },
                link.peer_supports_ht_duplicate_mcs32,
                link.peer_supports_ht_short_guard_interval,
            ),
        );
        let data_policy = sta_tx_rate_policy(link, config.tx.rate, false);
        let aggregate_policy = sta_tx_rate_policy(link, config.tx.rate, true);
        let aggregate_tx_rate = rate_control.ampdu_tx_rate(aggregate_policy);
        Ok(ConnectedStaPlan {
            interface,
            link,
            config,
            data_tx_rate: rate_control.tx_rate(data_policy),
            data_rate_policy: data_policy,
            control_schedule: rate_control.control_schedule(),
            aggregate_tx_rate,
            ht_duplicate_tx_selection,
            aggregate_rate_policy: aggregate_policy,
            rate_control: Some(rate_control),
            beacon_loss,
            esp_now_rx: None,
            ccmp_rx_replay: None,
            security,
        })
    }
}

const fn sta_tx_rate_policy(
    link: StaConnectedLink,
    config: ConnectedStaRateConfig,
    use_peer_capabilities: bool,
) -> StaTxRatePolicy {
    StaTxRatePolicy {
        association_phy: link.association_phy,
        high_throughput_enabled: config.high_throughput_enabled && link.peer_qos,
        fallback_legacy_rate: config.fallback_legacy_rate,
        fallback_ht_mcs: config.fallback_ht_mcs,
        fallback_ht_guard_interval: config.fallback_ht_guard_interval,
        ht_mcs_override: config.ht_mcs_override,
        ht_guard_interval_override: config.ht_guard_interval_override,
        he_mcs_override: config.he_mcs_override,
        he_guard_interval_and_ltf_override: config.he_guard_interval_and_ltf_override,
        he_dcm_override: config.he_dcm_override,
        he_800ns_gi_ltf: if use_peer_capabilities && link.peer_supports_one_ltf_800ns_gi {
            oer_esp32s31_ieee80211_mac::rx::HeGuardIntervalAndLtf::OneLtf800Ns
        } else {
            oer_esp32s31_ieee80211_mac::rx::HeGuardIntervalAndLtf::TwoLtf800Ns
        },
        peer_supports_ht_short_guard_interval: link.peer_supports_ht_short_guard_interval,
        peer_supports_ldpc: use_peer_capabilities && link.peer_supports_ldpc,
        peer_dcm_receive: if use_peer_capabilities {
            link.peer_dcm_receive
        } else {
            HeDcmConstellation::NotSupported
        },
    }
}
