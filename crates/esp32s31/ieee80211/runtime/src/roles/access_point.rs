#![expect(
    clippy::too_many_arguments,
    reason = "AP ingress and lifecycle boundaries expose independent borrowed owners without dynamic erasure"
)]

//! Embassy-owned AP MAC and network handoff service.
//!
//! The service handles beacons, management frames, WPA2 EAPOL and authorized
//! Ethernet traffic through one bounded RX/TX owner.

use core::{convert::Infallible, future::Future};

use crate::datapath::{
    MaterializedTxFrame, PinnedTxFrame, SelectedBurstMaterializer, SoftwareTxFrame,
};

use embassy_futures::yield_now;

use embassy_sync::blocking_mutex::raw::RawMutex;

use oer_time::{Instant, Timer};

use oer_memory::StableDmaBacking;

use oer_esp32s31_ieee80211::{
    ampdu_tx::HtAmpduTxRolePolicy,
    ordinary_tx::{WifiTxEntropy, WifiTxPowerProfile, WifiTxResources},
    tx::{WifiTxProgress, WifiTxWake},
};

use oer_esp32s31_ieee80211_ap::protocol::{
    AP_MAX_CLIENTS, AccessPointServiceStatus, ApAssociationIdentity, ApBufferedGroupRelease,
    ApBufferedUnicastRelease, ApDownlinkDisposition, ApPeerClose, ApPeerPhase, ApPeerPowerState,
    ApPowerSaveAction, ApWpa2RetryProgress,
};

#[cfg(any(feature = "diagnostics", test))]
use oer_esp32s31_ieee80211_ap::transaction::ApMacObservation;
#[cfg(any(feature = "diagnostics", test))]
use oer_esp32s31_ieee80211_mac::{
    rx::{HtDuplicateRxClassification, HtSignal, RxDescriptorSnapshot},
    tx::{HtChannelWidth, HtMcs, HtRate},
};
use oer_network_interface::{FrameLengthError, LinkState, RxEnqueueError};

use oer_esp32s31_ieee80211_ap::{
    ampdu::{ApAggregateAdmission, ApAmpduError, ApAmpduProgress},
    engine::{ApEngine, ApWpa2Outcome},
    rx::{
        ApRxAdmission, ApRxAdmissionRequest, ApRxConfig, ApRxDispatch, ApRxDispatcher, ApRxError,
        ApRxEvent, ApRxSink,
    },
    transaction::{ApMac, ApMacError, ApMacParked, ApPeerDisconnectStage, ApTxCompletionAction},
};

use oer_esp32s31_ieee80211_mac::{
    MacInterface,
    init::MAC_COLD_RX_INTERRUPT_MASK,
    irq::MacInterruptRoute,
    rx::{
        RxDma, RxIngressConfig,
        ampdu::{
            RX_BLOCK_ACK_MAX_WINDOW, RxBlockAckActivation, RxBlockAckRequest,
            RxBlockAckSessionsError, write_declined_addba_response,
        },
        hardware::{RxBlockAckHardware, S31RxBlockAckAgreementError},
        view_normalized_rx_frame,
    },
    tx::TxHardware,
};

use oer_ieee80211_mac::{
    ap::{
        ApManagementRequest, ApPowerSaveObservation,
        observe_ap_null_data_power_save_for_access_point, observe_ap_power_save_for_access_point,
        parse_ap_management_request,
    },
    block_ack::BlockAckAction,
    data::{
        DataInterfaceRole, EthernetFrameParts, IEEE80211_LEGACY_DATA_HEADER_LEN,
        IEEE80211_QOS_DATA_HEADER_LEN, plan_data_decapsulation,
    },
    security::LinkProtection,
};

use oer_ieee80211_runtime::await_stack_boundary;

use oer_ieee80211_softmac::MacRxEvidence;

use oer_ieee80211_rsn::{OwnedEapolFrame, RsnInterface};

use crate::diagnostics::core0_rx_performance::CORE0_PERFORMANCE;
#[cfg(feature = "diagnostics")]
use crate::diagnostics::network::{RxNetworkDeliveryEvent, RxNetworkDeliveryObserver};
#[cfg(any(feature = "diagnostics", test))]
use crate::{
    datapath::{irq::MacInterruptEpochDrain, rx::frontier::RxFrontierSchedulerSnapshot},
    diagnostics::{
        access_point::{
            AccessPointObservationStorage, AccessPointTerminalObservation,
            AccessPointTerminalObserver,
        },
        aggregate_tx::{
            AggregateBuildStop, AggregateTxObservation, AggregateTxObserver,
            PreparedTxSchedulerPhase,
        },
    },
};

use crate::{
    datapath::{
        DatapathControlContext, DatapathControlProgress, DatapathRunner, DatapathRxProgress,
        DatapathRxServiceContext, DatapathServices, DatapathStopProgress,
        irq::{InterruptEpoch, MacInterruptEpochActivateError, MacInterruptEpochQuiesceError},
        network::{DatapathNetworkLink, DatapathNetworkRx},
        rx::{
            reorder::{RX_REORDER_BACKING_SLOT_COUNT, RxReorderFrameStorage},
            staging::StagedEthernetPublication,
        },
        tx::aggregate::AggregateTxServiceEvent,
    },
    roles::concurrent::StaApRxBlockAck,
};
#[cfg(any(feature = "diagnostics", test))]
use oer_esp32s31_ieee80211_ap::ampdu::ApAmpduCompletion;

