use oer_esp32s31_hal::ieee80211::init::MacInterfaceAddressHardware;
use oer_esp32s31_hal::types::{MacDelaySlot, MacLegacyRate};
use oer_ieee80211_port::backend::RateScheduleKind;
use std::{cell::RefCell, rc::Rc};

use oer_esp32s31_hal::ieee80211::dma::descriptor::{
    DESCRIPTOR_BYTES, RxDescriptor, TxDescriptor, rx_armed_word, rx_landed_word, rx_rearm_word,
    tx_owned_word,
};
use oer_esp32s31_hal::types::{
    MacCcmpKeyIdentity, MacHtTxProgram, MacInterface, MacKeyInstallOutcome, MacLegacyTxProgram,
    MacTxCompletionObservation, MacTxDetachOutcome, MacTxDetachReason, MacTxQueueDetached,
};
use oer_ieee80211_port::{MacRxMetadata, RxEvidence};
use oer_ieee80211_wire::he::{HeMuSigBMimoUser, HeMuSigBNonMimoUser, HeMuSigBUser};
use oer_memory::{HardwareOwnedTxDma, PreparedTxDma};
use {
    oer_esp32s31_hal::{
        ieee80211::init::{
            MAC_COLD_HANDSHAKE_DEADLINE, MacCoexEvent, MacCoexPti, MacCoexPtiSource,
            MacColdAntennaHardware, MacColdCoexHardware, MacColdCoexPti, MacColdCryptoHardware,
            MacColdHalTailHardware, MacColdHandshakeHardware, MacColdHeHardware,
            MacColdLastRxBufferHardware, MacColdPrepareHardware, MacColdRxBufferHardware,
            MacColdRxPolicyHardware, MacColdStartConfig, MacColdStartError, MacColdTxRxHardware,
            MacDelayEntropy, MacLowRateHardware, MacResetHardware, MacSlowClockCalibration,
            MacSlowClockCalibrationSource, MacTxPowerSource, initialize_wifi_mac,
        },
        types::{MacTxPowerPair, MacTxPowerTable},
    },
    oer_esp32s31_ieee80211_driver::mac::{
        crypto::{
            CcmpKeyHardware, CryptoKeyError, clear_sta_ccmp_slots, install_sta_group_ccmp,
            install_sta_pairwise_ccmp,
        },
        init::{
            MacSnifferHardware, StaLinkRxPolicyHardware, activate_promiscuous_receive,
            configure_sta_link_receive_policy,
        },
        irq::{IrqWork, consumer::IrqStorage},
        rx::{
            HeBandwidth, HeGuardIntervalAndLtf, HeMuBandwidth, HeMuSignal, HeSuSignal,
            HeTriggerBasedSignal, HtDuplicateRxClassification, INGRESS_STRICT_DUMP,
            INGRESS_STRICT_RXEND, RxBasebandFormat, RxDma, RxDmaArenaState, RxError,
            RxHe20MuSigBUsersError, RxIngressConfig, RxPhyInfo, RxReloadObservation, RxRingError,
            RxRingLive, RxRingStopped, RxSegment, decode_normalized_rx_metadata,
            decode_rx_he_mu_sig_b, decode_rx_local_timestamp, decode_rx_phy_info,
            extract_ccmp_data, extract_control, first_segment_layout, view_normalized_rx_frame,
        },
        tx::{
            AmpduTxConfig, HeAmpduTxConfig, HeBccDcmMcs, HeEdcaTxopLimit, HeFecCoding,
            HeLdpcDcmMcs, HeMcs, HeRate, HeResourceUnit, HtAmpduDensity, HtAmpduTxConfig,
            HtChannelWidth, HtDuplicateCertificationRequest, HtDuplicateRate,
            HtDuplicateTxEvidenceGaps, HtDuplicateTxLinkCapabilities, HtDuplicateTxOracleField,
            HtDuplicateTxOracleGaps, HtDuplicateTxQualificationField,
            HtDuplicateTxQualificationGaps, HtDuplicateTxRejection, HtDuplicateTxSelection,
            HtDuplicateTxUnavailable, HtGuardInterval, HtMcs, HtPeerAmpduParameters,
            HtProtectionSpacing, HtRate, LegacyTxConfig, LegacyTxQueue, TxCompletion, TxDmaState,
            TxError, TxHardware, TxPhyRate, TxSlot, select_esp32s31_ht_duplicate_tx,
        },
    },
};

mod cases;
mod support;

fn free_slot<const N: usize>(
    slot: core::pin::Pin<&mut TxSlot<N>>,
) -> oer_esp32s31_ieee80211_driver::mac::tx::TxSlotFree<'_, N> {
    match slot.access() {
        oer_esp32s31_ieee80211_driver::mac::tx::TxSlotAccess::Free(free) => free,
        _ => panic!("fixture owns a free TX slot"),
    }
}
