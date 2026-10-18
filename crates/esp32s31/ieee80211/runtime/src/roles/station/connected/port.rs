//! Production owner graph for one connected ESP32-S31 station epoch.
//!
//! Board/HIL code supplies already allocated storage, a network RX sink and
//! executor task placement. This module owns the driver relationships between
//! the associated peer, RX dispatcher/protocol, control-TX handoff,
//! ordinary/A-MPDU TX, BlockAck control and the final [`SingleRoleServices`].

use crate::datapath::PinnedTxFrame;

use embassy_sync::blocking_mutex::raw::RawMutex;

use oer_esp32s31_ieee80211::ordinary_tx::{WifiTxEntropy, WifiTxPowerProfile};

use oer_esp32s31_ieee80211_mac::{
    capabilities::ESP32S31_MAC_SERVICE_CAPABILITIES,
    init::{StaEspNowRxPolicyHardware, configure_sta_esp_now_receive_policy},
    rate::control::{StaRateControlTx, StaTxRatePolicy},
    rx::{
        RxIngressConfig,
        ampdu::{RxBlockAckSessions, RxBlockAckSessionsError},
    },
    tx::{
        HeDcmRate, HeEdcaTxopLimit, HeMcs, HeTriggerBasedTxConfig, HtChannelWidth,
        HtDuplicateCertificationRequest, HtDuplicateTxLinkCapabilities, HtDuplicateTxSelection,
        HtGuardInterval, HtMcs, LegacyRate, TxPhyRate, TxSlotState, ampdu::TX_BLOCK_ACK_MAX_WINDOW,
        select_esp32s31_ht_duplicate_tx,
    },
};
use oer_espressif_ieee80211_policy::rate_control::StaRateControlAssociation;

use oer_esp32s31_ieee80211_sta::{
    connected_control::JoinBeacon,
    connected_rx::{ConnectedRxConfig, StaCcmpRxReplayRxEndpoint},
    control_tx::ControlTransmitter,
    modem_sleep::PmBeacon,
    peer::{ConnectedStaPeer, StaConnectedLink},
    single_mpdu_tx::{ConnectedTxHandoff, SingleMpduTxConfig},
};

use {
    oer_ieee80211_mac::he::HeDcmConstellation, oer_ieee80211_mac::qos::WmmAccessCategory,
    oer_ieee80211_mac::station::StaTxSequenceCounters,
    oer_ieee80211_mac::station::association::PhyMode,
    oer_ieee80211_mac::station_power_save::StaAssociationId,
};

use oer_ieee80211_softmac::{
    EspNowRxEpoch, MacServiceCapabilities, WifiPlan,
    interface::{BoundVirtualInterface, VifRole},
};

use oer_ieee80211_mac::block_ack::{
    TxBlockAckOriginator, TxBlockAckOriginatorConfig, TxBlockAckOriginatorError,
    TxBlockAckOriginatorPolicy,
};
use oer_ieee80211_sta::{
    link_monitor::{StaBeaconLossConfig, StaBeaconLossConfigError},
    request::StationPowerMode,
};

#[cfg(any(feature = "diagnostics", test))]
use crate::diagnostics::{
    aggregate_tx::AggregateTxObserver,
    rx_pipeline::{RxPipelineObserver, RxReorderAgreementObserver},
};

use crate::roles::station::power::StationPowerBinding;

use crate::{
    datapath::{
        irq::EmbassyMacIrqRuntime,
        rx::{
            reorder::{
                RX_REORDER_BACKING_SLOT_COUNT, RxReorderCommandReceiver, RxReorderCommandSender,
                RxReorderFrameStorage,
            },
            staging::StagedRxReceiver,
        },
        services::SingleRoleServices,
        tx::resources::AggregateTxResources,
    },
    roles::{
        concurrent::StaApRxBlockAck,
        station::{
            control::ConnectedControl,
            control_mailbox::ConnectedControlReceiver,
            rx_protocol::{
                ConnectedReceiveProtocol, ConnectedReceiveStorage, ConnectedRxProcessor,
                ConnectedRxProtocolSink,
            },
            tx::{AggregateTxConfig, ConnectedTx, StationTxBlockAckStatusSink},
        },
    },
};
/// Stateless namespace for preparing and composing a connected owner graph.
pub struct ConnectedStaPort;

mod composition;
mod plan;
mod resources;

pub use plan::{
    ConnectedStaBlockAckPolicy, ConnectedStaCcmpReplayError, ConnectedStaCcmpReplayFailure,
    ConnectedStaConfig, ConnectedStaConfigError, ConnectedStaEspNowRxError, ConnectedStaPlan,
    ConnectedStaPrepareFailure, ConnectedStaRateConfig, ConnectedStaRxPolicy, ConnectedStaTxPolicy,
};

pub use resources::{
    ConnectedStaCompositionFailure, ConnectedStaControlResources, ConnectedStaDriverParts,
    ConnectedStaDrivers, ConnectedStaNetworkTxDomain, ConnectedStaReport,
    ConnectedStaRxProcessorResources, ConnectedStaRxProtocolResources,
    ConnectedStaTxHandoffFailure, ConnectedStaTxResources,
};

#[cfg(test)]
mod tests;