const EAPOL_ETHERTYPE: u16 = 0x888e;
const EAPOL_CAPACITY: usize = 512;

fn access_point_tx_batch_target(operational_window: Option<u16>, arena_capacity: usize) -> usize {
    operational_window.map_or(1, |window| usize::from(window).min(arena_capacity).max(1))
}

/// Publish the AP network endpoint only while at least one peer has completed
/// authorization. Standalone and paired lifecycles use this same policy so a
/// role composition cannot leave one permanent logical device down.
pub const fn access_point_network_link_state(authorized_peers: u8) -> LinkState {
    if authorized_peers == 0 {
        LinkState::Down
    } else {
        LinkState::Up
    }
}

#[cfg(any(feature = "diagnostics", test))]
fn observe_aggregate_rate(observer: &dyn AggregateTxObserver, rate: HtRate) {
    observer.observe(AggregateTxObservation::RateSelected {
        bandwidth_mhz: match rate.channel_width {
            HtChannelWidth::Mhz20 => 20,
            HtChannelWidth::Mhz40 => 40,
        },
        nominal_kbps: rate.nominal_kbps(),
    });
}

/// Preserve a real DMA continuation, but do not report an ordered AP protocol
/// head as a descriptor-writeback probe once the physical frontier is drained.
const fn ap_rx_progress_while_protocol_tx_blocked(dma: DatapathRxProgress) -> DatapathRxProgress {
    match dma {
        DatapathRxProgress::Drained | DatapathRxProgress::UpperLayerBlockedButDroppable => {
            DatapathRxProgress::ProtocolBlockedByTx
        }
        pending => pending,
    }
}

/// An active TX keeps hardware out of the protocol consumer. The enclosing
/// radio owner remains responsible for executing the consumer's typed mailbox
/// actions after the protocol borrow ends.
const fn rx_protocol_consumer_has_hardware(tx_pending: bool) -> bool {
    !tx_pending
}

/// Keep one reorder release on a single ordered publication path after an
/// older cold frame has entered the deferred batch.
const fn can_publish_ap_rx_in_place(
    current_staging_owner: bool,
    current_is_amsdu: bool,
    deferred_bytes: usize,
) -> bool {
    current_staging_owner && !current_is_amsdu && deferred_bytes == 0
}

mod ampdu;
pub mod concurrent;
mod datapath;
pub mod network_tx;
mod protocol_mailbox;
mod rx_pipeline;
mod rx_reorder;

pub use ampdu::AccessPointAmpdu;

pub use concurrent::{
    AccessPointRoleRuntime, StaApAccessPointFinishFailure, StaApAccessPointFinished,
    StaApAccessPointParkError, StaApAccessPointParkFailure, StaApAccessPointTxActive,
    StaApAccessPointTxParked, finish_sta_ap_access_point_role, park_sta_ap_access_point_role,
};

pub use self::rx_reorder::{AccessPointRxReorder, AccessPointRxReorderError};

use datapath::AccessPointDatapathServices;
#[cfg(any(feature = "diagnostics", test))]
use datapath::BlockAckObservationState;

use network_tx::AccessPointNetworkTx;

pub use protocol_mailbox::{
    AccessPointHardwareAction, AccessPointProtocolAction, AccessPointProtocolMailbox,
    AccessPointProtocolPublisher, AccessPointProtocolReceiver,
};
#[doc(hidden)]
pub use rx_pipeline::{
    AccessPointReceiveProducer, AccessPointRxConsumer, AccessPointRxProducer,
    AccessPointRxProducerObservation, AccessPointRxProtocolConsumer, AccessPointStagedRxFrame,
};

// One protected frame can produce a BlockAck-window reset and one peer
// activity update. The active-TX protocol quantum owns four frames, so the
// mailbox covers exactly one complete bounded turn.
const AP_PROTOCOL_ACTION_CAPACITY: usize = 8;
// Only a reorder-window reset crosses the protocol-to-PAC boundary. Peer
// activity and power-save state are role-local values and are committed
// directly by the AP protocol owner.
const AP_PROTOCOL_ACTIONS_PER_RX_FRAME: usize = 1;

include!("access_point/control_types.rs");
mod rx_rejection;

pub use rx_rejection::{AccessPointRxRejection, AccessPointRxRejectionReason};
include!("access_point/rx_dispatch.rs");
include!("access_point/protocol_owner.rs");
include!("access_point/control_owner.rs");
include!("access_point/protocol_service.rs");
include!("access_point/control_readiness.rs");
include!("access_point/protocol_shutdown.rs");
include!("access_point/service_epoch.rs");
include!("access_point/ethernet_diagnostics.rs");

#[cfg(test)]
mod lifecycle_tests;

use oer_esp32s31_ieee80211_ap::hardware::ApRuntimeHardware;
